use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use theway_llm_provider::{Message, Tool, ToolCall, UserContentBlock};
use tokio_util::sync::CancellationToken;

use super::*;

#[derive(Clone)]
struct InvocationRecord {
    event: ExtensionLifecycleEvent,
    class: ExtensionHookClass,
    message_id: Option<String>,
    tool_call_id: Option<String>,
    payload: serde_json::Value,
}

type Handler = Arc<dyn Fn(&RuntimeExtensionInvocation) -> ExtensionActionBatch + Send + Sync>;

struct ScriptedPort {
    records: Mutex<Vec<InvocationRecord>>,
    handler: Handler,
}

impl ScriptedPort {
    fn new(handler: impl Fn(&RuntimeExtensionInvocation) -> ExtensionActionBatch + Send + Sync + 'static) -> Self {
        Self {
            records: Mutex::new(Vec::new()),
            handler: Arc::new(handler),
        }
    }

    fn invoke(&self, invocation: RuntimeExtensionInvocation) -> RawRuntimeExtensionResult {
        self.records.lock().push(InvocationRecord {
            event: invocation.event(),
            class: invocation.class(),
            message_id: invocation.context().scope.message_id.clone(),
            tool_call_id: invocation.context().scope.tool_call_id.clone(),
            payload: invocation.payload().clone(),
        });
        Ok((self.handler)(&invocation))
    }

    fn records(&self) -> Vec<InvocationRecord> {
        self.records.lock().clone()
    }
}

macro_rules! impl_scripted_port {
    ($trait_name:ident, $method:ident) => {
        #[async_trait]
        impl $trait_name for ScriptedPort {
            async fn $method(
                &self,
                invocation: RuntimeExtensionInvocation,
            ) -> RawRuntimeExtensionResult {
                self.invoke(invocation)
            }
        }
    };
}

impl_scripted_port!(RuntimeSessionExtensionPort, invoke_session);
impl_scripted_port!(RuntimeRunExtensionPort, invoke_run);
impl_scripted_port!(RuntimeRequestExtensionPort, invoke_request);
impl_scripted_port!(RuntimeMessageExtensionPort, invoke_message);
impl_scripted_port!(RuntimeToolExtensionPort, invoke_tool);
impl_scripted_port!(RuntimeCompactionExtensionPort, invoke_compaction);

struct RecordingTool {
    definition: Tool,
    calls: Arc<AtomicUsize>,
    delay: Duration,
    update: bool,
}

impl RecordingTool {
    fn new(name: &str, calls: Arc<AtomicUsize>, delay: Duration, update: bool) -> Self {
        Self {
            definition: Tool {
                name: name.into(),
                description: format!("{name} test tool"),
                parameters: serde_json::json!({"type": "object"}),
            },
            calls,
            delay,
            update,
        }
    }
}

#[async_trait]
impl AgentTool for RecordingTool {
    fn definition(&self) -> &Tool {
        &self.definition
    }

    fn label(&self) -> &str {
        &self.definition.name
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        _params: serde_json::Value,
        _cancel: CancellationToken,
        on_update: Option<AgentToolUpdate>,
    ) -> Result<AgentToolResult, AgentToolError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        if self.update {
            if let Some(update) = on_update {
                update(AgentToolResult {
                    content: vec![UserContentBlock::text("progress")],
                    ..Default::default()
                });
            }
        }
        tokio::time::sleep(self.delay).await;
        Ok(AgentToolResult {
            content: vec![UserContentBlock::text(format!(
                "raw-{}",
                self.definition.name
            ))],
            ..Default::default()
        })
    }
}

fn scripted_harness(
    port: Arc<ScriptedPort>,
    stream_fn: StreamFn,
    session: Session,
    tools: Vec<Arc<dyn AgentTool>>,
    before_tool_call: Option<BeforeToolCallHook>,
) -> AgentHarness {
    let mut options = AgentHarnessOptions::new(faux_model(), session);
    options.observation_context.session_id = Some("session-message-tool".into());
    options.runtime_extension_cwd = "/workspace".into();
    options.runtime_extensions = port;
    options.stream_fn = Some(stream_fn);
    options.tools = tools;
    options.before_tool_call = before_tool_call;
    AgentHarness::new(options)
}

