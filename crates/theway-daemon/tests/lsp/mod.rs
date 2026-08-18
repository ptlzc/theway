//! Mirrored tests for `lsp` — the private `read_framed` Content-Length
//! parser and the `Diagnostic` serde shape, without spawning a real LSP
//! server.

use tokio::io::BufReader;

use super::*;

fn framed_reader(frame: &[u8]) -> BufReader<&[u8]> {
    BufReader::new(frame)
}

#[tokio::test]
async fn read_framed_returns_none_at_eof_before_headers() {
    let mut reader = framed_reader(&[]);

    let value = read_framed(&mut reader).await.unwrap();

    assert_eq!(value, None);
}

#[tokio::test]
async fn read_framed_parses_crlf_content_length_frame() {
    let body = br#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#;
    let mut frame = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    frame.extend_from_slice(body);
    let mut reader = framed_reader(&frame);

    let value = read_framed(&mut reader).await.unwrap().unwrap();

    assert_eq!(value["id"], 1);
    assert_eq!(value["result"]["ok"], true);
}

#[tokio::test]
async fn read_framed_parses_lf_headers_and_ignores_unknown_headers() {
    let body = br#"{"jsonrpc":"2.0","id":2,"result":null}"#;
    let mut frame = format!("Content-Type: application/vscode-jsonrpc\nContent-Length: {}\n\n", body.len()).into_bytes();
    frame.extend_from_slice(body);
    let mut reader = framed_reader(&frame);

    let value = read_framed(&mut reader).await.unwrap().unwrap();

    assert_eq!(value["id"], 2);
    assert!(value["result"].is_null());
}

#[tokio::test]
async fn read_framed_rejects_missing_content_length() {
    let frame = b"Header: value\r\n\r\n";
    let mut reader = framed_reader(frame);

    let err = read_framed(&mut reader).await.unwrap_err();

    assert!(err.to_string().contains("Content-Length"), "{err}");
}

#[tokio::test]
async fn read_framed_rejects_invalid_json_body() {
    let body = b"not json";
    let mut frame = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    frame.extend_from_slice(body);
    let mut reader = framed_reader(&frame);

    let err = read_framed(&mut reader).await.unwrap_err();

    assert!(err.to_string().contains("json") || err.to_string().contains("expected"), "{err}");
}

#[tokio::test]
async fn read_framed_rejects_truncated_body() {
    let frame = b"Content-Length: 10\r\n\r\nabc".to_vec();
    let mut reader = framed_reader(&frame);

    let err = read_framed(&mut reader).await.unwrap_err();

    assert!(err.to_string().contains("early eof") || err.to_string().contains("failed to fill"), "{err}");
}

#[test]
fn diagnostic_range_and_position_serde_round_trip() {
    let diag = Diagnostic {
        range: DiagnosticRange {
            start: Position { line: 3, character: 1 },
            end: Position { line: 3, character: 4 },
        },
        severity: Some(1),
        message: "expected `;`".into(),
        source: Some("mock".into()),
    };

    let json = serde_json::to_value(&diag).unwrap();
    let back: Diagnostic = serde_json::from_value(json).unwrap();

    assert_eq!(back.message, "expected `;`");
    assert_eq!(back.severity, Some(1));
    assert_eq!(back.source.as_deref(), Some("mock"));
    assert_eq!(back.range.start.line, 3);
    assert_eq!(back.range.end.character, 4);
}

#[test]
fn diagnostic_serde_defaults_missing_fields() {
    let json = serde_json::json!({
        "range": {
            "start": {"line": 1, "character": 0},
            "end": {"line": 1, "character": 2}
        },
        "message": "missing severity and source"
    });

    let diag: Diagnostic = serde_json::from_value(json).unwrap();

    assert_eq!(diag.severity, None);
    assert_eq!(diag.source, None);
}
