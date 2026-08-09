//! End-to-end test for /bug-report. Builds a real bug-report file in a tempdir and asserts
//! that secrets seeded into the log are redacted in the output.

use std::sync::Arc;

use tempfile::TempDir;
use theway_core::{
    AgentHarness, AgentHarnessOptions, MemorySessionStorage, Session, SessionStorage,
};
use theway_llm_provider::{
    AssistantMessage, AssistantMessageEvent, AssistantMessageEventStream, AssistantRole,
    ContentBlock, DoneReason, ModelCost, StopReason, Usage,
};

#[allow(dead_code)]
#[path = "../src/bug_report.rs"]
mod bug_report;
#[allow(dead_code)]
#[path = "../src/config.rs"]
mod config;
#[allow(dead_code)]
#[path = "../src/export.rs"]
mod export;

fn faux_model() -> theway_llm_provider::Model {
    theway_llm_provider::Model {
        id: "faux".into(),
        name: "Faux".into(),
        api: theway_llm_provider::Api::from("faux"),
        provider: theway_llm_provider::Provider::from("faux"),
        base_url: String::new(),
        reasoning: false,
        thinking_level_map: None,
        input: vec![],
        cost: ModelCost::default(),
        context_window: 0,
        max_tokens: 0,
        headers: None,
        compat: None,
    }
}

fn faux_stream(text: &'static str) -> theway_core::StreamFn {
    Arc::new(move |_, _, _| {
        let (stream, mut sender) = AssistantMessageEventStream::new();
        tokio::spawn(async move {
            let msg = AssistantMessage {
                role: AssistantRole::Assistant,
                content: vec![ContentBlock::text(text)],
                api: theway_llm_provider::Api::from("faux"),
                provider: theway_llm_provider::Provider::from("faux"),
                model: "faux".into(),
                response_model: None,
                response_id: None,
                diagnostics: None,
                usage: Usage::default(),
                stop_reason: StopReason::Stop,
                error_message: None,
                timestamp: 0,
            };
            sender.push(AssistantMessageEvent::Start {
                partial: msg.clone(),
            });
            sender.push(AssistantMessageEvent::Done {
                reason: DoneReason::Stop,
                message: msg,
            });
        });
        stream
    })
}

#[tokio::test]
async fn bug_report_redacts_secrets_from_seeded_log() {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let mut opts = AgentHarnessOptions::new(faux_model(), session.clone());
    opts.stream_fn = Some(faux_stream("ok"));
    let harness = AgentHarness::new(opts);
    harness.prompt("describe the system").await.unwrap();

    // Seed a fake log file containing several secret patterns.
    let dir = TempDir::new().unwrap();
    let log = dir.path().join("session.log");
    std::fs::write(
        &log,
        "2026-01-01 INFO outbound request key=sk-abcdefghij1234567890abcd\n\
         2026-01-01 INFO aws creds AKIAEXAMPLEEXAMPLE1A used\n\
         2026-01-01 INFO header: Authorization: Bearer eyJabc.defghijklmnopqr\n",
    )
    .unwrap();

    let diag = bug_report::DiagInputs {
        session_id: "test".into(),
        model: Some("faux:faux".into()),
        thinking: "off".into(),
        tool_count: 0,
        skill_count: 0,
        cost_summary: "n/a".into(),
        log_path: Some(log.clone()),
    };
    let dest = dir.path().join("report.txt");
    let written = bug_report::build(diag, &session, &dest).await.unwrap();
    assert_eq!(written, dest);

    let body = std::fs::read_to_string(&dest).unwrap();
    // Header and structure.
    assert!(body.contains("theway bug report"));
    assert!(body.contains("---- diagnostic ----"));
    assert!(body.contains("---- log tail"));
    assert!(body.contains("---- transcript ----"));

    // Secrets gone, redaction markers present.
    assert!(!body.contains("sk-abcdefghij"), "openai key leaked: {body}");
    assert!(!body.contains("AKIAEXAMPLE"), "aws key leaked: {body}");
    assert!(
        !body.contains("eyJabc.defghijklmnopqr"),
        "bearer leaked: {body}"
    );
    assert!(body.contains("[REDACTED:openai_anthropic_key]"));
    assert!(body.contains("[REDACTED:aws_access_key]"));
    assert!(body.contains("[REDACTED:bearer_token]"));
}
