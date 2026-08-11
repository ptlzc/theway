//! `AgentHarness` — opinionated assembly around the bare `Agent`. 1:1 port of
//! `packages/agent/src/harness/agent-harness.ts` (~995 lines).
//!
//! Implemented:
//! - Compose `Agent` + `Session` + skills catalog + compaction settings
//! - `prompt(text)` / `prompt_with_images` / `continue_()`
//! - Auto-compaction trigger before each LLM call (when `compaction.enabled` is true)
//! - `set_model` / `set_thinking_level` mirror state mutations onto the session log
//! - `fork()` / `move_to()` branch operations (with optional branch summary)
//! - `prompt_from_template(name, vars)` — picks a `PromptTemplate`, interpolates, prompts
//! - `replace_tools` / `replace_skills` runtime mutations
//! - `enqueue_steering` / `enqueue_follow_up` queue passthrough
//! - `subscribe` to lifecycle events

use std::sync::Arc;

use parking_lot::Mutex;
use theway_llm_provider::{ImageContent, Message as PiMessage, Model};

use crate::agent::{Agent, AgentListener, AgentOptions, AgentRunError};
use crate::types::*;
// AfterToolCallHook is re-exported under types::* via `pub use` in the module; if it isn't
// directly visible here, fall back to the absolute path inside Agent::new.
#[allow(unused_imports)]
use crate::types::AfterToolCallHook;

pub mod events;
pub mod utils;

pub use events::{
    DEFAULT_TURN_CONTINUATION_CAP, HarnessEvent, HarnessListener, OnTurnEndContext, OnTurnEndHook,
    TurnEndAction, TurnEndDecision,
};

use super::compaction::algorithm::CompactAlgorithmRegistry;
use super::compaction::compaction::{CompactionSettings, DEFAULT_COMPACTION_SETTINGS};
use super::cost::{CostSnapshot, CostTracker};
use super::session::session::{BranchSummaryInput, Session};
use super::types::{PromptTemplate, Skill};
use utils::{
    build_system_prompt, extract_user_message_text, extract_user_prompt_text, finish_persisted_run,
    make_session_listener, preview_for_banner,
};

pub struct AgentHarnessOptions {
    /// Base system prompt prepended to the rendered skill catalog.
    pub system_prompt: String,
    pub model: Model,
    pub thinking_level: ThinkingLevel,
    pub skills: Vec<Skill>,
    pub prompt_templates: Vec<PromptTemplate>,
    pub tools: Vec<Arc<dyn AgentTool>>,
    pub session: Session,
    pub stream_fn: Option<StreamFn>,
    /// Auto-compaction thresholds. Defaults to [`DEFAULT_COMPACTION_SETTINGS`].
    pub compaction: CompactionSettings,
    /// Custom compaction algorithm registry. Empty by default (builtin only); embedders
    /// (e.g. the CLI) discover TS extensions host-side and inject the registry here via
    /// `ts_extensions::compact_algorithm_registry`. The builtin algorithm is always
    /// available regardless.
    pub compact_algorithms: Arc<CompactAlgorithmRegistry>,
    /// Optional `before_tool_call` hook. Wire a `PermissionPolicy::as_before_tool_call()` here
    /// to apply danger-detection to tool calls before the loop runs them.
    pub before_tool_call: Option<BeforeToolCallHook>,
    /// Optional `after_tool_call` hook. Used by the LSP supervisor (issue #12) to attach
    /// diagnostics to write/edit tool results.
    pub after_tool_call: Option<AfterToolCallHook>,
    /// Optional control-plane prompt resolution channel (issue #110 design v0.2 Artifact C).
    /// Routes through the bare `Agent`'s `on_control_plane_prompt` slot. `None` is
    /// fail-closed deny — any tool whose `permission_classification` returns `Prompt`
    /// (and no user `before_tool_call` hook hard-blocks) will receive a synthesized deny
    /// at runtime rather than executing. See `crates/core/src/agent_loop.rs` for the
    /// merge semantics.
    pub on_control_plane_prompt: Option<crate::types::OnControlPlanePromptHook>,
    /// Per-session USD cap. When set, the harness refuses to start a new prompt once the
    /// running cost exceeds the cap. `None` disables the check.
    pub budget_cap_usd: Option<f64>,
    /// Optional async closure invoked by [`AgentHarness::reload_skills_from_disk`] to fetch
    /// the up-to-date skill catalog from whatever sources the embedder considers
    /// authoritative (filesystem dirs, registry, …). When `None`,
    /// `reload_skills_from_disk` returns [`ReloadSkillsError::NotConfigured`].
    ///
    /// The closure owns: source directory list, dedup policy (e.g. project-wins),
    /// per-skill diagnostic aggregation. Runtime stays IO-free — it never inspects the
    /// filesystem itself. This keeps `~/.theway/skills` vs project `.theway/skills` precedence
    /// and naming policy in one place (the embedder), so startup loading and runtime
    /// reload (e.g. after `InstallSkillTool` writes a new `SKILL.md`) share one source of
    /// truth.
    pub reload_skills_fn: Option<ReloadSkillsFn>,
    /// Optional hook invoked after every prompt cycle completes. Powers `/goal` and
    /// any other turn-completion driven orchestrator. See [`OnTurnEndHook`] for the
    /// contract. `None` is equivalent to a hook that always returns
    /// [`TurnEndAction::Noop`] (i.e. current behavior).
    pub on_turn_end: Option<OnTurnEndHook>,
    /// Cap on the number of [`TurnEndAction::Continue`] decisions the runtime applies
    /// to a single prompt cycle. `None` uses [`DEFAULT_TURN_CONTINUATION_CAP`]. Set
    /// `0` to disable continuation entirely (the hook still fires once for audit /
    /// observability, but `Continue` decisions are treated as `budget_limited`).
    pub turn_continuation_cap: Option<u32>,
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
        }
    }
}

