//! Unit tests for HTTP transport helpers: the loopback bind policy.
//!
//! App-side helper tests (feed-line projection, prompt image decoding) live in
//! `theway::ui::web_loop` (they exercise App-owned functions).

use super::super::*;

#[test]
fn bind_addr_rejects_remote_by_default() {
    let err = bind_addr("0.0.0.0", 0).unwrap_err().to_string();
    assert!(err.contains("refusing non-loopback"));
}

#[test]
fn bind_addr_accepts_loopback_and_localhost() {
    let local = bind_addr("127.0.0.1", 0).unwrap();
    assert!(local.ip().is_loopback());

    let named = bind_addr("localhost", 0).unwrap();
    assert!(named.ip().is_loopback());
}
