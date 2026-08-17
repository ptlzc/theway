//! Tests for `relay` — split out of src (see docs/rust-test-files.md).

use super::*;

#[test]
fn tokens_are_long_random_and_url_safe() {
    let a = new_token();
    let b = new_token();
    assert_ne!(a, b, "tokens must be random");
    assert_eq!(a.len(), 40, "{a}");
    assert!(
        a.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
        "token must be URL-safe: {a}"
    );
}

#[test]
fn ws_url_derives_scheme_and_path_from_base() {
    assert_eq!(
        agent_ws_url("https://pie.0xfefe.me", "tok123").unwrap(),
        "wss://pie.0xfefe.me/relay/agent?token=tok123"
    );
    assert_eq!(
        agent_ws_url("http://127.0.0.1:8787/", "tok123").unwrap(),
        "ws://127.0.0.1:8787/relay/agent?token=tok123"
    );
    assert!(agent_ws_url("ftp://nope", "t").is_err());
}

#[test]
fn viewer_url_is_session_path_with_trailing_slash() {
    // The trailing slash matters: the shared HTML uses relative fetch paths, so
    // /session/<token> (no slash) would resolve them against /session/.
    assert_eq!(
        viewer_url("https://pie.0xfefe.me", "tok123"),
        "https://pie.0xfefe.me/session/tok123/"
    );
    assert_eq!(
        viewer_url("http://127.0.0.1:8787/", "tok123"),
        "http://127.0.0.1:8787/session/tok123/"
    );
}

#[test]
fn qr_lines_render_a_scannable_block_grid() {
    let lines = qr_lines("https://pie.0xfefe.me/session/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/")
        .expect("urls of this shape must encode");
    assert!(
        lines.len() > 10,
        "expected a QR-sized grid, got {}",
        lines.len()
    );
    let width = lines[0].chars().count();
    assert!(width > 10);
    assert!(
        lines.iter().all(|l| l.chars().count() == width),
        "all QR lines must be equal width"
    );
    let blocks: usize = lines
        .iter()
        .map(|l| l.chars().filter(|c| "█▀▄".contains(*c)).count())
        .sum();
    assert!(blocks > 50, "expected block characters, got {blocks}");
}

#[test]
fn frames_round_trip_as_tagged_json() {
    let hello = serde_json::to_string(&AgentFrame::Hello {
        agent_key: "k".into(),
    })
    .unwrap();
    assert!(hello.contains("\"type\":\"hello\""), "{hello}");

    let prompt: WorkerFrame = serde_json::from_str(r#"{"type":"prompt","text":"hi"}"#).unwrap();
    assert_eq!(prompt, WorkerFrame::Prompt { text: "hi".into() });
    let viewers: WorkerFrame = serde_json::from_str(r#"{"type":"viewers","count":3}"#).unwrap();
    assert_eq!(viewers, WorkerFrame::Viewers { count: 3 });
    let abort: WorkerFrame = serde_json::from_str(r#"{"type":"abort"}"#).unwrap();
    assert_eq!(abort, WorkerFrame::Abort);
    let resolve: WorkerFrame =
        serde_json::from_str(r#"{"type":"control_plane_resolve","approve":true}"#).unwrap();
    assert_eq!(resolve, WorkerFrame::ControlPlaneResolve { approve: true });
    let set_model: WorkerFrame =
        serde_json::from_str(r#"{"type":"set_model","model":"anthropic:claude-haiku-4-5"}"#)
            .unwrap();
    assert_eq!(
        set_model,
        WorkerFrame::SetModel {
            model: "anthropic:claude-haiku-4-5".into()
        }
    );
}
