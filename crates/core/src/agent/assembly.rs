//! `AgentHarness` — opinionated assembly around the bare `Agent`. 1:1 port of
//! `packages/agent/src/harness/agent-harness.ts` (~995 lines).
//!
//! Single file by owner decision (see the AGENTS.md file-size governance exceptions):
//! the composed agent API (struct + options + lifecycle events + session-listener
//! helpers) reads as one unit. Do NOT split it back into a directory.
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

use crate::agent::session::session::{BranchSummaryInput, Session};
use crate::agent::system_prompt::format_skills_for_system_prompt;
use crate::agent::{Agent, AgentOptions, AgentRunError, LoopEvent, LoopListener};
use crate::types::*;
// AfterToolCallHook is re-exported under types::* via `pub use` in the module; if it isn't
// directly visible here, fall back to the absolute path inside Agent::new.
#[allow(unused_imports)]
use crate::types::AfterToolCallHook;

use super::compaction::algorithm::CompactAlgorithmRegistry;
use super::compaction::compaction::{CompactionSettings, DEFAULT_COMPACTION_SETTINGS};
use super::cost::{CostSnapshot, CostTracker};
use super::types::{PromptTemplate, Skill};

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
    /// at runtime rather than executing. See `crate::agent::run_loop` for the
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
    harness_listeners: Arc<Mutex<Vec<SessionListener>>>,
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
    /// Listener panics are caught — see [`SessionEvent`] for the isolation contract. The
    /// returned closure removes the listener; calling it twice is a no-op.
    pub fn subscribe_harness(&self, listener: SessionListener) -> Box<dyn FnOnce() + Send> {
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

    pub(crate) fn emit_harness_event(&self, event: SessionEvent) {
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
        self.emit_harness_event(SessionEvent::Started {
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
    /// decision, and emits [`SessionEvent::TriggerHandlingStart`] / [`SessionEvent::TriggerHandled`].
    ///
    /// Returns the [`EvaluationOutcome`] so adapters that synchronously dispatched the
    /// trigger know whether downstream rule evaluation should proceed. In this PR `Accept`
    /// is terminal — actually invoking the agent loop on an accepted trigger lands with the
    /// permission evaluator extension and the running-state machine in sub-PR 3.
    ///
    /// Persistence is best-effort: if the audit write fails, this method still returns the
    /// evaluator outcome and emits a [`SessionEvent::PersistenceError`] alongside the
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
        self.emit_harness_event(SessionEvent::SkillsReloaded {
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

    pub fn subscribe(&self, listener: LoopListener) -> impl FnOnce() {
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
        self.emit_harness_event(SessionEvent::Branch {
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
    /// [`SessionEvent::TurnDecision`] event. Best-effort: persistence failures do not
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
            self.emit_harness_event(SessionEvent::PersistenceError {
                context: "turn_end_decision".into(),
                message: format!("turn_end_decision append failed: {:?}", e.code),
            });
        }
        self.emit_harness_event(SessionEvent::TurnDecision {
            decision,
            continuation_count,
            reason,
            next_prompt_preview,
        });
    }
}

// ─────────────────────────────────────────────────────────────────────────────────────────
// Harness lifecycle events + the OnTurnEnd hook contract
// ─────────────────────────────────────────────────────────────────────────────────────────

/// Harness-level lifecycle events. These are emitted in addition to the per-turn `LoopEvent`s
/// the inner `Agent` already publishes — they cover the cross-turn lifecycle decisions the
/// harness is responsible for (compaction, branching, session boundaries).
///
/// Subscribers run synchronously in delivery order on the calling tokio task. Panicking
/// subscribers are isolated via `catch_unwind` so one bad observer cannot break the harness;
/// the offending listener is dropped from the registry.
#[derive(Clone, Debug)]
pub enum SessionEvent {
    /// First call to `prompt`/`continue_`/`prompt_from_template` after `AgentHarness::new`
    /// fires this once. `messages_replayed` reflects how many session messages were already on
    /// the active branch (e.g. a `--resume` start vs a fresh session).
    Started { messages_replayed: usize },
    /// Auto- or manual compaction ran. `from_hook = true` currently means it came from
    /// `force_compact` (the CLI `/compact` path); `false` means the internal threshold check
    /// triggered it before a prompt.
    Compaction {
        from_hook: bool,
        summary: String,
        tokens_before: u64,
    },
    /// A branch operation (`move_to` / `fork`) landed. `from_entry_id` is `None` for moves to
    /// the root; `to_entry_id` is the new active leaf id (or `None` for root).
    Branch {
        from_entry_id: Option<String>,
        to_entry_id: Option<String>,
        summary_entry_id: Option<String>,
    },
    /// that observability (TUI banner, `/triggers`, JSONL logs) can mark the audit as
    /// best-effort lost rather than dropping it silently.
    PersistenceError {
        /// Free-form context — pinned strings: `"trigger_audit"`, `"trigger_result"`. New
        /// write sites that surface through this event must pin themselves to a stable
        /// string.
        context: String,
        /// Short, secret-free message. The original `SessionError` is *not* exposed because
        /// some implementations include filesystem paths or storage backend details that
        /// belong in trace logs, not user-facing event surfaces.
        message: String,
    },
    /// the count of `Continue` decisions that fired earlier in the same prompt cycle.
    TurnDecision {
        decision: &'static str,
        continuation_count: u32,
        reason: Option<String>,
        next_prompt_preview: Option<String>,
    },
    /// The skill catalog was hot-reloaded via [`AgentHarness::reload_skills_from_disk`]
    /// (`InstallSkill`, `SkillBuilder`, `/skills reload`, …). UIs that display the catalog
    /// repaint off this — a reload can happen with no other feed activity (e.g. a trigger
    /// or cron sub-agent installing a skill while the parent sits idle).
    SkillsReloaded { total: usize },
}

/// Listener for [`SessionEvent`]. Shape mirrors `crate::agent::LoopListener` so the same Fn
/// helpers translate.
pub type SessionListener = Arc<dyn Fn(SessionEvent) + Send + Sync>;

// ─────────────────────────────────────────────────────────────────────────────────────────
// OnTurnEnd hook (powers `/goal` and other turn-completion driven orchestrators)
// ─────────────────────────────────────────────────────────────────────────────────────────

/// Snapshot passed into [`OnTurnEndHook`] after a prompt-cycle reaches a natural stop
/// (assistant turned in a no-tool-call message, the agent's own `should_stop_after_turn`
/// returned true, etc.). The hook owns the cross-prompt decision: should the harness
/// start another prompt cycle in the same conversation (for `/goal` evaluator-driven
/// continuation), pause it, or stop normally.
///
/// `transcript` is a **clone** of `Agent::state().messages` taken at the boundary — the
/// mutex is released before the hook runs, so the hook future is `'static`. The hook is
/// responsible for bounding what it forwards downstream (e.g. last N messages, token cap)
/// when it builds an evaluator prompt; the runtime does not pre-trim because different
/// orchestrators want different windows.
///
/// `continuation_count` is the number of times this same prompt-cycle has already been
/// continued by an earlier `TurnEndAction::Continue` decision. Starts at 0 on the
/// original user/template/continue entry, increments by 1 each time the hook decides to
/// continue. The hard cap is [`AgentHarnessOptions::turn_continuation_cap`]; the runtime
/// stops calling the hook (and records `decision: "budget_limited"`) once it would be
/// exceeded — no need for the hook to enforce the cap itself.
///
/// `last_user_prompt` carries the text of the most recent `Message::User` text content,
/// when one is identifiable, so evaluators can render "the user asked for X" without
/// re-walking the transcript. `None` when no user-text message exists (e.g. `continue_`
/// from a transcript with only assistant + tool messages).
#[derive(Clone)]
pub struct OnTurnEndContext {
    pub transcript: Vec<AgentMessage>,
    pub continuation_count: u32,
    pub last_user_prompt: Option<String>,
}

/// What the runtime should do after [`OnTurnEndHook`] inspects a completed prompt cycle.
///
/// `Stop` / `Pause` / `Continue` each map to a stable `decision` string in the persisted
/// `turn_end_decision` audit entry (`"stop"` / `"pause"` / `"continue"`); a fourth
/// `"budget_limited"` value is reserved for the runtime-emitted audit when the
/// continuation cap is hit before the hook can run, so call sites never need to invent
/// that string themselves. `Noop` is intentionally not in that list — it deliberately
/// writes nothing.
#[derive(Clone, Debug)]
pub enum TurnEndAction {
    /// Hook is currently inactive and has nothing to record for this turn. Behaves
    /// identically to "no `on_turn_end` configured": **no `turn_end_decision` audit
    /// entry is written, and no [`SessionEvent::TurnDecision`] is emitted**. Use when the
    /// hook is permanently registered but only meaningful in specific session states
    /// — e.g. `/goal` returns `Noop` when there is no active goal, when the goal is
    /// already `achieved`, or when the user has paused it externally — so untouched
    /// sessions don't accumulate noise audit entries on every prompt.
    ///
    /// `TurnEndDecision::payload` is ignored when `action == Noop`; pass `None`.
    Noop,
    /// Normal completion. Runtime returns control to the caller. Records
    /// `decision: "stop"` in the `turn_end_decision` audit and emits
    /// [`SessionEvent::TurnDecision`].
    Stop,
    /// Soft stop with an explanatory reason (e.g. "evaluator unavailable", "user
    /// requested pause"). Persisted in `turn_end_decision.data.reason` and surfaced
    /// through [`SessionEvent::TurnDecision`]. Runtime returns control to the caller.
    Pause { reason: String },
    /// Run another prompt cycle in the same conversation. The runtime appends `prompt`
    /// as a user `AgentMessage`, runs auto-compaction again, then drives the inner
    /// agent's loop. `continuation_count` increments by 1 before the next hook call.
    Continue { prompt: String },
}

impl TurnEndAction {
    /// Stable `decision` string for the `turn_end_decision` audit entry. `Noop` is
    /// intentionally unmapped — it returns `None` and signals to the runtime that no
    /// audit / event should be emitted for this turn. Avoid stringifying the `Debug`
    /// representation — these values are part of the audit contract and downstream
    /// JSONL readers compare against them.
    pub fn as_audit_str(&self) -> Option<&'static str> {
        match self {
            Self::Noop => None,
            Self::Stop => Some("stop"),
            Self::Pause { .. } => Some("pause"),
            Self::Continue { .. } => Some("continue"),
        }
    }
}

/// Decision envelope returned from [`OnTurnEndHook`]. Wrapping the action lets the hook
/// attach an opaque embedder-owned `payload` that gets persisted into the
/// `turn_end_decision` audit record under `data.payload` — runtime never inspects it.
/// `/goal` uses this to record evaluator JSON, evidence quotes, evaluator model id, etc.,
/// without runtime needing to know about goal-mode-specific fields.
#[derive(Clone, Debug)]
pub struct TurnEndDecision {
    pub action: TurnEndAction,
    /// Optional structured payload merged into the `turn_end_decision` audit entry as
    /// `data.payload`. `None` writes `data.payload: null`. The embedder is responsible
    /// for keeping this serializable and small — bodies should be capped before being
    /// returned, just like trigger result summaries.
    pub payload: Option<serde_json::Value>,
}

impl From<TurnEndAction> for TurnEndDecision {
    fn from(action: TurnEndAction) -> Self {
        Self {
            action,
            payload: None,
        }
    }
}

/// Hook invoked at the boundary between two prompt cycles inside
/// [`AgentHarness::prompt`] / [`AgentHarness::continue_`]. Fires exactly once after the
/// inner agent's loop returns (success or `AgentRunError` short-circuit), with the cancel
/// token wired to [`AgentHarness::abort`] so user-driven aborts interrupt the hook's own
/// awaits (e.g. an evaluator sub-agent call).
///
/// Returning [`TurnEndAction::Continue { prompt }`] starts a new prompt cycle with the
/// given text appended as a `Message::User`. Returning [`TurnEndAction::Stop`] or
/// [`TurnEndAction::Pause`] returns control to the caller with an audit/event.
/// Returning [`TurnEndAction::Noop`] returns control without audit/event, matching the
/// no-hook path. `None` (no hook configured) is equivalent to `Noop`.
///
/// The hook runs **after** the persistence listener has flushed every `MessageEnd` to
/// the session, so `transcript` matches what `--resume` would replay. It runs **before**
/// the runtime writes the `turn_end_decision` audit entry — the entry's `payload` field
/// comes from the returned [`TurnEndDecision::payload`].
pub type OnTurnEndHook = Arc<
    dyn Fn(
            OnTurnEndContext,
            tokio_util::sync::CancellationToken,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = TurnEndDecision> + Send>>
        + Send
        + Sync,
>;

/// Default maximum number of [`TurnEndAction::Continue`] iterations per prompt cycle.
/// When exceeded, the runtime records a `turn_end_decision` audit with
/// `decision: "budget_limited"` and returns control to the caller without invoking the
/// hook again. Embedders override via [`AgentHarnessOptions::turn_continuation_cap`].
pub const DEFAULT_TURN_CONTINUATION_CAP: u32 = 25;

// ─────────────────────────────────────────────────────────────────────────────────────────
// Free helper functions: system-prompt assembly, session persistence listener, banner
// previews, user-prompt text extraction
// ─────────────────────────────────────────────────────────────────────────────────────────

fn build_system_prompt(base: &str, skills: &[Skill]) -> String {
    let skills_block = format_skills_for_system_prompt(skills);
    if base.is_empty() {
        return skills_block;
    }
    if skills_block.is_empty() {
        return base.to_string();
    }
    format!("{base}\n\n{skills_block}")
}

/// Build an `LoopListener` that persists every emitted `MessageEnd` to the session log.
fn make_session_listener(
    session: Session,
) -> (
    crate::agent::LoopListener,
    Arc<Mutex<Vec<crate::agent::types::SessionError>>>,
) {
    let errors = Arc::new(Mutex::new(Vec::new()));
    let listener_errors = errors.clone();
    let listener: crate::agent::LoopListener = Arc::new(move |event, _cancel| {
        let session = session.clone();
        let listener_errors = listener_errors.clone();
        Box::pin(async move {
            match event {
                LoopEvent::MessageEnd { message } => {
                    if let Err(e) = session.append_message(message).await {
                        listener_errors.lock().push(e);
                    }
                }
                LoopEvent::ControlPlanePromptResolved {
                    tool_call_id,
                    tool_name,
                    args_hash,
                    label,
                    decision,
                    reason,
                } => {
                    // Issue #110 design v0.2 Artifact E: write a `control_plane_prompt`
                    // Custom audit per resolution. Label is capped at 200 chars
                    // (cap-inclusive on char boundary) so a hook-supplied unbounded
                    // string cannot grow the audit / `--resume` body without limit
                    // — per @QA-Release-Lead non-blocking note on PR #135.
                    let data = serde_json::json!({
                        "schema_version": 1,
                        "tool_call_id": tool_call_id,
                        "tool_name": tool_name,
                        "args_hash": args_hash,
                        "label": cap_control_plane_audit_label(&label),
                        "decision": decision,
                        "reason": reason,
                        "at": chrono::Utc::now().to_rfc3339(),
                    });
                    if let Err(e) = session
                        .append_custom("control_plane_prompt", Some(data))
                        .await
                    {
                        listener_errors.lock().push(e);
                    }
                }
                _ => {}
            }
        })
    });
    (listener, errors)
}

/// Cap rule for `control_plane_prompt.data.label`. Hook-supplied labels MUST be
/// bounded before persistence to prevent an embedder hook from inflating audit /
/// `--resume` body size. Per @QA-Release-Lead non-blocking note on PR #135.
///
/// Caps at 200 chars, cap-inclusive on char boundary (same shape as RFC 1 sub-PR 5a's
/// 4 KiB summary cap — character-walked, not byte-walked, so multi-byte chars don't
/// land mid-rune).
const CONTROL_PLANE_PROMPT_LABEL_CAP_CHARS: usize = 200;

fn cap_control_plane_audit_label(label: &str) -> String {
    if label.chars().count() <= CONTROL_PLANE_PROMPT_LABEL_CAP_CHARS {
        return label.to_string();
    }
    let mut out: String = label
        .chars()
        .take(CONTROL_PLANE_PROMPT_LABEL_CAP_CHARS.saturating_sub(1))
        .collect();
    out.push('…');
    out
}

fn finish_persisted_run(
    result: Result<(), AgentRunError>,
    persist_errors: Arc<Mutex<Vec<crate::agent::types::SessionError>>>,
) -> Result<(), AgentRunError> {
    result?;
    if let Some(e) = persist_errors.lock().first() {
        return Err(AgentRunError::Other(format!("session append message: {e}")));
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────────────────
// Sub-agent execution (RFC 1 sub-PR 5a)
// ─────────────────────────────────────────────────────────────────────────────────────────

/// Emit a [`SessionEvent`] to a snapshot of the listener registry, isolating each listener
/// with `catch_unwind` so a single panicking listener cannot poison the others. Mirrors
/// the contract of `AgentHarness::emit_harness_event` but operates on a cloned `Arc` of
/// listeners (so the spawned sub-agent task does not need an `AgentHarness` reference).
fn preview_for_banner(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max_chars).collect();
    out.push('…');
    out
}

/// Extract the text body of a `Message::User`, joining `Blocks` text content. Returns
/// `None` for image-only messages or empty text. Used to fill
/// [`OnTurnEndContext::last_user_prompt`] for the most recent user message in the
/// transcript.
fn extract_user_message_text(u: &theway_llm_provider::UserMessage) -> Option<String> {
    match &u.content {
        theway_llm_provider::UserContent::Text(s) => {
            if s.is_empty() {
                None
            } else {
                Some(s.clone())
            }
        }
        theway_llm_provider::UserContent::Blocks(blocks) => {
            let mut out = String::new();
            for block in blocks {
                if let theway_llm_provider::UserContentBlock::Text(t) = block {
                    if !out.is_empty() {
                        out.push('\n');
                    }
                    out.push_str(&t.text);
                }
            }
            if out.is_empty() { None } else { Some(out) }
        }
    }
}

/// Extract the text payload from the `AgentMessage` the caller passed into
/// `prompt_with_message`. Returns `None` for non-LLM or non-user messages and for empty
/// content. Used to fill [`OnTurnEndContext::last_user_prompt`] for the freshly-arrived
/// user prompt before the transcript has been mutated.
fn extract_user_prompt_text(msg: &AgentMessage) -> Option<String> {
    match msg {
        AgentMessage::Llm(PiMessage::User(u)) => extract_user_message_text(u),
        _ => None,
    }
}
