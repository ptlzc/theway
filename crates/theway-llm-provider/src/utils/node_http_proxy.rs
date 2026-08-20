//! HTTP/HTTPS proxy support. TODO: 1:1 port of `packages/ai/src/utils/node-http-proxy.ts`.
//!
//! TS uses `undici`'s `ProxyAgent`; Rust uses `reqwest::Proxy`. Honors `HTTP_PROXY`,
//! `HTTPS_PROXY`, and `NO_PROXY` env vars.

use std::env;

/// Return a `reqwest::Proxy` configured from env vars, or `None` if no proxy is set.
pub fn proxy_from_env() -> Option<reqwest::Proxy> {
    let url = env::var("HTTPS_PROXY")
        .ok()
        .or_else(|| env::var("https_proxy").ok())
        .or_else(|| env::var("HTTP_PROXY").ok())
        .or_else(|| env::var("http_proxy").ok())?;
    reqwest::Proxy::all(&url)
        .ok()
        .map(|proxy| proxy.no_proxy(reqwest::NoProxy::from_env()))
}

/// Build a `reqwest::Client` honoring proxy env vars and theway-llm-provider defaults.
pub fn build_client(timeout_ms: Option<u64>) -> reqwest::Result<reqwest::Client> {
    let mut b = reqwest::Client::builder()
        .user_agent(crate::utils::headers::user_agent())
        .connect_timeout(std::time::Duration::from_secs(15));
    if let Some(p) = proxy_from_env() {
        b = b.proxy(p);
    }
    if let Some(ms) = timeout_ms {
        b = b.timeout(std::time::Duration::from_millis(ms));
    }
    b.build()
}
