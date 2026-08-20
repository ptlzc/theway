//! `AgentHarness` — opinionated assembly around the bare [`Agent`].
//!
//! The directory is split by domain: catalog mutation, lifecycle events, session
//! persistence, and prompt execution. This root module owns the composed types and
//! constructor so the public harness API remains one unit.
//!
//! ## Event planes
//!
//! - [`LoopEvent`](crate::agent::LoopEvent) covers single turn-loop internals.
//! - [`SessionEvent`] covers cross-turn session lifecycle and configuration changes.
//! - `SubagentJobEvent` covers multi-agent graph job telemetry through `SubagentJobRegistry`.
//!
//! External consumers subscribe through [`AgentHarness::subscribe_session_broadcast`].
//! Synchronous callbacks registered with [`AgentHarness::subscribe_harness`] are isolated
//! with `catch_unwind` and must remain memory-only. Persistence happens at its explicit
//! call sites before matching session events are broadcast.

mod catalog;
mod events;
mod run;
mod runtime_extensions;
mod session;

use std::sync::Arc;

use parking_lot::Mutex;
use theway_llm_provider::Model;
use tokio::sync::broadcast;

use crate::agent::session::session::Session;
use crate::agent::{Agent, AgentOptions, LoopListener};
use crate::observability::{
    ObservationContext, OperationId, RuntimeObserver, noop_runtime_observer,
};
use crate::types::*;

use self::catalog::build_system_prompt;
use super::compaction::algorithm::CompactAlgorithmRegistry;
use super::compaction::compaction::{CompactionSettings, DEFAULT_COMPACTION_SETTINGS};
use super::cost::{CostSnapshot, CostTracker};
use super::runtime_extensions::{
    ExtensionModelContextProjection, NoopRuntimeExtensionPort, RuntimeExtensionPort,
};
use super::types::{PromptTemplate, Skill};

pub use self::events::{
    DEFAULT_TURN_CONTINUATION_CAP, OnTurnEndContext, OnTurnEndHook, SessionEvent, SessionListener,
    TurnEndAction, TurnEndDecision,
};

#[cfg(test)]
use self::run::{extract_user_message_text, extract_user_prompt_text, preview_for_banner};
#[cfg(test)]
use self::session::{cap_control_plane_audit_label, finish_persisted_run, make_session_listener};
#[cfg(test)]
use crate::agent::AgentRunError;
#[cfg(test)]
use crate::agent::session::session::BranchSummaryInput;

/// Capacity of the [`SessionEvent`] broadcast channel.
pub const SESSION_EVENT_BROADCAST_CAPACITY: usize = 128;

pub struct AgentHarnessOptions {
    /// Base system prompt prepended to the rendered skill catalog.
    pub system_prompt: String,
    pub model: Model,
    pub thinking_level: ThinkingLevel,
    pub skills: Vec<Skill>,
    pub prompt_templates: Vec<PromptTemplate>,
    pub tools: Vec<Arc<dyn AgentTool>>,
    pub session: Session,
    /// Embedder-owned runtime observer shared with the inner bare Agent.
    pub observer: Arc<dyn RuntimeObserver>,
    /// Session/run/job/node correlation inherited by inner operations.
    pub observation_context: ObservationContext,
    /// Optional parent operation for the inner agent run.
    pub observation_parent: Option<OperationId>,
    pub stream_fn: Option<StreamFn>,
    /// Auto-compaction thresholds. Defaults to [`DEFAULT_COMPACTION_SETTINGS`].
    pub compaction: CompactionSettings,
    /// Custom compaction algorithm registry. The builtin algorithm is always available.
    pub compact_algorithms: Arc<CompactAlgorithmRegistry>,
    /// Optional tool hooks supplied by the embedding runtime.
    pub before_tool_call: Option<BeforeToolCallHook>,
    pub after_tool_call: Option<AfterToolCallHook>,
    /// Control-plane prompt resolver. `None` keeps prompt-classified tool calls fail-closed.
    pub on_control_plane_prompt: Option<OnControlPlanePromptHook>,
    /// Per-session USD cap. `None` disables the check.
    pub budget_cap_usd: Option<f64>,
    /// Embedder-owned loader used by [`AgentHarness::reload_skills_from_disk`].
    pub reload_skills_fn: Option<ReloadSkillsFn>,
    /// Optional cross-prompt turn completion hook.
    pub on_turn_end: Option<OnTurnEndHook>,
    /// Maximum hook-driven continuations for one prompt cycle.
    pub turn_continuation_cap: Option<u32>,
    /// Hard cap on inner agent loop iterations. `None` is unbounded.
    pub max_iterations: Option<u32>,
    /// Engine-independent lifecycle port supplied by the embedding runtime.
    pub runtime_extensions: Arc<dyn RuntimeExtensionPort>,
    /// Reconstructed, de-duplicated persistent model context for this branch.
    pub runtime_extension_model_context: ExtensionModelContextProjection,
    /// Working directory included in runtime-extension lifecycle context.
    pub runtime_extension_cwd: String,
    /// Whether this harness is currently attached to an interactive client.
    pub runtime_extension_has_interactive_client: bool,
}

