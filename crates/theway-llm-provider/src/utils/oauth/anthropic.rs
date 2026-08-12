//! Anthropic OAuth (PKCE) flow. Partial 1:1 port of
//! `packages/ai/src/utils/oauth/anthropic.ts`.
//!
//! Implemented: PKCE challenge, authorize-URL building, code→token exchange, token refresh, and a
//! local-listener login that catches the redirect. The interactive "open the browser" step is
//! delegated to the caller via [`LoginCallbacks`] so this stays headless-testable.

use base64::Engine;
use serde::Deserialize;

use super::pkce::{PkcePair, generate_pkce};
use super::types::OAuthCredentials;

const AUTHORIZE_URL: &str = "https://claude.ai/oauth/authorize";
const TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
const CALLBACK_PORT: u16 = 53692;
const CALLBACK_PATH: &str = "/callback";
const SCOPES: &str = "org:create_api_key user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload";

/// Client id, base64-obfuscated in the TS source. Decoded at runtime to match 1:1.
fn client_id() -> String {
    let encoded = "OWQxYzI1MGEtZTYxYi00NGQ5LTg4ZWQtNTk0NGQxOTYyZjVl";
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .unwrap_or_default();
    String::from_utf8(bytes).unwrap_or_default()
}

fn redirect_uri() -> String {
    format!("http://localhost:{CALLBACK_PORT}{CALLBACK_PATH}")
}

/// Build the authorization URL the user must open. `state` is echoed back on the redirect.
pub fn build_authorize_url(challenge: &str, state: &str) -> String {
    let q = [
        ("response_type", "code"),
        ("client_id", &client_id()),
        ("redirect_uri", &redirect_uri()),
        ("scope", SCOPES),
        ("code_challenge", challenge),
        ("code_challenge_method", "S256"),
        ("state", state),
    ];
    let query: Vec<String> = q
        .iter()
        .map(|(k, v)| {
            format!(
                "{k}={}",
                percent_encoding::utf8_percent_encode(v, percent_encoding::NON_ALPHANUMERIC)
            )
        })
        .collect();
    format!("{AUTHORIZE_URL}?{}", query.join("&"))
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: String,
    expires_in: i64,
}

fn to_credentials(t: TokenResponse) -> OAuthCredentials {
    let now = chrono::Utc::now().timestamp_millis();
    OAuthCredentials {
        access_token: t.access_token,
        refresh_token: Some(t.refresh_token),
        // 5-minute safety margin, matching the TS impl.
        expires_at: Some(now + t.expires_in * 1000 - 5 * 60 * 1000),
        extra: None,
    }
}

/// Exchange an authorization code for tokens.
pub async fn exchange_authorization_code(
    code: &str,
    state: &str,
    verifier: &str,
) -> Result<OAuthCredentials, String> {
    let client = reqwest::Client::new();
    let resp = client
        .post(TOKEN_URL)
        .json(&serde_json::json!({
            "grant_type": "authorization_code",
            "client_id": client_id(),
            "code": code,
            "state": state,
            "redirect_uri": redirect_uri(),
            "code_verifier": verifier,
        }))
        .send()
        .await
        .map_err(|e| format!("token exchange request failed: {e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("token exchange failed ({status}): {body}"));
    }
    let token: TokenResponse = resp
        .json()
        .await
        .map_err(|e| format!("invalid token response: {e}"))?;
    Ok(to_credentials(token))
}

/// Refresh an access token using the stored refresh token.
pub async fn refresh(creds: &OAuthCredentials) -> Result<OAuthCredentials, String> {
    let refresh_token = creds
        .refresh_token
        .as_ref()
        .ok_or_else(|| "no refresh token available".to_string())?;
    let client = reqwest::Client::new();
    let resp = client
        .post(TOKEN_URL)
        .json(&serde_json::json!({
            "grant_type": "refresh_token",
            "client_id": client_id(),
            "refresh_token": refresh_token,
        }))
        .send()
        .await
        .map_err(|e| format!("token refresh request failed: {e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("token refresh failed ({status}): {body}"));
    }
    let token: TokenResponse = resp
        .json()
        .await
        .map_err(|e| format!("invalid token response: {e}"))?;
    Ok(to_credentials(token))
}

/// Hooks for the interactive parts of the login flow.
pub struct LoginCallbacks {
    /// Called with the authorization URL the user should open.
    pub open_url: Box<dyn Fn(&str) + Send>,
}

/// Run the full PKCE login: build the URL, hand it to the caller, listen on the loopback
/// callback port for the redirect, then exchange the code for tokens.
pub async fn login(callbacks: LoginCallbacks) -> Result<OAuthCredentials, String> {
    let PkcePair {
        verifier,
        challenge,
    } = generate_pkce();
    let state = uuid::Uuid::new_v4().to_string();
    let url = build_authorize_url(&challenge, &state);
    (callbacks.open_url)(&url);

    let (code, returned_state) = wait_for_redirect().await?;
    if returned_state != state {
        return Err("OAuth state mismatch (possible CSRF)".into());
    }
    exchange_authorization_code(&code, &state, &verifier).await
}

/// Listen once on the loopback callback port and parse `code`/`state` from the redirect query.
async fn wait_for_redirect() -> Result<(String, String), String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", CALLBACK_PORT))
        .await
        .map_err(|e| format!("cannot bind callback port {CALLBACK_PORT}: {e}"))?;
    let (mut socket, _) = listener
        .accept()
        .await
        .map_err(|e| format!("accept failed: {e}"))?;

    let mut buf = vec![0u8; 8192];
    let n = socket
        .read(&mut buf)
        .await
        .map_err(|e| format!("read failed: {e}"))?;
    let request = String::from_utf8_lossy(&buf[..n]);
    // First line: "GET /callback?code=...&state=... HTTP/1.1"
    let path = request
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .unwrap_or("");
    let (code, state) = parse_callback_query(path);

    let body = "<html><body><h1>Sign-in complete. You can close this tab.</h1></body></html>";
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    let _ = socket.write_all(response.as_bytes()).await;

    match (code, state) {
        (Some(c), Some(s)) => Ok((c, s)),
        _ => Err("redirect did not include code/state".into()),
    }
}

fn parse_callback_query(path: &str) -> (Option<String>, Option<String>) {
    let query = match path.split_once('?') {
        Some((_, q)) => q,
        None => return (None, None),
    };
    let mut code = None;
    let mut state = None;
    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            let decoded = percent_encoding::percent_decode_str(v)
                .decode_utf8_lossy()
                .to_string();
            match k {
                "code" => code = Some(decoded),
                "state" => state = Some(decoded),
                _ => {}
            }
        }
    }
    (code, state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_id_decodes() {
        assert_eq!(client_id(), "9d1c250a-e61b-44d9-88ed-5944d1962f5e");
    }

    #[test]
    fn authorize_url_contains_pkce_and_state() {
        let url = build_authorize_url("CHAL", "STATE123");
        assert!(url.starts_with("https://claude.ai/oauth/authorize?"));
        assert!(url.contains("code_challenge=CHAL"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("state=STATE123"));
        assert!(url.contains("response_type=code"));
    }

    #[test]
    fn parse_callback() {
        let (code, state) = parse_callback_query("/callback?code=abc123&state=xyz");
        assert_eq!(code.as_deref(), Some("abc123"));
        assert_eq!(state.as_deref(), Some("xyz"));
    }

    #[test]
    fn parse_callback_no_query() {
        let (code, state) = parse_callback_query("/callback");
        assert!(code.is_none() && state.is_none());
    }
}
