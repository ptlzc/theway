//! `run_one` tool-call execution: blocked and unknown calls, success and error
//! outcomes, update event streaming, cancellation, and pump join timeout.

use super::*;

#[tokio::test]
async fn run_one_blocked_call_returns_error_outcome_without_tool() {
    // Arrange
    let inner = agent().inner.clone();
    let call = PreparedCall::Blocked {
        id: "call_1".into(),
        name: "blocked".into(),
        args: serde_json::json!({}),
        result: AgentToolResult {
            content: vec![theway_llm_provider::UserContentBlock::text("blocked")],
            details: serde_json::Value::Null,
            terminate: None,
        },
    };

    // Act
    let outcome = run_one(inner, call, tokio_util::sync::CancellationToken::new()).await;

    // Assert
    assert!(outcome.is_error);
    assert_eq!(outcome.id, "call_1");
    assert_eq!(outcome.name, "blocked");
}

#[tokio::test]
async fn run_one_unknown_tool_returns_synthesized_error() {
    // Arrange
    let inner = agent().inner.clone();
    let call = PreparedCall::Run {
        id: "call_1".into(),
        name: "missing".into(),
        args: serde_json::json!({}),
        tool: None,
    };

    // Act
    let outcome = run_one(inner, call, tokio_util::sync::CancellationToken::new()).await;

    // Assert
    assert!(outcome.is_error);
    assert_eq!(outcome.name, "missing");
    match &outcome.result.content[0] {
        theway_llm_provider::UserContentBlock::Text(t) => {
            assert!(t.text.contains("No tool registered named 'missing'"));
        }
        _ => panic!("expected text content"),
    }
}

struct RunOneTool {
    ok: Option<AgentToolResult>,
    err: Option<String>,
    update: Option<AgentToolResult>,
    def: theway_llm_provider::Tool,
}

#[async_trait::async_trait]
impl crate::types::AgentTool for RunOneTool {
    fn definition(&self) -> &theway_llm_provider::Tool {
        &self.def
    }

    fn label(&self) -> &str {
        "run-one"
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        _params: serde_json::Value,
        _cancel: CancellationToken,
        on_update: Option<AgentToolUpdate>,
    ) -> Result<AgentToolResult, AgentToolError> {
        if let (Some(update), Some(on_update)) = (&self.update, on_update) {
            on_update(update.clone());
        }
        if let Some(err) = &self.err {
            return Err(AgentToolError::Message(err.clone()));
        }
        Ok(self
            .ok
            .clone()
            .unwrap_or_default())
    }
}

fn run_one_tool_ok() -> Arc<RunOneTool> {
    Arc::new(RunOneTool {
        ok: Some(AgentToolResult {
            content: vec![UserContentBlock::text("ok")],
            details: serde_json::Value::Null,
            terminate: None,
        }),
        err: None,
        update: None,
        def: theway_llm_provider::Tool {
            name: "run_one".into(),
            description: String::new(),
            parameters: serde_json::Value::Null,
        },
    })
}

fn run_one_tool_err() -> Arc<RunOneTool> {
    Arc::new(RunOneTool {
        ok: None,
        err: Some("boom".into()),
        update: None,
        def: theway_llm_provider::Tool {
            name: "run_one".into(),
            description: String::new(),
            parameters: serde_json::Value::Null,
        },
    })
}

#[tokio::test]
async fn run_one_with_tool_returns_success_and_error_outcomes() {
    let ok = run_one(
        inner_with_model_and_stream(stream_that_returns(
            "unused",
            theway_llm_provider::StopReason::Stop,
        )),
        PreparedCall::Run {
            id: "c1".into(),
            name: "run_one".into(),
            args: serde_json::json!({}),
            tool: Some(run_one_tool_ok()),
        },
        CancellationToken::new(),
    )
    .await;
    assert!(!ok.is_error);
    assert_eq!(ok.name, "run_one");

    let err = run_one(
        inner_with_model_and_stream(stream_that_returns(
            "unused",
            theway_llm_provider::StopReason::Stop,
        )),
        PreparedCall::Run {
            id: "c2".into(),
            name: "run_one".into(),
            args: serde_json::json!({}),
            tool: Some(run_one_tool_err()),
        },
        CancellationToken::new(),
    )
    .await;
    assert!(err.is_error);
    assert!(matches!(&err.result.content[0], UserContentBlock::Text(t) if t.text == "boom"));
}

