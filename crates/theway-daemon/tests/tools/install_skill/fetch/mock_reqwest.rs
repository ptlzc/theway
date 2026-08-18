//! Test-only `reqwest` module for `install_skill::fetch` URL tests.
//!
//! `fetch.rs` binds this module as `#[cfg(test)] mod reqwest;` so the URL success/stream-read
//! branches can be covered with scripted local responses (no real network). `Url` is
//! re-exported from the real reqwest crate, so URL parsing and the SSRF pre-flight behave
//! exactly as in production.

use std::collections::VecDeque;
use std::time::Duration;

use ::reqwest as real_reqwest;
use tokio::net::TcpStream;

pub use real_reqwest::Url;

pub mod redirect {
    pub struct Policy;

    impl Policy {
        pub fn limited(_: usize) -> Self {
            Policy
        }
    }
}

#[derive(Debug)]
pub struct Error(String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Status(u16);

impl Status {
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.0)
    }
}

impl std::fmt::Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

pub struct Response {
    status: Status,
    chunks: VecDeque<Result<Option<Vec<u8>>, Error>>,
}

impl Response {
    fn new(status: u16, chunks: Vec<Result<Option<Vec<u8>>, Error>>) -> Self {
        Self {
            status: Status(status),
            chunks: chunks.into(),
        }
    }

    pub fn status(&self) -> Status {
        self.status
    }

    pub async fn chunk(&mut self) -> Result<Option<Vec<u8>>, Error> {
        self.chunks.pop_front().unwrap_or(Ok(None))
    }
}

pub struct Client;

impl Client {
    pub fn builder() -> ClientBuilder {
        ClientBuilder
    }

    pub fn get(&self, url: Url) -> RequestBuilder {
        RequestBuilder(url)
    }
}

pub struct ClientBuilder;

impl ClientBuilder {
    pub fn timeout(self, _: Duration) -> Self {
        self
    }

    pub fn redirect(self, _: redirect::Policy) -> Self {
        self
    }

    pub fn user_agent(self, _: String) -> Self {
        self
    }

    pub fn build(self) -> Result<Client, Error> {
        Ok(Client)
    }
}

pub struct RequestBuilder(Url);

impl RequestBuilder {
    pub async fn send(self) -> Result<Response, Error> {
        // Yield once so a pre-cancelled select always observes the cancellation branch
        // before this future resolves.
        tokio::task::yield_now().await;

        let url = self.0;
        let host = url.host_str().unwrap_or("").to_string();
        match host.as_str() {
            "mock-status.example" => Ok(Response::new(500, vec![])),
            "mock-ok.example" => Ok(Response::new(
                200,
                vec![
                    Ok(Some(b"hello".to_vec())),
                    Ok(Some(b" world".to_vec())),
                    Ok(None),
                ],
            )),
            "mock-read-error.example" => Ok(Response::new(
                200,
                vec![Ok(Some(b"abc".to_vec())), Err(Error("read body".into()))],
            )),
            "mock-invalid-utf8.example" => Ok(Response::new(200, vec![Ok(Some(vec![0xff, 0xfe]))])),
            "mock-oom.example" => Ok(Response::new(
                200,
                vec![
                    Ok(Some(vec![0; 16 * 1024 * 1024 - 1])),
                    Ok(Some(vec![0; 2])),
                ],
            )),
            _ => {
                // Non-scripted hosts are still local fixtures in this test suite: the only
                // non-scripted URL path exercised is the IPv4-mapped IPv6 loopback used by
                // the connection-error test. Connecting to it lets that test's local TCP
                // server finish; then we surface the same TLS-level error the real client
                // would report.
                let trimmed = host
                    .trim_start_matches('[')
                    .trim_end_matches(']')
                    .to_string();
                if let Ok(ip) = trimmed.parse::<std::net::IpAddr>() {
                    let port = url.port().unwrap_or(443);
                    let _ = TcpStream::connect((ip, port)).await;
                }
                Err(Error("fetch failed".into()))
            }
        }
    }
}
