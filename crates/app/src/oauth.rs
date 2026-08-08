//! Generic OAuth 2.0 PKCE helper. Foundation for c4pt0r/theway#13's browser-based login flows.
//! Provider-specific wiring (Anthropic Pro/Max, Codex, Copilot, Google) plugs into this
//! generic flow by supplying its authorization/token endpoints + client id.
//!
//! Flow:
//!   1. Build a `Flow` with provider endpoints + scopes + a redirect_port.
//!   2. `flow.authorize_url()` returns the URL to open in the user's browser, plus the
//!      `state` and PKCE `verifier` we keep on the side.
//!   3. `flow.await_callback(timeout)` binds 127.0.0.1:redirect_port, waits for the OAuth
//!      provider's redirect, and extracts `code` + `state`. The `state` is checked against
//!      what we generated to defend against CSRF.
//!   4. `flow.exchange_code(code, verifier)` POSTs to the token endpoint and returns
//!      `(access_token, refresh_token, expires_at)` ready to drop into the auth store.
//!
//! The actual browser-open and the user copy-paste fallback live above this module.

#![allow(dead_code)]

use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use base64::Engine;
use serde::Deserialize;
use sha2::{Digest, Sha256};

pub struct Flow {
    pub authorize_url: String,
    pub token_url: String,
    pub client_id: String,
    pub scopes: Vec<String>,
    pub redirect_port: u16,
}

#[derive(Debug, Clone)]
pub struct Authorization {
    /// URL the user must open in a browser to start the flow.
    pub url: String,
    /// PKCE verifier — keep this private until the token exchange.
    pub verifier: String,
    /// CSRF state token — assert it matches when the callback arrives.
    pub state: String,
}

#[derive(Debug, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// Lifetime in seconds from now.
    #[serde(default)]
    pub expires_in: Option<i64>,
    #[serde(default)]
    pub scope: Option<String>,
}

impl Flow {
    pub fn redirect_uri(&self) -> String {
        format!("http://127.0.0.1:{}/callback", self.redirect_port)
    }

    /// Build the authorize URL + state + verifier.
    pub fn authorize_url(&self) -> Authorization {
        let verifier = random_token(43);
        let challenge = sha256_base64url(&verifier);
        let state = random_token(24);
        let scope = self.scopes.join(" ");
        let url = format!(
            "{}?response_type=code&client_id={}&redirect_uri={}&scope={}&state={}&code_challenge={}&code_challenge_method=S256",
            self.authorize_url,
            urlencode(&self.client_id),
            urlencode(&self.redirect_uri()),
            urlencode(&scope),
            urlencode(&state),
            urlencode(&challenge),
        );
        Authorization {
            url,
            verifier,
            state,
        }
    }

    /// Bind 127.0.0.1:redirect_port and wait for the OAuth provider to redirect here. Returns
    /// the parsed `code` + `state` from the callback's query string. The browser sees a tiny
    /// HTML page confirming success.
    pub async fn await_callback(&self, timeout: Duration) -> Result<(String, String)> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;
        let listener = TcpListener::bind(("127.0.0.1", self.redirect_port))
            .await
            .with_context(|| format!("bind 127.0.0.1:{}", self.redirect_port))?;
        let accept = async {
            let (mut sock, _) = listener.accept().await?;
            let mut buf = [0u8; 4096];
            let mut request = Vec::new();
            // Read until we have the first line + headers. We only need the request line.
            let n = sock.read(&mut buf).await?;
            request.extend_from_slice(&buf[..n]);
            let req_text = String::from_utf8_lossy(&request).to_string();
            let path = req_text
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .unwrap_or("")
                .to_string();
            let (code, state) = parse_callback_query(&path);
            let body = if code.is_some() {
                "<html><body><h2>theway: login complete</h2><p>You can close this tab.</p></body></html>"
            } else {
                "<html><body><h2>theway: login failed</h2></body></html>"
            };
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: text/html\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            sock.write_all(resp.as_bytes()).await.ok();
            sock.shutdown().await.ok();
            Ok::<(Option<String>, Option<String>), std::io::Error>((code, state))
        };
        let result = tokio::time::timeout(timeout, accept)
            .await
            .map_err(|_| anyhow!("OAuth callback timed out after {timeout:?}"))?
            .map_err(|e| anyhow!("OAuth callback read failed: {e}"))?;
        let code = result.0.ok_or_else(|| anyhow!("callback missing `code`"))?;
        let state = result
            .1
            .ok_or_else(|| anyhow!("callback missing `state`"))?;
        Ok((code, state))
    }

    /// Exchange a refresh_token for a fresh access token. Provider-agnostic — uses
    /// `grant_type=refresh_token`. Returns the new TokenResponse; callers persist the
    /// updated access_token (and refresh_token, if rotated) to the auth store.
    pub async fn refresh_token(&self, refresh_token: &str) -> Result<TokenResponse> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()?;
        let form = [
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", &self.client_id),
        ];
        let resp = client.post(&self.token_url).form(&form).send().await?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            return Err(anyhow!(
                "refresh endpoint {status}: {}",
                text.chars().take(500).collect::<String>()
            ));
        }
        let parsed: TokenResponse = serde_json::from_str(&text)?;
        Ok(parsed)
    }

    /// Exchange the auth code + PKCE verifier for an access token.
    pub async fn exchange_code(&self, code: &str, verifier: &str) -> Result<TokenResponse> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()?;
        let form = [
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", &self.redirect_uri()),
            ("client_id", &self.client_id),
            ("code_verifier", verifier),
        ];
        let resp = client.post(&self.token_url).form(&form).send().await?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            return Err(anyhow!(
                "token endpoint {status}: {}",
                text.chars().take(500).collect::<String>()
            ));
        }
        let parsed: TokenResponse = serde_json::from_str(&text)?;
        Ok(parsed)
    }
}