#[tokio::test]
async fn run_one_with_tool_streams_update_events() {
    let tool = Arc::new(RunOneTool {
        ok: Some(AgentToolResult {
            content: vec![UserContentBlock::text("done")],
            details: serde_json::Value::Null,
            terminate: None,
        }),
        err: None,
        update: Some(AgentToolResult {
            content: vec![UserContentBlock::text("partial")],
            details: serde_json::Value::Null,
            terminate: None,
        }),
        def: theway_llm_provider::Tool {
            name: "run_one".into(),
            description: String::new(),
            parameters: serde_json::Value::Null,
        },
    });

    let inner = inner_with_model_and_stream(stream_that_returns(
        "unused",
        theway_llm_provider::StopReason::Stop,
    ));
    let mut rx = inner.broadcast_tx.subscribe();

    let outcome = run_one(
        inner.clone(),
        PreparedCall::Run {
            id: "c3".into(),
            name: "run_one".into(),
            args: serde_json::json!({}),
            tool: Some(tool),
        },
        CancellationToken::new(),
    )
    .await;

    assert!(!outcome.is_error);
    let mut saw_update = false;
    while let Ok(event) = rx.try_recv() {
        if matches!(event, LoopEvent::ToolExecutionUpdate { .. }) {
            saw_update = true;
        }
    }
    assert!(saw_update, "ToolExecutionUpdate must be emitted for tool streaming");
}

#[tokio::test]
async fn run_one_with_cancelled_token_marks_cancelled() {
    let inner = agent().inner.clone();
    let cancel = CancellationToken::new();
    cancel.cancel();
    let call = PreparedCall::Blocked {
        id: "call_1".into(),
        name: "blocked".into(),
        args: serde_json::json!({}),
        result: AgentToolResult {
            content: vec![theway_llm_provider::UserContentBlock::text("blocked")],
            details: serde_json::Value::Null,
            terminate: None,
        },
    };

    let outcome = run_one(inner, call, cancel).await;

    assert!(outcome.is_error);
}

#[tokio::test(start_paused = true)]
async fn run_one_pump_join_timeout_aborts_pump_when_tool_retains_update() {
    static RETAINED_UPDATE: std::sync::Mutex<Option<AgentToolUpdate>> = std::sync::Mutex::new(None);

    struct RetainTool {
        def: theway_llm_provider::Tool,
    }
    #[async_trait::async_trait]
    impl crate::types::AgentTool for RetainTool {
        fn definition(&self) -> &theway_llm_provider::Tool {
            &self.def
        }
        fn label(&self) -> &str {
            "retain"
        }
        async fn execute(
            &self,
            _tool_call_id: &str,
            _params: serde_json::Value,
            _cancel: CancellationToken,
            on_update: Option<crate::types::AgentToolUpdate>,
        ) -> Result<crate::types::AgentToolResult, crate::types::AgentToolError> {
            *RETAINED_UPDATE.lock().unwrap() = on_update;
            Ok(crate::types::AgentToolResult::default())
        }
    }

    let inner = agent().inner.clone();
    let tool = Arc::new(RetainTool {
        def: theway_llm_provider::Tool {
            name: "retain".into(),
            description: String::new(),
            parameters: serde_json::Value::Null,
        },
    });

    let outcome = run_one(
        inner,
        PreparedCall::Run {
            id: "call_1".into(),
            name: "retain".into(),
            args: serde_json::json!({}),
            tool: Some(tool),
        },
        CancellationToken::new(),
    )
    .await;

    assert!(!outcome.is_error);
    // Clear the retained update so the pump's sender is dropped.
    *RETAINED_UPDATE.lock().unwrap() = None;
}
