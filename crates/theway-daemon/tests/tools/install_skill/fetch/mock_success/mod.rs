//! Success/stream-read tests for `fetch_url` using the test-only local `reqwest` module
//! (see `mock_reqwest.rs`, bound from `fetch.rs` under `#[cfg(test)] mod reqwest;`).
//!
//! The scripted hosts below exercise the URL response path without a webpki-trusted local
//! TLS fixture or real network.

use super::super::*;

#[tokio::test]
async fn fetch_url_rejects_non_success_status() {
    // Arrange
    let url = "https://mock-status.example/skill.md";

    // Act
    let err = fetch_url(url, &CancellationToken::new())
        .await
        .err()
        .expect("500 response must fail");

    // Assert
    let msg = err.to_string();
    assert!(msg.contains("non-success status"), "got: {msg}");
    assert!(msg.contains("500"), "got: {msg}");
}

#[tokio::test]
async fn fetch_url_reads_chunks_until_eof_and_returns_utf8_body() {
    // Arrange
    let url = "https://mock-ok.example/skill.md";

    // Act
    let fetched = fetch_url(url, &CancellationToken::new())
        .await
        .expect("success response should parse");

    // Assert
    assert_eq!(fetched.content, "hello world");
}

#[tokio::test]
async fn fetch_url_rejects_body_read_error() {
    // Arrange
    let url = "https://mock-read-error.example/skill.md";

    // Act
    let err = fetch_url(url, &CancellationToken::new())
        .await
        .err()
        .expect("stream read error must fail");

    // Assert
    assert!(err.to_string().contains("read body"), "got: {err}");
}

#[tokio::test]
async fn fetch_url_rejects_invalid_utf8_body() {
    // Arrange
    let url = "https://mock-invalid-utf8.example/skill.md";

    // Act
    let err = fetch_url(url, &CancellationToken::new())
        .await
        .err()
        .expect("invalid utf-8 must fail");

    // Assert
    assert!(err.to_string().contains("not valid utf-8"), "got: {err}");
}

#[tokio::test]
async fn fetch_url_rejects_body_over_oom_guard() {
    // Arrange: first chunk fills the guard to one byte short; the second trips it.
    let url = "https://mock-oom.example/skill.md";

    // Act
    let err = fetch_url(url, &CancellationToken::new())
        .await
        .err()
        .expect("oversized body must fail");

    // Assert
    let msg = err.to_string();
    assert!(msg.contains("exceeds"), "got: {msg}");
    assert!(msg.contains("in-memory guard"), "got: {msg}");
}