/// Async loader closure invoked by [`AgentHarness::reload_skills_from_disk`]. Returns the
/// fresh skill catalog (post-dedup, per the embedder's policy) plus any per-skill
/// diagnostics from the load. See [`AgentHarnessOptions::reload_skills_fn`] for the
/// design rationale (one source-of-truth across startup load + runtime reload).
pub type ReloadSkillsFn = std::sync::Arc<
    dyn Fn() -> std::pin::Pin<
            Box<dyn std::future::Future<Output = super::skills::LoadSkillsOutput> + Send>,
        > + Send
        + Sync,
>;

/// Why [`AgentHarness::reload_skills_from_disk`] couldn't run.
#[derive(Debug, thiserror::Error)]
pub enum ReloadSkillsError {
    /// [`AgentHarnessOptions::reload_skills_fn`] was `None` at construction. Callers should
    /// either pass a loader at startup or use [`AgentHarness::replace_skills`] directly.
    #[error("reload_skills_fn was not configured at harness construction")]
    NotConfigured,
}

pub struct AgentHarness {
    /// `pub(crate)`: the harness-side compaction triggers live in `agent/compaction/`
    /// (impl block in `compaction::triggers`) and need access to these internals.
    pub(crate) agent: Arc<Agent>,
    pub(crate) session: Session,
    skills: Mutex<Vec<Skill>>,
    base_system_prompt: String,
    templates: Mutex<Vec<PromptTemplate>>,
    pub(crate) compaction_settings: Mutex<CompactionSettings>,
    /// Resolves `compaction_settings.algorithm` to an implementation (builtin + TS ext).
    pub(crate) compact_algorithms: Arc<CompactAlgorithmRegistry>,
    /// Used by auto-compaction to call the LLM for summarization.
    pub(crate) stream_fn: Option<StreamFn>,
    /// Harness-level lifecycle listeners. Separate from `Agent::listeners` — those cover
    /// per-turn events; this covers cross-turn / session-level decisions. Held behind an
    /// `Arc` so an unsubscriber closure can drop its captured handle independently of the
    /// `AgentHarness` lifetime.
    harness_listeners: Arc<Mutex<Vec<HarnessListener>>>,
    session_start_emitted: Mutex<bool>,
    /// Running token / cost totals for this harness lifetime. Updated automatically by an
    /// internal listener subscribed to `Agent::MessageEnd`. Snapshot via [`Self::cost`].
    cost: CostTracker,
    budget_cap_usd: Option<f64>,
    /// Embedder-supplied skill catalog loader. See [`AgentHarnessOptions::reload_skills_fn`]
    /// for ownership of source directories + dedup policy.
    reload_skills_fn: Option<ReloadSkillsFn>,
    /// Optional turn-completion hook. See [`OnTurnEndHook`]. `None` keeps the legacy
    /// "one prompt cycle per call" behavior.
    on_turn_end: Option<OnTurnEndHook>,
    /// Resolved continuation cap — defaults to [`DEFAULT_TURN_CONTINUATION_CAP`] when
    /// `AgentHarnessOptions::turn_continuation_cap` is `None`.
    turn_continuation_cap: u32,
    /// Cancellation token for the currently-running `OnTurnEndHook` future, when one
    /// is in flight. Wired so [`Self::abort`] cancels the hook (e.g. an evaluator
    /// sub-agent call) the same way it cancels the inner agent loop.
    active_hook_cancel: Mutex<Option<tokio_util::sync::CancellationToken>>,
}