impl AgentHarnessOptions {
    pub fn new(model: Model, session: Session) -> Self {
        Self {
            system_prompt: String::new(),
            model,
            thinking_level: ThinkingLevel::Off,
            skills: Vec::new(),
            prompt_templates: Vec::new(),
            tools: Vec::new(),
            session,
            observer: noop_runtime_observer(),
            observation_context: ObservationContext::default(),
            observation_parent: None,
            stream_fn: None,
            compaction: DEFAULT_COMPACTION_SETTINGS.clone(),
            compact_algorithms: Arc::new(CompactAlgorithmRegistry::new()),
            before_tool_call: None,
            after_tool_call: None,
            on_control_plane_prompt: None,
            budget_cap_usd: None,
            reload_skills_fn: None,
            on_turn_end: None,
            turn_continuation_cap: None,
            max_iterations: None,
            runtime_extensions: Arc::new(NoopRuntimeExtensionPort),
            runtime_extension_model_context: ExtensionModelContextProjection::default(),
            runtime_extension_cwd: ".".into(),
            runtime_extension_has_interactive_client: false,
        }
    }
}

/// Async loader closure invoked by [`AgentHarness::reload_skills_from_disk`].
pub type ReloadSkillsFn = Arc<
    dyn Fn() -> std::pin::Pin<
            Box<dyn std::future::Future<Output = super::skills::LoadSkillsOutput> + Send>,
        > + Send
        + Sync,
>;

#[derive(Debug, thiserror::Error)]
pub enum ReloadSkillsError {
    #[error("reload_skills_fn was not configured at harness construction")]
    NotConfigured,
}

pub struct AgentHarness {
    /// Compaction triggers live in a sibling module and require crate-level access.
    pub(crate) agent: Arc<Agent>,
    pub(crate) session: Session,
    skills: Mutex<Vec<Skill>>,
    base_system_prompt: String,
    templates: Mutex<Vec<PromptTemplate>>,
    pub(crate) compaction_settings: Mutex<CompactionSettings>,
    pub(crate) compact_algorithms: Arc<CompactAlgorithmRegistry>,
    pub(crate) stream_fn: Option<StreamFn>,
    harness_listeners: Arc<Mutex<Vec<SessionListener>>>,
    session_broadcast_tx: broadcast::Sender<SessionEvent>,
    session_start_emitted: Mutex<bool>,
    cost: CostTracker,
    budget_cap_usd: Option<f64>,
    reload_skills_fn: Option<ReloadSkillsFn>,
    on_turn_end: Option<OnTurnEndHook>,
    turn_continuation_cap: u32,
    active_hook_cancel: Mutex<Option<tokio_util::sync::CancellationToken>>,
    runtime_extensions: Arc<runtime_extensions::HarnessRuntimeExtensions>,
}

impl AgentHarness {
    pub fn new(options: AgentHarnessOptions) -> Self {
        let session_id = options.observation_context.session_id.clone();
        let runtime_session_id = session_id
            .clone()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "local-session".into());
        let runtime_extensions = Arc::new(runtime_extensions::HarnessRuntimeExtensions::new(
            Arc::clone(&options.runtime_extensions),
            runtime_session_id,
            if options.runtime_extension_cwd.trim().is_empty() {
                ".".into()
            } else {
                options.runtime_extension_cwd.clone()
            },
            options.runtime_extension_has_interactive_client,
            Some(theway_contract::extension::ExtensionModelRef {
                provider: options.model.provider.0.clone(),
                model: options.model.id.clone(),
            }),
            options.runtime_extension_model_context.clone(),
        ));
        let state = AgentState {
            model: Some(options.model),
            thinking_level: Some(options.thinking_level),
            tools: options.tools,
            system_prompt: build_system_prompt(&options.system_prompt, &options.skills),
            ..Default::default()
        };

        let transform_runtime = Arc::clone(&runtime_extensions);
        let transform_context: TransformContext = Arc::new(move |messages, cancel| {
            let runtime = Arc::clone(&transform_runtime);
            Box::pin(async move { runtime.transform_context(messages, cancel).await })
        });

