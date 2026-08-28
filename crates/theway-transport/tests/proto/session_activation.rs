use super::*;
use prost::Message;

fn fixture_summary() -> crate::wire::SessionSummary {
    crate::wire::SessionSummary {
        session_id: "sess-1".into(),
        name: "main".into(),
        cwd: "/tmp/theway".into(),
        model: "anthropic:claude-x".into(),
        created_at: "2026-08-01T00:00:00Z".into(),
        last_activity_at: 1234,
        graph_count: 3,
        active_graph_count: 1,
        busy: false,
        preview: Some("last prompt".into()),
        metadata: std::collections::HashMap::new(),
    }
}

#[test]
fn session_runtime_context_preserves_optional_presence() {
    let runtime = crate::wire::WireSessionRuntimeContext {
        work_dir: "/work".into(),
        provider: None,
        model: Some("claude-x".into()),
        base_url: Some(String::new()),
        thinking: Some(false),
    };
    let proto = session_runtime_context_to_proto(&runtime);
    assert_eq!(proto.work_dir, "/work");
    assert!(proto.provider.is_none());
    assert_eq!(proto.model.as_deref(), Some("claude-x"));
    assert_eq!(proto.base_url.as_deref(), Some(""));
    assert_eq!(proto.thinking, Some(false));
    assert_eq!(session_runtime_context_from_proto(&proto), runtime);

    let omitted = crate::wire::WireSessionRuntimeContext {
        base_url: None,
        ..Default::default()
    };
    assert!(session_runtime_context_to_proto(&omitted).base_url.is_none());
    let explicit_empty = crate::wire::WireSessionRuntimeContext {
        base_url: Some(String::new()),
        ..Default::default()
    };
    assert_eq!(
        session_runtime_context_to_proto(&explicit_empty)
            .base_url
            .as_deref(),
        Some("")
    );
}

#[test]
fn activate_session_request_converts_proto_tags_and_optional_fields() {
    let bytes = [
        0x0a, 0x03, b'1', b'2', b'3', // session_id = "123"
        0x12, 0x03, b'k', b'e', b'y', // client_key = "key"
        0x1a, 0x04, b'n', b'a', b'm', b'e', // name = "name"
        0x22, 0x02, 0x0a, 0x00, // runtime { work_dir = "" }
    ];
    let proto = wire::ActivateSessionRequest::decode(bytes.as_slice()).unwrap();
    let request = activate_session_request_from_proto(&proto).unwrap();
    assert_eq!(request.session_id.as_deref(), Some("123"));
    assert_eq!(request.client_key, "key");
    assert_eq!(request.name.as_deref(), Some("name"));
    assert_eq!(request.runtime.as_ref().unwrap().work_dir, "");
    assert!(request.runtime.as_ref().unwrap().provider.is_none());

    let optional_absent = wire::ActivateSessionRequest {
        session_id: None,
        client_key: "key".into(),
        name: None,
        runtime: Some(session_runtime_context_to_proto(
            &crate::wire::WireSessionRuntimeContext {
                work_dir: "/work".into(),
                provider: Some(String::new()),
                ..Default::default()
            },
        )),
    };
    let request = activate_session_request_from_proto(&optional_absent).unwrap();
    assert!(request.session_id.is_none());
    assert!(request.name.is_none());
    assert_eq!(
        request.runtime.as_ref().unwrap().provider.as_deref(),
        Some("")
    );
}

#[test]
fn activate_session_request_requires_runtime() {
    let proto = wire::ActivateSessionRequest {
        session_id: Some("sess-1".into()),
        client_key: "key".into(),
        name: None,
        runtime: None,
    };
    let err = activate_session_request_from_proto(&proto).unwrap_err();
    assert_eq!(err.code, "missing_runtime");
    assert_eq!(err.message, "ActivateSessionRequest.runtime is required");
}

#[test]
fn activate_session_response_maps_session_and_created() {
    let response = crate::wire::WireActivateSessionResponse {
        session: Some(fixture_summary()),
        created: true,
    };
    let proto = activate_session_response_to_proto(&response);
    let session = proto.session.as_ref().unwrap();
    assert!(proto.created);
    assert_eq!(session.session_id, "sess-1");
    assert_eq!(session.name, "main");
    assert_eq!(session.model, "anthropic:claude-x");
    assert_eq!(session.preview.as_deref(), Some("last prompt"));
    let decoded = wire::ActivateSessionResponse::decode(proto.encode_to_vec().as_slice()).unwrap();
    assert!(decoded.created);
    assert_eq!(decoded.session.as_ref().unwrap().session_id, "sess-1");
}

