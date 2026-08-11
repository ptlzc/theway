//! End-to-end test for the spinner's integration with the agent's event stream.
//!
//! Drives a real AgentHarness + a faux StreamFn whose first text delta arrives only after
//! a deliberate delay. Subscribes the same kind of listener main.rs installs (calls
//! `stop_sync` on TextDelta / tool execution, but not ThinkingDelta). Captures the spinner's
//! stderr-equivalent via the BufferSink test hook. Asserts:
//!
//! - Frame 0 is in the captured buffer BEFORE the LLM event arrives (synchronous render).
//! - Multiple frames render during the delay (animation actually runs).
//! - On first text delta the `\r\x1b[2K` clear is appended.
//! - After stop, no further frames are written (animation task exits).

use std::sync::Arc;
use std::time::Duration;

use theway_core::{
    AgentHarness, AgentHarnessOptions, LoopEvent, MemorySessionStorage, Session, SessionStorage,
    StreamFn,
};
use theway_llm_provider::{
    AssistantMessage, AssistantMessageEvent, AssistantMessageEventStream, AssistantRole,
    ContentBlock, DoneReason, ModelCost, StopReason, Usage,
};

#[allow(dead_code)]
#[path = "../src/spinner.rs"]
mod spinner;

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

/// A faux stream that emits a thinking delta, then waits before emitting text + done.
/// Lets the spinner animate while the model is still thinking.
fn delayed_thinking_then_text_stream(
    thinking: &'static str,
    text: &'static str,
    text_delay_ms: u64,
) -> StreamFn {
    Arc::new(move |_, _, _| {
        let (stream, mut sender) = AssistantMessageEventStream::new();
        tokio::spawn(async move {
            let mut msg = AssistantMessage {
                role: AssistantRole::Assistant,
                content: vec![ContentBlock::Thinking(
                    theway_llm_provider::ThinkingContent {
                        thinking: String::new(),
                        thinking_signature: None,
                        redacted: false,
                    },
                )],
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
            msg.content = vec![ContentBlock::Thinking(
                theway_llm_provider::ThinkingContent {
                    thinking: thinking.to_string(),
                    thinking_signature: None,
                    redacted: false,
                },
            )];
            sender.push(AssistantMessageEvent::ThinkingDelta {
                content_index: 0,
                delta: thinking.to_string(),
                partial: msg.clone(),
            });

            tokio::time::sleep(Duration::from_millis(text_delay_ms)).await;
            msg.content = vec![ContentBlock::text(text)];
            sender.push(AssistantMessageEvent::TextDelta {
                content_index: 0,
                delta: text.to_string(),
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

/// Predicate matching what main.rs installs.
fn should_stop_on(ev: &LoopEvent) -> bool {
    use theway_llm_provider::AssistantMessageEvent;
    match ev {
        LoopEvent::ToolExecutionStart { .. } | LoopEvent::ToolExecutionEnd { .. } => true,
        LoopEvent::MessageUpdate {
            assistant_message_event,
            ..
        } => matches!(
            assistant_message_event,
            AssistantMessageEvent::TextDelta { .. } | AssistantMessageEvent::ToolCallDelta { .. }
        ),
        _ => false,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spinner_shows_during_thinking_then_clears_on_first_delta() {
    let sink = spinner::BufferSink::new();
    let spin = spinner::start_with(
        "thinking",
        Arc::new(sink.clone()) as Arc<dyn spinner::SpinnerSink>,
        true,
    );

    // Frame 0 must already be there before any agent event has fired.
    let initial = sink.as_string();
    assert!(
        initial.contains("⠋") && initial.contains("thinking"),
        "synchronous frame 0 missing: {initial:?}"
    );

    // Wire the agent + listener.
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let mut opts = AgentHarnessOptions::new(faux_model(), session);
    // Text arrives only after a thinking delta and a delay — enough for the spinner to
    // draw a few frames while the model is reasoning.
    opts.stream_fn = Some(delayed_thinking_then_text_stream(
        "considering",
        "hello",
        450,
    ));
    let harness = AgentHarness::new(opts);

    let spin_for_listener = spin.clone();
    let _unsub = harness.agent().subscribe(Arc::new(move |ev, _| {
        let s = spin_for_listener.clone();
        Box::pin(async move {
            if should_stop_on(&ev) {
                s.stop_sync();
            }
        })
    }));

    let prompt_fut = harness.prompt("hi");
    tokio::pin!(prompt_fut);

    tokio::select! {
        res = &mut prompt_fut => panic!("prompt completed before delayed text: {res:?}"),
        _ = tokio::time::sleep(Duration::from_millis(250)) => {}
    }

    let before_more_frames = sink.as_string().len();
    tokio::time::sleep(Duration::from_millis(120)).await;
    let after_more_frames = sink.as_string().len();
    assert!(
        after_more_frames > before_more_frames,
        "spinner stopped during thinking; len stayed at {before_more_frames}"
    );

    // Drive the prompt to completion. The first text delta should stop the spinner.
    prompt_fut.await.unwrap();

    // Inspect what was written to the spinner sink:
    let captured = sink.as_string();
    eprintln!("---captured---\n{captured:?}\n---end---");

    // 1. The synchronous clear from stop_sync must be in there.
    assert!(
        captured.contains("\r\x1b[2K"),
        "stop_sync clear escape missing: {captured:?}"
    );

    // 2. At least two distinct frame glyphs (so the animation actually ran for >1 tick).
    let distinct_frames: usize = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]
        .iter()
        .filter(|f| captured.contains(*f))
        .count();
    assert!(
        distinct_frames >= 2,
        "expected ≥2 distinct frames during the 350ms delay, got {distinct_frames}\n{captured:?}"
    );

    // 3. After the prompt completes, calling stop_sync again is a no-op.
    let before_double_stop = sink.as_string().len();
    spin.stop_sync();
    let after_double_stop = sink.as_string().len();
    assert_eq!(
        before_double_stop, after_double_stop,
        "stop_sync after listener-fired stop should be a no-op"
    );
}

/// Regression: stopping the spinner BEFORE the listener fires (e.g. error path) leaves
/// the buffer in a sane state — frame 0 + a clear — and the animation task exits without
/// adding more frames.
#[tokio::test]
async fn explicit_stop_works_without_any_agent_event() {
    let sink = spinner::BufferSink::new();
    let spin = spinner::start_with(
        "thinking",
        Arc::new(sink.clone()) as Arc<dyn spinner::SpinnerSink>,
        true,
    );

    // Don't wire any listener. Stop immediately.
    spin.stop_sync();

    let s = sink.as_string();
    assert!(s.contains("⠋"), "frame 0: {s:?}");
    assert!(s.ends_with("\r\x1b[2K"), "trailing clear: {s:?}");

    // Wait past frame interval — animation task must not append anything.
    let before = s.len();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let after = sink.as_string().len();
    assert_eq!(after, before, "animation task continued past stop");
}
