//! End-to-end test for the session transcript exporter (used by `/save`). Runs the
//! AgentHarness against a faux model, prompts twice, exports the active branch to a
//! Markdown file in a tempdir, then asserts the file contains the prompts + assistant
//! replies in order.

use std::sync::Arc;

use tempfile::TempDir;
use theway_core::{
    AgentHarness, AgentHarnessOptions, MemorySessionStorage, Session, SessionStorage, StreamFn,
};
use theway_llm_provider::{
    AssistantMessage, AssistantMessageEvent, AssistantMessageEventStream, AssistantRole,
    ContentBlock, DoneReason, ModelCost, StopReason, Usage,
};

#[allow(dead_code)]
mod config {
    #[allow(unused_imports)]
    pub use theway_transport::config::*;
}
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

fn faux_stream(text: &'static str) -> StreamFn {
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
async fn save_writes_markdown_transcript_with_prompts_and_replies_in_order() {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let mut opts = AgentHarnessOptions::new(faux_model(), session.clone());
    opts.stream_fn = Some(faux_stream("first ack"));
    let harness = AgentHarness::new(opts);
    harness.prompt("first question").await.unwrap();

    // Switch stream content for the second turn by rebuilding harness; cheaper than juggling
    // a shared interior mutability. Re-wires same Session so transcript is continuous.
    let mut opts2 = AgentHarnessOptions::new(faux_model(), session.clone());
    opts2.stream_fn = Some(faux_stream("second ack"));
    let harness2 = AgentHarness::new(opts2);
    harness2.prompt("second question").await.unwrap();

    let outdir = TempDir::new().unwrap();
    let dest = outdir.path().join("transcript.md");
    let written = export::save(&session, &dest).await.unwrap();
    assert_eq!(written, dest);

    let body = std::fs::read_to_string(&dest).unwrap();
    let pos_q1 = body.find("first question").expect("q1 present");
    let pos_r1 = body.find("first ack").expect("r1 present");
    let pos_q2 = body.find("second question").expect("q2 present");
    let pos_r2 = body.find("second ack").expect("r2 present");
    assert!(
        pos_q1 < pos_r1 && pos_r1 < pos_q2 && pos_q2 < pos_r2,
        "order: {body}"
    );
    assert!(body.contains("# Session Transcript"));
}