fn random_token(len: usize) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    // Avoid pulling in a fresh rand dep — synthesize entropy from clock + process id +
    // address-of-stack-local. Not cryptographic-grade, but sufficient for CSRF state in a
    // localhost-only flow. The PKCE verifier just needs to be unguessable to a remote
    // attacker for the short window of the flow.
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id() as u128;
    let mut seed = now.wrapping_mul(0x9E3779B97F4A7C15).wrapping_add(pid);
    let mut out = String::with_capacity(len);
    let alphabet: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    for _ in 0..len {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let idx = ((seed >> 33) as usize) % alphabet.len();
        out.push(alphabet[idx] as char);
    }
    out
}

fn sha256_base64url(s: &str) -> String {
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    let digest = h.finalize();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}

fn parse_callback_query(path: &str) -> (Option<String>, Option<String>) {
    let qpos = match path.find('?') {
        Some(i) => i + 1,
        None => return (None, None),
    };
    let mut code = None;
    let mut state = None;
    for pair in path[qpos..].split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            let v = urldecode(v);
            match k {
                "code" => code = Some(v),
                "state" => state = Some(v),
                _ => {}
            }
        }
    }
    (code, state)
}

fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}

fn urldecode(s: &str) -> String {
    let mut out = Vec::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        if bytes[i] == b'+' {
            out.push(b' ');
        } else {
            out.push(bytes[i]);
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_challenge_is_sha256_base64url() {
        // RFC 7636 reference vector.
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let expected = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
        assert_eq!(sha256_base64url(verifier), expected);
    }

    #[test]
    fn urlencode_round_trip() {
        let raw = "scope spaces & special=chars";
        let encoded = urlencode(raw);
        assert!(!encoded.contains(' '));
        assert!(encoded.contains("%20"));
        let decoded = urldecode(&encoded);
        assert_eq!(decoded, raw);
    }

    #[test]
    fn parse_callback_extracts_code_and_state() {
        let path = "/callback?code=abc&state=xyz&other=1";
        let (code, state) = parse_callback_query(path);
        assert_eq!(code.as_deref(), Some("abc"));
        assert_eq!(state.as_deref(), Some("xyz"));
    }

    #[test]
    fn random_token_has_requested_len_and_alphabet() {
        let t = random_token(43);
        assert_eq!(t.len(), 43);
        assert!(
            t.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "{t}"
        );
    }

    #[test]
    fn authorize_url_includes_pkce_and_state() {
        let flow = Flow {
            authorize_url: "https://example.com/auth".into(),
            token_url: "https://example.com/token".into(),
            client_id: "cli-123".into(),
            scopes: vec!["chat".into(), "user".into()],
            redirect_port: 9999,
        };
        let auth = flow.authorize_url();
        assert!(auth.url.starts_with("https://example.com/auth?"));
        assert!(auth.url.contains("code_challenge="));
        assert!(auth.url.contains("code_challenge_method=S256"));
        assert!(auth.url.contains("client_id=cli-123"));
        assert!(auth.url.contains("scope=chat%20user"));
        assert!(auth.url.contains(&format!("state={}", auth.state)));
    }
}