        let request_runtime = Arc::clone(&runtime_extensions);
        let transform_model_request: TransformModelRequest = Arc::new(move |request, cancel| {
            let runtime = Arc::clone(&request_runtime);
            Box::pin(async move {
                runtime
                    .before_model_request(request, u32::MAX, cancel)
                    .await
            })
        });

        let message_runtime = Arc::clone(&runtime_extensions);
        let transform_message: TransformMessage = Arc::new(move |message, cancel| {
            let runtime = Arc::clone(&message_runtime);
            Box::pin(async move { runtime.transform_message(message, cancel).await })
        });

        let configured_before_tool_call = options.before_tool_call.clone();
        let before_tool_runtime = Arc::clone(&runtime_extensions);
        let before_tool_call: BeforeToolCallHook = Arc::new(move |context, cancel| {
            let runtime = Arc::clone(&before_tool_runtime);
            let configured = configured_before_tool_call.clone();
            Box::pin(async move {
                let extension = runtime.before_tool_call(&context, &cancel).await;
                if extension.block {
                    return extension;
                }
                match configured {
                    Some(hook) => hook(context, cancel).await,
                    None => extension,
                }
            })
        });

        let after_tool_runtime = Arc::clone(&runtime_extensions);
        let transform_tool_result: AfterToolCallHook = Arc::new(move |context, cancel| {
            let runtime = Arc::clone(&after_tool_runtime);
            Box::pin(async move { runtime.transform_tool_result(&context, &cancel).await })
        });

        let agent = Agent::new(AgentOptions {
            initial_state: Some(state),
            transform_context: Some(transform_context),
            transform_model_request: Some(transform_model_request),
            transform_message: Some(transform_message),
            stream_fn: options.stream_fn.clone(),
            before_tool_call: Some(before_tool_call),
            after_tool_call: options.after_tool_call.clone(),
            transform_tool_result: Some(transform_tool_result),
            on_control_plane_prompt: options.on_control_plane_prompt.clone(),
            session_id,
            observer: Arc::clone(&options.observer),
            observation_context: options.observation_context.clone(),
            observation_parent: options.observation_parent,
            max_iterations: options.max_iterations,
            ..Default::default()
        });

        let cost = CostTracker::new();
        let _ = agent.subscribe_sync(cost.as_callback());
        let (session_broadcast_tx, _) = broadcast::channel(SESSION_EVENT_BROADCAST_CAPACITY);

        Self {
            agent: Arc::new(agent),
            session: options.session,
            skills: Mutex::new(options.skills),
            base_system_prompt: options.system_prompt,
            templates: Mutex::new(options.prompt_templates),
            compaction_settings: Mutex::new(options.compaction),
            compact_algorithms: options.compact_algorithms,
            stream_fn: options.stream_fn,
            harness_listeners: Arc::new(Mutex::new(Vec::new())),
            session_broadcast_tx,
            session_start_emitted: Mutex::new(false),
            cost,
            budget_cap_usd: options.budget_cap_usd,
            reload_skills_fn: options.reload_skills_fn,
            on_turn_end: options.on_turn_end,
            turn_continuation_cap: options
                .turn_continuation_cap
                .unwrap_or(DEFAULT_TURN_CONTINUATION_CAP),
            active_hook_cancel: Mutex::new(None),
            runtime_extensions,
        }
    }

    pub fn cost(&self) -> CostSnapshot {
        self.cost.snapshot()
    }

    pub fn reset_cost(&self) {
        self.cost.reset();
    }

    pub fn agent_arc(&self) -> Arc<Agent> {
        Arc::clone(&self.agent)
    }

    pub fn agent(&self) -> &Agent {
        &self.agent
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    pub fn runtime_extensions(&self) -> &Arc<dyn RuntimeExtensionPort> {
        self.runtime_extensions.port()
    }

    pub fn abort(&self) {
        self.agent.abort();
        if let Some(token) = self.active_hook_cancel.lock().as_ref() {
            token.cancel();
        }
    }

    pub fn interrupt(&self) {
        self.agent.interrupt();
    }

    pub fn enqueue_steering(&self, message: AgentMessage) {
        self.agent.enqueue_steering(message);
    }

    pub fn enqueue_follow_up(&self, message: AgentMessage) {
        self.agent.enqueue_follow_up(message);
    }

    pub fn subscribe(&self, listener: LoopListener) -> impl FnOnce() {
        self.agent.subscribe(listener)
    }
}

#[cfg(test)]
tests_bridge_macro::tests_bridge!("agent/assembly");

#[cfg(test)]
mod assembly_linecov_tests {
    tests_bridge_macro::tests_bridge!("agent/assembly/linecov");
}