#[test]
fn set_credential_converts_secret_without_encoded_response_exposure() {
    const SENTINEL: &[u8] = b"super-secret-sentinel";
    let proto = wire::SetCredentialRequest {
        session_id: "sess-1".into(),
        provider: "anthropic".into(),
        secret: SENTINEL.to_vec(),
    };
    let request = set_credential_request_from_proto(&proto);
    assert_eq!(request.session_id, "sess-1");
    assert_eq!(request.provider, "anthropic");
    assert_eq!(request.secret, SENTINEL);
    let response = wire::CommandResult { accepted: true };
    let encoded = response.encode_to_vec();
    assert!(
        !encoded
            .windows(SENTINEL.len())
            .any(|window| window == SENTINEL)
    );
}

#[test]
fn clear_credential_preserves_optional_provider_clear_all_semantics() {
    let clear_all = wire::ClearCredentialRequest {
        session_id: "sess-1".into(),
        provider: None,
    };
    let request = clear_credential_request_from_proto(&clear_all);
    assert_eq!(request.session_id, "sess-1");
    assert!(request.provider.is_none());

    let clear_one = wire::ClearCredentialRequest {
        session_id: "sess-1".into(),
        provider: Some(String::new()),
    };
    let request = clear_credential_request_from_proto(&clear_one);
    assert_eq!(request.provider.as_deref(), Some(""));
}

#[test]
fn set_credential_request_debug_redacts_secret() {
    let request = crate::wire::WireSetCredentialRequest {
        session_id: "sess-1".into(),
        provider: "anthropic".into(),
        secret: b"super-secret-sentinel".to_vec(),
    };

    let debug = format!("{request:?}");

    assert!(
        !debug.contains("super-secret-sentinel"),
        "secret leaked into Debug output: {debug}"
    );
    assert!(debug.contains("sess-1"));
    assert!(debug.contains("anthropic"));
    assert!(debug.contains("<redacted>"));
}

#[test]
fn set_credential_request_from_proto_keeps_metadata_and_secret() {
    let proto = wire::SetCredentialRequest {
        session_id: "sess-9".into(),
        provider: "openai".into(),
        secret: b"opaque".to_vec(),
    };

    let request = set_credential_request_from_proto(&proto);

    assert_eq!(request.session_id, "sess-9");
    assert_eq!(request.provider, "openai");
    assert_eq!(request.secret, b"opaque");
}

#[derive(Clone, PartialEq, ::prost::Message)]
struct LegacyStreamEvent {
    #[prost(oneof = "legacy_stream_event::Kind", tags = "1")]
    kind: Option<legacy_stream_event::Kind>,
}

mod legacy_stream_event {
    #[derive(Clone, PartialEq, ::prost::Message)]
    pub struct SubagentStarted {
        #[prost(string, tag = "1")]
        pub id: String,
        #[prost(string, tag = "2")]
        pub agent: String,
        #[prost(string, tag = "3")]
        pub source: String,
    }

    #[derive(Clone, PartialEq, ::prost::Oneof)]
    pub enum Kind {
        #[prost(message, tag = "1")]
        SubagentStarted(SubagentStarted),
    }
}

#[test]
fn legacy_stream_event_decoder_ignores_session_id_tag() {
    let event = wire::StreamEvent {
        session_id: "sess-9".into(),
        kind: Some(wire::stream_event::Kind::SubagentStarted(
            wire::SubagentStarted {
                id: "job-1".into(),
                agent: "researcher".into(),
                source: "dag".into(),
                run_id: Some("run-1".into()),
                node_id: Some("node-1".into()),
            },
        )),
    };
    let encoded = event.encode_to_vec();
    assert!(encoded.windows(2).any(|window| window == [0x3a, 0x06]));
    let legacy = LegacyStreamEvent::decode(encoded.as_slice()).unwrap();
    match legacy.kind {
        Some(legacy_stream_event::Kind::SubagentStarted(started)) => {
            assert_eq!(started.id, "job-1");
            assert_eq!(started.agent, "researcher");
            assert_eq!(started.source, "dag");
        }
        None => panic!("legacy decoder should preserve the known kind"),
    }
}