impl AgentHarness {
    pub fn new(options: AgentHarnessOptions) -> Self {
        let mut state = AgentState::default();
        state.model = Some(options.model);
        state.thinking_level = Some(options.thinking_level);
        state.tools = options.tools;
        state.system_prompt = build_system_prompt(&options.system_prompt, &options.skills);

        let agent = Agent::new(AgentOptions {
            initial_state: Some(state),
            stream_fn: options.stream_fn.clone(),
            before_tool_call: options.before_tool_call.clone(),
            after_tool_call: options.after_tool_call.clone(),
            on_control_plane_prompt: options.on_control_plane_prompt.clone(),
            ..Default::default()
        });

        let cost = CostTracker::new();
        // Subscribe the cost tracker to assistant MessageEnd events. Listener is wired against
        // the inner Agent so the harness has no per-prompt setup cost.
        let _ = agent.subscribe(cost.as_listener());

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
            session_start_emitted: Mutex::new(false),
            cost,
            budget_cap_usd: options.budget_cap_usd,
            reload_skills_fn: options.reload_skills_fn,
            active_hook_cancel: Mutex::new(None),
            on_turn_end: options.on_turn_end,
            turn_continuation_cap: options
                .turn_continuation_cap
                .unwrap_or(DEFAULT_TURN_CONTINUATION_CAP),
        }
    }

    /// Snapshot of running token + cost totals.
    pub fn cost(&self) -> CostSnapshot {
        self.cost.snapshot()
    }

    /// Reset the cost tracker — `/cost reset` and on session-switch.
    pub fn reset_cost(&self) {
        self.cost.reset();
    }

    /// Register a harness-level lifecycle listener. Returns an unsubscriber closure.
    ///
    /// Listener panics are caught — see [`HarnessEvent`] for the isolation contract. The
    /// returned closure removes the listener; calling it twice is a no-op.
    pub fn subscribe_harness(&self, listener: HarnessListener) -> Box<dyn FnOnce() + Send> {
        self.harness_listeners.lock().push(listener.clone());
        // Identity-match the listener for removal. Capture the data-pointer address as a
        // `usize` (Send) so the unsubscriber doesn't carry a raw pointer across threads.
        let target = Arc::as_ptr(&listener) as *const () as usize;
        let listeners = Arc::clone(&self.harness_listeners);
        Box::new(move || {
            let mut g = listeners.lock();
            if let Some(i) = g
                .iter()
                .position(|l| (Arc::as_ptr(l) as *const () as usize) == target)
            {
                g.remove(i);
            }
        })
    }

    pub(crate) fn emit_harness_event(&self, event: HarnessEvent) {
        let listeners = self.harness_listeners.lock().clone();
        for l in listeners {
            // Each listener runs isolated so one panic doesn't poison the rest.
            let l = l.clone();
            let ev = event.clone();
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || l(ev)));
        }
    }

    fn ensure_session_start_emitted(&self) {
        let mut g = self.session_start_emitted.lock();
        if *g {
            return;
        }
        *g = true;
        let count = self.agent.state().messages.len();
        drop(g);
        self.emit_harness_event(HarnessEvent::SessionStart {
            messages_replayed: count,
        });
    }

    /// Clone of the inner agent handle (for detached tasks that need an `Arc<Agent>`).
    pub fn agent_arc(&self) -> Arc<Agent> {
        Arc::clone(&self.agent)
    }

    pub fn agent(&self) -> &Agent {
        &self.agent
    }

    /// Accept an incoming [`Trigger`] from a notification adapter. Evaluates it against the
    /// runtime's dedup + cycle bookkeeping, persists a
    /// `SessionTreeEntry::Custom { custom_type: "trigger" }` audit entry summarizing the
    /// decision, and emits [`HarnessEvent::TriggerHandlingStart`] / [`HarnessEvent::TriggerHandled`].
    ///
    /// Returns the [`EvaluationOutcome`] so adapters that synchronously dispatched the
    /// trigger know whether downstream rule evaluation should proceed. In this PR `Accept`
    /// is terminal — actually invoking the agent loop on an accepted trigger lands with the
    /// permission evaluator extension and the running-state machine in sub-PR 3.
    ///
    /// Persistence is best-effort: if the audit write fails, this method still returns the
    /// evaluator outcome and emits a [`HarnessEvent::PersistenceError`] alongside the
    /// `TriggerHandled` event (with `audit_entry_id = None`). The trigger evaluation is
    /// authoritative; the audit record is observability.
    pub fn session(&self) -> &Session {
        &self.session
    }

    pub fn skills(&self) -> Vec<Skill> {
        self.skills.lock().clone()
    }

    /// Snapshot of the loaded prompt templates. Listing-only — callers run them via
    /// [`Self::prompt_from_template`].
    pub fn templates(&self) -> Vec<PromptTemplate> {
        self.templates.lock().clone()
    }

    pub fn system_prompt(&self) -> String {
        self.agent.state().system_prompt.clone()
    }

    /// Replace the skill catalog. Rebuilds the system prompt so the in-flight Agent state has
    /// the new `<skills>` block on its next LLM call.
    pub fn replace_skills(&self, skills: Vec<Skill>) {
        *self.skills.lock() = skills;
        let prompt = build_system_prompt(&self.base_system_prompt, &self.skills.lock());
        self.agent.state().system_prompt = prompt;
    }

    /// Hot-reload the skill catalog from disk via the embedder-supplied
    /// [`AgentHarnessOptions::reload_skills_fn`] closure. Used by `InstallSkillTool`,
    /// `/skills reload`, and any future control-plane that needs to refresh the catalog
    /// after a filesystem write — they all share the same source directories + dedup
    /// policy as startup because they go through the same closure.
    ///
    /// Returns the loader's [`super::skills::LoadSkillsOutput`] (skills + per-skill
    /// diagnostics) so the caller can surface a summary to the user. On success the new
    /// catalog has already been applied via [`Self::replace_skills`] and the system prompt
    /// rebuilt — the next prompt will see the new `<skills>` block. In-flight turns
    /// continue against their existing context (no mid-turn prompt mutation).
    ///
    /// Errors with [`ReloadSkillsError::NotConfigured`] if no loader was wired at
    /// construction — embedders that don't need reload simply leave `reload_skills_fn` as
    /// `None` and use [`Self::replace_skills`] directly.
    pub async fn reload_skills_from_disk(
        &self,
    ) -> Result<super::skills::LoadSkillsOutput, ReloadSkillsError> {
        let loader = self
            .reload_skills_fn
            .as_ref()
            .ok_or(ReloadSkillsError::NotConfigured)?
            .clone();
        let out = loader().await;
        self.replace_skills(out.skills.clone());
        self.emit_harness_event(HarnessEvent::SkillsReloaded {
            total: out.skills.len(),
        });
        Ok(out)
    }

    pub fn abort(&self) {
        self.agent.abort();
        // If an `OnTurnEndHook` future is currently in flight (typically waiting on an
        // evaluator sub-agent), cancel it too so Ctrl-C / `/cancel` interrupts the
        // entire prompt+continuation pipeline, not just the inner agent loop.
        if let Some(token) = self.active_hook_cancel.lock().as_ref() {
            token.cancel();
        }
    }

    /// Interrupt the in-flight turn: cancels the current LLM call only. The run ends
    /// unless a steering message was queued beforehand (then the next turn carries it).
    pub fn interrupt(&self) {
        self.agent.interrupt();
    }

    pub fn enqueue_steering(&self, message: AgentMessage) {
        self.agent.enqueue_steering(message);
    }

    pub fn enqueue_follow_up(&self, message: AgentMessage) {
        self.agent.enqueue_follow_up(message);
    }

    pub fn subscribe(&self, listener: AgentListener) -> impl FnOnce() {
        self.agent.subscribe(listener)
    }

    /// Switch model. Persists a `ModelChange` session entry so resume sees the right one.
    pub async fn set_model(&self, model: Model) -> Result<String, super::types::SessionError> {
        let provider = model.provider.0.clone();
        let model_id = model.id.clone();
        let id = self.session.append_model_change(provider, model_id).await?;
        self.agent.state().model = Some(model);
        Ok(id)
    }

    pub async fn set_thinking_level(
        &self,
        level: ThinkingLevel,
    ) -> Result<String, super::types::SessionError> {
        let id = self
            .session
            .append_thinking_level_change(level.as_str())
            .await?;
        self.agent.state().thinking_level = Some(level);
        Ok(id)
    }

    /// Move the session leaf to a specific entry id (or root). When `summary` is provided,
    /// records a branch_summary entry so siblings see the fork's contribution. Replays the new
    /// branch into agent state via [`Self::rehydrate_from_session`].
    pub async fn move_to(
        &self,
        entry_id: Option<&str>,
        summary: Option<BranchSummaryInput>,
    ) -> Result<Option<String>, super::types::SessionError> {
        let from = self.session.leaf_id().await.ok().flatten();
        let result = self.session.move_to(entry_id, summary).await?;
        self.rehydrate_from_session().await?;
        self.emit_harness_event(HarnessEvent::Branch {
            from_entry_id: from,
            to_entry_id: entry_id.map(|s| s.to_string()),
            summary_entry_id: result.clone(),
        });
        Ok(result)
    }

    /// Replace the agent's in-memory state with the session's active branch. Messages, model,
    /// and thinking level are restored from `Session::build_context()`. Returns the rebuilt
    /// `SessionContext` for callers that want to render the transcript or inspect the recovered
    /// model.
    ///
    /// CLI startup (`--resume`) and post-branch-switch flows both go through this — keeps the
    /// "how do we rehydrate?" decision in one place.
    pub async fn rehydrate_from_session(
        &self,
    ) -> Result<super::session::session::SessionContext, super::types::SessionError> {
        let ctx = self.session.build_context().await?;
        let mut s = self.agent.state();
        s.messages = ctx.messages.clone();
        if let Some(model) = &ctx.model {
            // Restore the previously-active model when it's still in the catalog. Unknown
            // models keep whatever the caller set up — the resume banner reflects that fact.
            if let Some(m) = theway_llm_provider::get_model(
                &theway_llm_provider::Provider::from(model.provider.clone()),
                &model.model_id,
            ) {
                s.model = Some(m);
            }
        }
        if let Ok(level) = ctx.thinking_level.parse::<ThinkingLevel>() {
            s.thinking_level = Some(level);
        }
        Ok(ctx)
    }

    /// Pick a template by name, interpolate, and prompt the agent.
    pub async fn prompt_from_template(
        &self,
        name: &str,
        vars: serde_json::Map<String, serde_json::Value>,
    ) -> Result<(), AgentRunError> {
        let template = {
            let g = self.templates.lock();
            g.iter().find(|t| t.name == name).cloned()
        };
        let template = match template {
            Some(t) => t,
            None => {
                return Err(AgentRunError::Other(format!(
                    "unknown prompt template: {name}"
                )));
            }
        };
        let rendered = template.interpolate(&vars);
        self.prompt(rendered).await
    }

    /// Prompt the agent with text. Runs auto-compaction first, persists results to session.
    pub async fn prompt(&self, text: impl Into<String>) -> Result<(), AgentRunError> {
        let text = text.into();
        let user_message = AgentMessage::Llm(PiMessage::User(theway_llm_provider::UserMessage {
            role: theway_llm_provider::UserRole::User,
            content: theway_llm_provider::UserContent::Text(text),
            timestamp: chrono::Utc::now().timestamp_millis(),
        }));
        self.prompt_with_message(user_message).await
    }

    /// Prompt with text + images (multimodal users).
    pub async fn prompt_with_images(
        &self,
        text: impl Into<String>,
        images: Vec<ImageContent>,
    ) -> Result<(), AgentRunError> {
        let mut blocks: Vec<theway_llm_provider::UserContentBlock> = images
            .into_iter()
            .map(theway_llm_provider::UserContentBlock::Image)
            .collect();
        let text = text.into();
        if !text.is_empty() {
            blocks.insert(0, theway_llm_provider::UserContentBlock::text(text));
        }
        let user_message = AgentMessage::Llm(PiMessage::User(theway_llm_provider::UserMessage {
            role: theway_llm_provider::UserRole::User,
            content: theway_llm_provider::UserContent::Blocks(blocks),
            timestamp: chrono::Utc::now().timestamp_millis(),
        }));
        self.prompt_with_message(user_message).await
    }

    async fn prompt_with_message(&self, msg: AgentMessage) -> Result<(), AgentRunError> {
        self.ensure_session_start_emitted();
        self.check_budget_cap()?;
        // Run compaction if we've crossed the threshold. This must happen before the user
        // message is appended so the cut point doesn't risk splitting the current turn.
        self.run_auto_compaction().await?;

        // First iteration runs `agent.prompt(msg)` with the caller's user message; any
        // `TurnEndAction::Continue` follow-up runs `agent.prompt(<new user msg>)` with the
        // text the hook returned. `run_turn_with_continuation` handles the hook loop,
        // persistence listener wiring, audit emission, and continuation cap enforcement.
        let last_user_prompt = extract_user_prompt_text(&msg);
        self.run_turn_with_continuation(Some(msg), last_user_prompt)
            .await
    }

    pub async fn continue_(&self) -> Result<(), AgentRunError> {
        self.ensure_session_start_emitted();
        self.check_budget_cap()?;
        self.run_auto_compaction().await?;

        // `continue_` runs `agent.continue_()` on the first iteration (no new user
        // message), and falls back to `agent.prompt(<hook text>)` on continuations
        // exactly like the `prompt_with_message` path.
        let last_user_prompt = self.last_user_text_from_state();
        self.run_turn_with_continuation(None, last_user_prompt)
            .await
    }

    /// Common driver for one prompt cycle plus zero or more `OnTurnEndHook`-driven
    /// continuation cycles. `first_msg = Some(_)` triggers `agent.prompt(msg)` on the
    /// first iteration; `None` triggers `agent.continue_()` (used by the public
    /// [`Self::continue_`] entry). Subsequent iterations always go through
    /// `agent.prompt(<user msg built from hook text>)`.
    async fn run_turn_with_continuation(
        &self,
        first_msg: Option<AgentMessage>,
        last_user_prompt: Option<String>,
    ) -> Result<(), AgentRunError> {
        let mut continuation_count: u32 = 0;
        let mut pending_user_msg = first_msg;
        let mut is_first_iteration = true;
        let mut last_user_prompt = last_user_prompt;

        loop {
            let (listener, persist_errors) = make_session_listener(self.session.clone());
            let unsub = self.agent.subscribe(listener);
            let result = if is_first_iteration {
                match pending_user_msg.take() {
                    Some(msg) => self.agent.prompt(msg).await,
                    None => self.agent.continue_().await,
                }
            } else {
                // Continuation: every iteration after the first runs as a fresh prompt.
                let msg = pending_user_msg.take().expect(
                    "continuation iteration must have a pending user message from the hook",
                );
                self.agent.prompt(msg).await
            };
            unsub();
            finish_persisted_run(result, persist_errors)?;
            is_first_iteration = false;

            // No hook configured → behave like the legacy single-cycle path. Skip event
            // and audit emission so embedders that never opt in pay zero overhead and
            // see no schema change in their session jsonl.
            let Some(hook) = self.on_turn_end.clone() else {
                return Ok(());
            };

            // Cap enforcement: if the previous iteration was already a continuation and
            // the cap is exhausted, record `budget_limited` and stop without invoking
            // the hook again. Counted on `continuation_count`, not the loop iteration
            // count, so the initial turn never counts against the cap.
            if continuation_count >= self.turn_continuation_cap {
                let reason = format!(
                    "continuation cap reached: {} >= {}",
                    continuation_count, self.turn_continuation_cap
                );
                self.record_turn_end_decision(
                    "budget_limited",
                    continuation_count,
                    Some(reason.clone()),
                    None,
                    None,
                )
                .await;
                return Ok(());
            }

            // Snapshot transcript outside the hook future so the parking_lot guard is
            // released before any `.await`. The hook is responsible for trimming.
            let transcript_snapshot = self.agent.state().messages.clone();
            let ctx = OnTurnEndContext {
                transcript: transcript_snapshot,
                continuation_count,
                last_user_prompt: last_user_prompt.clone(),
            };

            // Wire a cancel token to harness.abort() for the duration of the hook
            // future. Released in all exit paths below so abort() does not see stale
            // tokens between turns.
            let cancel = tokio_util::sync::CancellationToken::new();
            *self.active_hook_cancel.lock() = Some(cancel.clone());
            let decision = hook(ctx, cancel).await;
            *self.active_hook_cancel.lock() = None;

            match decision.action {
                TurnEndAction::Noop => {
                    // Hook deliberately recused itself — behave as if no hook were
                    // configured: no audit, no event. Lets long-lived hooks (e.g.
                    // `/goal`'s permanent registration) stay quiet on every plain
                    // turn that doesn't have an active goal.
                    return Ok(());
                }
                TurnEndAction::Stop => {
                    self.record_turn_end_decision(
                        "stop",
                        continuation_count,
                        None,
                        None,
                        decision.payload,
                    )
                    .await;
                    return Ok(());
                }
                TurnEndAction::Pause { reason } => {
                    self.record_turn_end_decision(
                        "pause",
                        continuation_count,
                        Some(reason),
                        None,
                        decision.payload,
                    )
                    .await;
                    return Ok(());
                }
                TurnEndAction::Continue { prompt } => {
                    continuation_count = continuation_count.saturating_add(1);
                    let preview = Some(preview_for_banner(&prompt, 80));
                    self.record_turn_end_decision(
                        "continue",
                        continuation_count,
                        None,
                        preview,
                        decision.payload,
                    )
                    .await;
                    // Build the follow-up user message and loop. Re-check the budget cap
                    // before each continuation iteration so a `Continue` decision cannot
                    // bypass a tripped cap. Compaction also runs again because the
                    // previous turn may have grown the transcript past the threshold.
                    self.check_budget_cap()?;
                    self.run_auto_compaction().await?;
                    let user_msg =
                        AgentMessage::Llm(PiMessage::User(theway_llm_provider::UserMessage {
                            role: theway_llm_provider::UserRole::User,
                            content: theway_llm_provider::UserContent::Text(prompt.clone()),
                            timestamp: chrono::Utc::now().timestamp_millis(),
                        }));
                    last_user_prompt = Some(prompt);
                    pending_user_msg = Some(user_msg);
                }
            }
        }
    }

    /// Shared budget-cap precondition used by every entry path
    /// (`prompt` / `prompt_with_images` / `continue_` / continuation iterations).
    fn check_budget_cap(&self) -> Result<(), AgentRunError> {
        if let Some(cap) = self.budget_cap_usd {
            let total = self.cost.snapshot().tokens.cost.total;
            if total >= cap {
                return Err(AgentRunError::Other(format!(
                    "budget cap reached: ${total:.4} >= ${cap:.4}. Reset with /cost reset or raise budget_cap_usd.",
                )));
            }
        }
        Ok(())
    }

    /// Walk the current agent transcript in reverse and return the text of the most
    /// recent `Message::User` with text content, if any. Used by `continue_` to fill
    /// `OnTurnEndContext::last_user_prompt` so evaluators don't need to re-scan.
    fn last_user_text_from_state(&self) -> Option<String> {
        let state = self.agent.state();
        state.messages.iter().rev().find_map(|m| match m {
            AgentMessage::Llm(PiMessage::User(u)) => extract_user_message_text(u),
            _ => None,
        })
    }

    /// Persist a `turn_end_decision` audit entry and emit the matching
    /// [`HarnessEvent::TurnEnded`] event. Best-effort: persistence failures do not
    /// abort the surrounding prompt cycle (the event still fires so observers can
    /// flag the lost audit), matching the trigger audit reflux pattern.
    async fn record_turn_end_decision(
        &self,
        decision: &'static str,
        continuation_count: u32,
        reason: Option<String>,
        next_prompt_preview: Option<String>,
        payload: Option<serde_json::Value>,
    ) {
        let data = serde_json::json!({
            "decision": decision,
            "continuation_count": continuation_count,
            "reason": reason,
            "next_prompt_preview": next_prompt_preview,
            "payload": payload.unwrap_or(serde_json::Value::Null),
        });
        if let Err(e) = self
            .session
            .append_custom("turn_end_decision", Some(data))
            .await
        {
            self.emit_harness_event(HarnessEvent::PersistenceError {
                context: "turn_end_decision".into(),
                message: format!("turn_end_decision append failed: {:?}", e.code),
            });
        }
        self.emit_harness_event(HarnessEvent::TurnEnded {
            decision,
            continuation_count,
            reason,
            next_prompt_preview,
        });
    }
}
