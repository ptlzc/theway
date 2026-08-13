//! Source fetching for `InstallSkill` — resolve the skill body from one of the three
//! accepted sources: `https://` URL, absolute local path, or inline content.
//!
//! URL fetches are SSRF-guarded (https-only, loopback / RFC1918 / link-local /
//! `.localhost` hosts pre-flight rejected), time-bounded, and stream-read behind the
//! `SKILL_FETCH_OOM_GUARD_BYTES` memory guard. See the parent module docs for the full
//! safety model.

use std::path::PathBuf;
use std::time::Duration;

use theway_core::AgentToolError;
use tokio_util::sync::CancellationToken;

use super::{HTTP_TIMEOUT_SECS, SKILL_FETCH_OOM_GUARD_BYTES, Source};

pub(super) struct Fetched {
    pub(super) content: String,
}

pub(super) async fn fetch_source(
    source: &Source,
    cancel: &CancellationToken,
) -> Result<Fetched, AgentToolError> {
    match source {
        Source::Url { url } => fetch_url(url, cancel).await,
        Source::Path { path } => fetch_path(path).await,
        Source::Content { content } => Ok(fetch_inline(content)),
    }
}

async fn fetch_url(url: &str, cancel: &CancellationToken) -> Result<Fetched, AgentToolError> {
    let parsed = reqwest::Url::parse(url)
        .map_err(|e| AgentToolError::Message(format!("invalid url: {e}")))?;
    if parsed.scheme() != "https" {
        return Err(AgentToolError::Message(
            "url must use https:// (http, file, data, and other schemes are refused)".into(),
        ));
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| AgentToolError::from("url must have a host"))?;
    if is_private_or_local_host(host) {
        return Err(AgentToolError::Message(format!(
            "refusing to fetch from local/private host '{host}' (SSRF guard)"
        )));
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
        .redirect(reqwest::redirect::Policy::limited(5))
        .user_agent(format!("theway/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| AgentToolError::Message(format!("http client init: {e}")))?;

    let fut = client.get(parsed).send();
    let mut resp = tokio::select! {
        r = fut => r.map_err(|e| AgentToolError::Message(format!("fetch failed: {e}")))?,
        _ = cancel.cancelled() => return Err(AgentToolError::Message("cancelled".into())),
    };
    if !resp.status().is_success() {
        return Err(AgentToolError::Message(format!(
            "fetch returned non-success status: {}",
            resp.status()
        )));
    }
    // Stream-read with cap so a hostile server can't OOM the agent.
    let mut buf = Vec::<u8>::new();
    loop {
        let chunk = tokio::select! {
            r = resp.chunk() => r,
            _ = cancel.cancelled() => return Err(AgentToolError::Message("cancelled".into())),
        };
        match chunk {
            Ok(Some(c)) => {
                if buf.len() + c.len() > SKILL_FETCH_OOM_GUARD_BYTES {
                    // Pure OOM guard, not a per-skill artifact cap. See module-level docs.
                    return Err(AgentToolError::Message(format!(
                        "fetched skill body exceeds {SKILL_FETCH_OOM_GUARD_BYTES}-byte \
                         in-memory guard ({} bytes received so far); refusing to install \
                         from a stream this large",
                        buf.len()
                    )));
                }
                buf.extend_from_slice(&c);
            }
            Ok(None) => break,
            Err(e) => {
                return Err(AgentToolError::Message(format!("read body: {e}")));
            }
        }
    }
    let content = String::from_utf8(buf)
        .map_err(|e| AgentToolError::Message(format!("skill body is not valid utf-8: {e}")))?;
    Ok(Fetched { content })
}

async fn fetch_path(path: &str) -> Result<Fetched, AgentToolError> {
    let p = PathBuf::from(path);
    if !p.is_absolute() {
        return Err(AgentToolError::from(
            "path must be absolute (relative paths are ambiguous in agent context)",
        ));
    }
    let meta = tokio::fs::metadata(&p)
        .await
        .map_err(|e| AgentToolError::Message(format!("stat {}: {e}", p.display())))?;
    if !meta.is_file() {
        return Err(AgentToolError::Message(format!(
            "{} is not a regular file",
            p.display()
        )));
    }
    // Local fs source is user-trusted (they pointed at this path) — same OOM guard as
    // the URL stream-read, just to keep memory bounded if the path points at something
    // unexpectedly huge.
    if meta.len() as usize > SKILL_FETCH_OOM_GUARD_BYTES {
        return Err(AgentToolError::Message(format!(
            "{} ({} bytes) exceeds {SKILL_FETCH_OOM_GUARD_BYTES}-byte in-memory guard",
            p.display(),
            meta.len()
        )));
    }
    let content = tokio::fs::read_to_string(&p)
        .await
        .map_err(|e| AgentToolError::Message(format!("read {}: {e}", p.display())))?;
    Ok(Fetched { content })
}

fn fetch_inline(content: &str) -> Fetched {
    Fetched {
        content: content.to_string(),
    }
}

/// Reject hostnames that point at the loopback / private RFC1918 / link-local space.
/// Pre-flight check: refuses the request before the HTTP client gets a chance to follow a
/// DNS rebinding or hit a local service. Not airtight (a hostile DNS could still resolve a
/// public name to a private IP), but raises the bar.
fn is_private_or_local_host(host: &str) -> bool {
    let host_lower = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_ascii_lowercase();
    if matches!(
        host_lower.as_str(),
        "localhost" | "ip6-localhost" | "ip6-loopback" | "broadcasthost"
    ) {
        return true;
    }
    if host_lower.ends_with(".localhost") || host_lower.ends_with(".local") {
        return true;
    }
    if let Ok(ip) = host_lower.parse::<std::net::IpAddr>() {
        return match ip {
            std::net::IpAddr::V4(v4) => {
                v4.is_loopback()
                    || v4.is_private()
                    || v4.is_link_local()
                    || v4.is_unspecified()
                    || v4.is_broadcast()
            }
            std::net::IpAddr::V6(v6) => {
                v6.is_loopback() || v6.is_unspecified() || v6.segments()[0] & 0xfe00 == 0xfc00
            }
        };
    }
    false
}
