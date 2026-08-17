//! Tests for `bedrock_anthropic` — split out of src (see docs/rust-test-files.md).

use super::*;
use std::collections::HashMap;

fn frame(payload_json: &str) -> EventMessage {
    let b64 = base64::engine::general_purpose::STANDARD.encode(payload_json);
    let outer = serde_json::json!({ "bytes": b64 });
    EventMessage {
        headers: HashMap::new(),
        payload: serde_json::to_vec(&outer).unwrap(),
    }
}

#[test]
fn full_text_turn_round_trip() {
    let mut c = Converter::new();
    // message_start
    let events = c
        .ingest(&frame(
            r#"{"type":"message_start","message":{"id":"m1","model":"claude","role":"assistant"}}"#,
        ))
        .unwrap();
    assert!(matches!(
        events.first(),
        Some(AssistantMessageEvent::Start { .. })
    ));
    // content_block_start (text)
    let events = c
        .ingest(&frame(
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
        ))
        .unwrap();
    assert!(matches!(
        events.first(),
        Some(AssistantMessageEvent::TextStart { .. })
    ));
    // delta
    let events = c
            .ingest(&frame(
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hello"}}"#,
            ))
            .unwrap();
    match events.first() {
        Some(AssistantMessageEvent::TextDelta { delta, .. }) => assert_eq!(delta, "hello"),
        other => panic!("unexpected: {other:?}"),
    }
    // stop block
    let events = c
        .ingest(&frame(r#"{"type":"content_block_stop","index":0}"#))
        .unwrap();
    match events.first() {
        Some(AssistantMessageEvent::TextEnd { content, .. }) => assert_eq!(content, "hello"),
        other => panic!("unexpected: {other:?}"),
    }
    // usage
    let _ = c
            .ingest(&frame(
                r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"input_tokens":7,"output_tokens":3}}"#,
            ))
            .unwrap();
    // message_stop
    let events = c.ingest(&frame(r#"{"type":"message_stop"}"#)).unwrap();
    match events.first() {
        Some(AssistantMessageEvent::Done { reason, message }) => {
            assert_eq!(*reason, DoneReason::Stop);
            assert_eq!(message.usage.input, 7);
            assert_eq!(message.usage.output, 3);
        }
        other => panic!("unexpected terminal: {other:?}"),
    }
}

#[test]
fn error_event_emits_error_variant() {
    let mut c = Converter::new();
    let events = c
        .ingest(&frame(
            r#"{"type":"error","error":{"message":"too many tokens"}}"#,
        ))
        .unwrap();
    match events.first() {
        Some(AssistantMessageEvent::Error { error, .. }) => {
            assert_eq!(error.error_message.as_deref(), Some("too many tokens"));
        }
        other => panic!("expected Error variant, got {other:?}"),
    }
}

#[test]
fn ping_emits_nothing() {
    let mut c = Converter::new();
    let events = c.ingest(&frame(r#"{"type":"ping"}"#)).unwrap();
    assert!(events.is_empty());
}