fn assistant_with_calls(calls: Vec<ToolCall>) -> AssistantMessage {
    let mut message = assistant("");
    message.content = calls.into_iter().map(ContentBlock::ToolCall).collect();
    message.stop_reason = StopReason::ToolUse;
    message
}

fn tool_call(id: &str, name: &str) -> ToolCall {
    ToolCall {
        id: id.into(),
        name: name.into(),
        arguments: serde_json::Map::new(),
        thought_signature: None,
    }
}

fn assistant_text(message: &AgentMessage) -> Option<String> {
    let AgentMessage::Llm(Message::Assistant(message)) = message else {
        return None;
    };
    message.content.iter().find_map(|block| match block {
        ContentBlock::Text(text) => Some(text.text.clone()),
        _ => None,
    })
}

#[tokio::test]
async fn message_lifecycle_uses_stable_ids_and_final_transform_persists() {
    let replacement = AgentMessage::Llm(Message::Assistant(assistant("rewritten")));
    let port = Arc::new(ScriptedPort::new(move |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::MessageEnd
            && invocation.class() == ExtensionHookClass::Transform
            && invocation.payload()["message"]["role"] == "assistant"
        {
            return ExtensionActionBatch {
                abi_major: ExtensionAbiMajor::V2,
                decision: None,
                actions: vec![
                    ExtensionAction {
                        kind: ExtensionActionKind::ReplaceMessage,
                        payload: serde_json::json!({"message": replacement}),
                    },
                    ExtensionAction {
                        kind: ExtensionActionKind::SetState,
                        payload: serde_json::json!({
                            "abiMajor": 2,
                            "extensionId": "test-extension",
                            "stateSchemaVersion": 1,
                            "originSequence": 1,
                            "entry": {
                                "kind": "state_mutation",
                                "key": "message-phase",
                                "mutation": {"operation": "set", "value": "complete"},
                            },
                        }),
                    },
                ],
            };
        }
        empty_batch()
    }));
    let stream_fn: StreamFn = Arc::new(move |_, _, _| {
        let (stream, mut sender) = AssistantMessageEventStream::new();
        sender.push(AssistantMessageEvent::Start {
            partial: assistant(""),
        });
        sender.push(AssistantMessageEvent::TextDelta {
            content_index: 0,
            delta: "raw".into(),
            partial: assistant("raw"),
        });
        sender.push(AssistantMessageEvent::Done {
            reason: DoneReason::Stop,
            message: assistant("raw"),
        });
        stream
    });
    let session = Session::new(Arc::new(MemorySessionStorage::new()));
    let harness = scripted_harness(port.clone(), stream_fn, session.clone(), Vec::new(), None);

    harness.prompt("hello").await.unwrap();

    assert_eq!(
        assistant_text(harness.agent().state().messages.last().unwrap()),
        Some("rewritten".into())
    );
    assert_eq!(
        assistant_text(session.build_context().await.unwrap().messages.last().unwrap()),
        Some("rewritten".into())
    );
    let messages = port
        .records()
        .into_iter()
        .filter(|record| {
            matches!(
                record.event,
                ExtensionLifecycleEvent::MessageStart
                    | ExtensionLifecycleEvent::MessageUpdate
                    | ExtensionLifecycleEvent::MessageEnd
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(messages.len(), 5);
    assert_eq!(messages[0].class, ExtensionHookClass::Observe);
    assert_eq!(messages[1].class, ExtensionHookClass::Transform);
    assert_eq!(messages[0].message_id, messages[1].message_id);
    assert_eq!(messages[2].message_id, messages[3].message_id);
    assert_eq!(messages[3].message_id, messages[4].message_id);
    assert_ne!(messages[0].message_id, messages[2].message_id);
}

#[tokio::test]
async fn transformed_assistant_drives_tool_extraction_and_full_tool_observation() {
    let transformed = AgentMessage::Llm(Message::Assistant(assistant_with_calls(vec![tool_call(
        "call-1", "echo",
    )])));
    let replaced = Arc::new(AtomicBool::new(false));
    let port = Arc::new(ScriptedPort::new({
        let replaced = Arc::clone(&replaced);
        move |invocation| {
            if invocation.event() == ExtensionLifecycleEvent::MessageEnd
                && invocation.class() == ExtensionHookClass::Transform
                && invocation.payload()["message"]["role"] == "assistant"
                && !replaced.swap(true, Ordering::AcqRel)
            {
                return ExtensionActionBatch {
                    abi_major: ExtensionAbiMajor::V2,
                    decision: None,
                    actions: vec![ExtensionAction {
                        kind: ExtensionActionKind::ReplaceMessage,
                        payload: serde_json::json!({"message": transformed}),
                    }],
                };
            }
            empty_batch()
        }
    }));
    let provider_calls = Arc::new(AtomicUsize::new(0));
    let stream_fn: StreamFn = {
        let provider_calls = Arc::clone(&provider_calls);
        Arc::new(move |_, _, _| {
            let (stream, mut sender) = AssistantMessageEventStream::new();
            let call = provider_calls.fetch_add(1, Ordering::Relaxed);
            let message = if call == 0 {
                assistant("raw stop")
            } else {
                assistant("finished")
            };
            sender.push(AssistantMessageEvent::Done {
                reason: DoneReason::Stop,
                message,
            });
            stream
        })
    };
    let tool_calls = Arc::new(AtomicUsize::new(0));
    let tool: Arc<dyn AgentTool> = Arc::new(RecordingTool::new(
        "echo",
        Arc::clone(&tool_calls),
        Duration::ZERO,
        true,
    ));
    let harness = scripted_harness(
        port.clone(),
        stream_fn,
        Session::new(Arc::new(MemorySessionStorage::new())),
        vec![tool],
        None,
    );

    harness.prompt("use transformed tool").await.unwrap();

    assert_eq!(provider_calls.load(Ordering::Relaxed), 2);
    assert_eq!(tool_calls.load(Ordering::Relaxed), 1);
    let tool_events = port
        .records()
        .into_iter()
        .filter(|record| {
            matches!(
                record.event,
                ExtensionLifecycleEvent::ToolCall
                    | ExtensionLifecycleEvent::ToolExecutionStart
                    | ExtensionLifecycleEvent::ToolExecutionUpdate
                    | ExtensionLifecycleEvent::ToolExecutionEnd
                    | ExtensionLifecycleEvent::ToolResult
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        tool_events.iter().map(|record| record.event).collect::<Vec<_>>(),
        vec![
            ExtensionLifecycleEvent::ToolCall,
            ExtensionLifecycleEvent::ToolExecutionStart,
            ExtensionLifecycleEvent::ToolExecutionUpdate,
            ExtensionLifecycleEvent::ToolExecutionEnd,
            ExtensionLifecycleEvent::ToolResult,
        ]
    );
    assert!(tool_events
        .iter()
        .all(|record| record.tool_call_id.as_deref() == Some("call-1")));
}

#[tokio::test]
async fn first_extension_gate_denial_skips_later_gate_and_execution() {
    let port = Arc::new(ScriptedPort::new(|invocation| {
        if invocation.event() == ExtensionLifecycleEvent::ToolCall {
            return ExtensionActionBatch {
                abi_major: ExtensionAbiMajor::V2,
                decision: Some(ExtensionGateDecision::Deny {
                    code: "policy_denied".into(),
                    message: "blocked by extension".into(),
                }),
                actions: Vec::new(),
            };
        }
        empty_batch()
    }));
    let provider_calls = Arc::new(AtomicUsize::new(0));
    let second_context = Arc::new(Mutex::new(None));
    let stream_fn: StreamFn = {
        let provider_calls = Arc::clone(&provider_calls);
        let second_context = Arc::clone(&second_context);
        Arc::new(move |_, context, _| {
            let (stream, mut sender) = AssistantMessageEventStream::new();
            let call = provider_calls.fetch_add(1, Ordering::Relaxed);
            let message = if call == 0 {
                assistant_with_calls(vec![tool_call("denied-1", "echo")])
            } else {
                *second_context.lock() = serde_json::to_value(&context.messages).ok();
                assistant("finished")
            };
            sender.push(AssistantMessageEvent::Done {
                reason: if call == 0 {
                    DoneReason::ToolUse
                } else {
                    DoneReason::Stop
                },
                message,
            });
            stream
        })
    };
    let later_gate_calls = Arc::new(AtomicUsize::new(0));
    let later_gate: BeforeToolCallHook = {
        let calls = Arc::clone(&later_gate_calls);
        Arc::new(move |_, _| {
            calls.fetch_add(1, Ordering::Relaxed);
            Box::pin(async { BeforeToolCallResult::default() })
        })
    };
    let tool_calls = Arc::new(AtomicUsize::new(0));
    let tool: Arc<dyn AgentTool> = Arc::new(RecordingTool::new(
        "echo",
        Arc::clone(&tool_calls),
        Duration::ZERO,
        false,
    ));
    let harness = scripted_harness(
        port.clone(),
        stream_fn,
        Session::new(Arc::new(MemorySessionStorage::new())),
        vec![tool],
        Some(later_gate),
    );

    harness.prompt("deny tool").await.unwrap();

    assert_eq!(tool_calls.load(Ordering::Relaxed), 0);
    assert_eq!(later_gate_calls.load(Ordering::Relaxed), 0);
    assert!(second_context
        .lock()
        .as_ref()
        .unwrap()
        .to_string()
        .contains("extension tool gate denied [policy_denied]"));
    let events = port
        .records()
        .into_iter()
        .filter(|record| {
            matches!(
                record.event,
                ExtensionLifecycleEvent::ToolCall
                    | ExtensionLifecycleEvent::ToolExecutionStart
                    | ExtensionLifecycleEvent::ToolExecutionEnd
                    | ExtensionLifecycleEvent::ToolResult
            )
        })
        .map(|record| record.event)
        .collect::<Vec<_>>();
    assert_eq!(
        events,
        vec![
            ExtensionLifecycleEvent::ToolCall,
            ExtensionLifecycleEvent::ToolResult,
        ]
    );
}

#[tokio::test]
async fn parallel_preflight_and_finalized_results_preserve_assistant_source_order() {
    let port = Arc::new(ScriptedPort::new(|invocation| {
        if invocation.event() == ExtensionLifecycleEvent::ToolResult {
            let name = invocation.payload()["toolCall"]["name"]
                .as_str()
                .unwrap_or("unknown");
            return ExtensionActionBatch {
                abi_major: ExtensionAbiMajor::V2,
                decision: None,
                actions: vec![ExtensionAction {
                    kind: ExtensionActionKind::ReplaceToolResult,
                    payload: serde_json::json!({
                        "result": {
                            "content": [{"type": "text", "text": format!("rewritten-{name}")}],
                            "details": null,
                        },
                        "isError": false,
                    }),
                }],
            };
        }
        empty_batch()
    }));
    let provider_calls = Arc::new(AtomicUsize::new(0));
    let second_context = Arc::new(Mutex::new(None));
    let stream_fn: StreamFn = {
        let provider_calls = Arc::clone(&provider_calls);
        let second_context = Arc::clone(&second_context);
        Arc::new(move |_, context, _| {
            let (stream, mut sender) = AssistantMessageEventStream::new();
            let call = provider_calls.fetch_add(1, Ordering::Relaxed);
            let message = if call == 0 {
                assistant_with_calls(vec![
                    tool_call("source-1", "slow"),
                    tool_call("source-2", "fast"),
                ])
            } else {
                *second_context.lock() = serde_json::to_value(&context.messages).ok();
                assistant("finished")
            };
            sender.push(AssistantMessageEvent::Done {
                reason: if call == 0 {
                    DoneReason::ToolUse
                } else {
                    DoneReason::Stop
                },
                message,
            });
            stream
        })
    };
    let slow: Arc<dyn AgentTool> = Arc::new(RecordingTool::new(
        "slow",
        Arc::new(AtomicUsize::new(0)),
        Duration::from_millis(30),
        false,
    ));
    let fast: Arc<dyn AgentTool> = Arc::new(RecordingTool::new(
        "fast",
        Arc::new(AtomicUsize::new(0)),
        Duration::ZERO,
        false,
    ));
    let harness = scripted_harness(
        port.clone(),
        stream_fn,
        Session::new(Arc::new(MemorySessionStorage::new())),
        vec![slow, fast],
        None,
    );

    harness.prompt("parallel tools").await.unwrap();

    let gates = port
        .records()
        .into_iter()
        .filter(|record| record.event == ExtensionLifecycleEvent::ToolCall)
        .map(|record| record.payload["toolCall"]["name"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert_eq!(gates, vec!["slow", "fast"]);
    let context = second_context.lock().clone().unwrap().to_string();
    assert!(context.find("rewritten-slow").unwrap() < context.find("rewritten-fast").unwrap());
}
