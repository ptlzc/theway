// ── branch-coverage gap fillers for turn/daemon ─────────────────────────────────
//
// These tests target specific LLVM branch records that the existing suites do
// not exercise. The queue/state/snapshot/input/commands/runtime submodules stay
// split by source domain and share the helpers (and the `FailingAppendStorage`
// test double) defined in the other `final_coverage` files.

use std::sync::Arc;

use async_trait::async_trait;
use theway_contract::session::{SessionBinding, SessionRuntimeContext};
use theway_core::{AgentToolError, AgentToolResult};
use theway_transport::wire::{WireActivateSessionRequest, WireSessionRuntimeContext};
use tokio_util::sync::CancellationToken;

use super::*;
use crate::mcp_loader::{McpProvisionState, ServerConfig, ServerKind};
use crate::orchestration::{SessionRuntime, SessionRuntimeBuilder};
use crate::runtime_storage::local_runtime_storage;
use crate::session_activation::SessionActivator;
use crate::turn::daemon::{SUPPORTED_APIS, TurnHost};

mod commands;
mod input;
mod queue;
mod runtime;
mod snapshot;
mod state;

// ── small helpers ───────────────────────────────────────────────────────────────

struct DummyTool {
    def: theway_llm_provider::Tool,
}

#[async_trait]
impl theway_core::AgentTool for DummyTool {
    fn definition(&self) -> &theway_llm_provider::Tool {
        &self.def
    }

    fn label(&self) -> &str {
        "dummy"
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        _params: serde_json::Value,
        _cancel: CancellationToken,
        _on_update: Option<theway_core::AgentToolUpdate>,
    ) -> Result<AgentToolResult, AgentToolError> {
        Ok(AgentToolResult::default())
    }
}

fn dummy_tool() -> Arc<dyn theway_core::AgentTool> {
    Arc::new(DummyTool {
        def: theway_llm_provider::Tool {
            name: "dummy".into(),
            description: "dummy tool".into(),
            parameters: serde_json::json!({ "type": "object" }),
        },
    })
}

fn branch_gap_faux_stream() -> theway_core::StreamFn {
    std::sync::Arc::new(|_, _, _| {
        let (stream, _sender) = theway_llm_provider::AssistantMessageEventStream::new();
        stream
    })
}

#[allow(dead_code)]
fn real_session_factory() -> SessionFactory {
    Arc::new(
        |id: String| -> std::pin::Pin<
            Box<
                dyn std::future::Future<Output = anyhow::Result<SessionRuntime>> + Send,
            >,
        > {
            Box::pin(async move {
                let harness = harness_with_input(Vec::new());
                Ok(SessionRuntime::for_test(id, harness))
            })
        },
    )
}

fn same_cwd_session_factory(cwd: std::path::PathBuf) -> SessionFactory {
    Arc::new(
        move |id: String| -> std::pin::Pin<
            Box<
                dyn std::future::Future<Output = anyhow::Result<SessionRuntime>> + Send,
            >,
        > {
            let cwd = cwd.clone();
            Box::pin(async move {
                let harness = harness_with_input(Vec::new());
                let mut runtime = SessionRuntime::for_test(id, harness);
                runtime.cwd = cwd.clone();
                Ok(runtime)
            })
        },
    )
}

fn register_session_binding(host: &mut TurnHost, id: &str) {
    let work = host.runtime.cwd.clone();
    std::fs::create_dir_all(&work).unwrap();
    let work = work.canonicalize().unwrap();
    host.automation
        .services
        .session_execution
        .set(
            id,
            SessionBinding {
                client_key: "client-1".into(),
                runtime: SessionRuntimeContext {
                    work_dir: work.display().to_string(),
                    provider: None,
                    model: None,
                    base_url: None,
                    thinking: None,
                },
            },
        )
        .unwrap();
}

fn install_activator_after_host(host: &mut TurnHost) -> Arc<SessionRuntimeBuilder> {
    let (main_run_tx, main_run_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    // Keep the receiver alive for the builder's lifetime; runtime assembly may
    // send main-run triggers later.
    std::mem::forget(main_run_rx);
    let builder = Arc::new(SessionRuntimeBuilder {
        thinking: theway_core::ThinkingLevel::High,
        stream_fn: branch_gap_faux_stream(),
        dag_engine: host.automation.dag.clone(),
        subagent_registry: host.automation.subagents.clone(),
        services: host.automation.services.clone(),
        before_tool_call: None,
        control_plane_hook: None,
        control_plane_prompt_tx: None,
        after_tool_call: None,
        feed_tx: host.inputs.feed_tx.clone(),
        main_run_tx,
        debug: false,
        session_cells: Default::default(),
    });
    let activator = SessionActivator::new(
        &builder,
        local_runtime_storage(),
        host.runtime.paths.clone(),
        theway_core::ThinkingLevel::High,
        Vec::new(),
        Vec::new(),
        false,
    );
    let _ = host
        .automation
        .services
        .session_activator
        .set(Arc::new(activator));
    builder
}

async fn activate_turn_with_future(
    host: &mut TurnHost,
    work_dir: &std::path::Path,
) -> TurnState {
    let model = theway_llm_provider::list_models()
        .into_iter()
        .find(|m| SUPPORTED_APIS.contains(&m.api.0.as_str()))
        .expect("a supported model should exist in the catalog");
    let request = WireActivateSessionRequest {
        session_id: None,
        client_key: "branch-gap-client".into(),
        name: Some("branch-gap".into()),
        runtime: Some(WireSessionRuntimeContext {
            work_dir: work_dir.display().to_string(),
            provider: Some(model.provider.0.clone()),
            model: Some(model.id.clone()),
            base_url: None,
            thinking: Some(false),
        }),
        mcp_servers: Vec::new(),
    };
    let mut turn = sample_turn_with_future();
    // `handle_activate_session` awaits `apply_activation`, which aborts the
    // in-flight turn and takes its future.
    host.handle_web_command(
        WireCommand::ActivateSession {
            request,
            response: tokio::sync::oneshot::channel().0,
        },
        &mut turn,
    )
    .await;
    turn
}

fn mcp_provision_with_configs() -> McpProvisionState {
    let mut slot = McpProvisionState::default();
    slot.configs.push(ServerConfig {
        name: "slot-config".into(),
        kind: ServerKind::Stdio,
        command: Some("thewayd".into()),
        args: Vec::new(),
        endpoint: None,
        auth: None,
        request_timeout_ms: None,
        sse_idle_timeout_ms: None,
        body_cap_bytes: None,
        reconnect: None,
        inject_summary: false,
        inject_and_run: false,
    });
    slot
}
