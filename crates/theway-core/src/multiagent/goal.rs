//! Session-level `/goal` stop hook.
//!
//! A goal is stored as append-only session metadata, then evaluated after each successful
//! model turn. The evaluator runs as the goal run's node — a real agent run via the
//! multiagent runner ([`run_agent`]), tool-less, with only a bounded text transcript, so
//! the graph surface sees a live node job (transcript, interrupt/steer, GetNodeOutput).
//! It returns structured JSON; missing evidence defaults to "not done".

use std::sync::{Arc, OnceLock};

use serde::{Deserialize, Serialize};
use serde_json::json;
use strum::IntoStaticStr;
use theway_core::multiagent::graph::engine::DagEngine;
use theway_core::multiagent::graph::types::{DagStatus, RunKind};
use theway_core::multiagent::registry::AgentJobRegistry;
use theway_core::multiagent::runner::{AgentRunOptions, run_agent};
use theway_core::multiagent::types::AgentRunResolver;
use theway_core::{
    AgentHarness, AgentMessage, OnTurnEndContext, OnTurnEndHook, SessionTreeEntry, TurnEndAction,
    TurnEndDecision,
};
use theway_llm_provider::{ContentBlock, Message, UserContent, UserContentBlock};

pub const CUSTOM_TYPE: &str = "goal_state";
const TRANSCRIPT_CHAR_LIMIT: usize = 40_000;
pub const MAX_CONTINUATIONS: u32 = 8;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, IntoStaticStr)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum GoalStatus {
    Pursuing,
    Paused,
    Achieved,
    BudgetLimited,
    Cleared,
}

impl GoalStatus {
    pub fn as_str(&self) -> &'static str {
        self.into()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GoalState {
    pub condition: String,
    pub status: GoalStatus,
    #[serde(default)]
    pub iterations: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_reason: Option<String>,
    pub updated_at: String,
}

impl GoalState {
    pub fn active(&self) -> bool {
        matches!(
            self.status,
            GoalStatus::Pursuing | GoalStatus::Paused | GoalStatus::BudgetLimited
        )
    }
}

#[derive(Debug, Deserialize)]
struct EvaluatorDecision {
    ok: bool,
    reason: String,
}

pub async fn current(harness: &Arc<AgentHarness>) -> Option<GoalState> {
    latest_from_entries(&harness.session().entries().await.ok()?)
        .filter(|state| !matches!(state.status, GoalStatus::Cleared))
}

pub async fn set(harness: &Arc<AgentHarness>, condition: String) -> Result<GoalState, String> {
    let state = GoalState {
        condition,
        status: GoalStatus::Pursuing,
        iterations: 0,
        last_reason: None,
        updated_at: chrono::Utc::now().to_rfc3339(),
    };
    append_state(harness, &state).await?;
    Ok(state)
}

pub async fn pause(harness: &Arc<AgentHarness>) -> Result<GoalState, String> {
    let mut state = current(harness)
        .await
        .ok_or_else(|| "no active goal; set one with /goal <condition>".to_string())?;
    state.status = GoalStatus::Paused;
    state.updated_at = chrono::Utc::now().to_rfc3339();
    append_state(harness, &state).await?;
    Ok(state)
}

pub async fn resume(harness: &Arc<AgentHarness>) -> Result<GoalState, String> {
    let mut state = current(harness)
        .await
        .ok_or_else(|| "no paused goal; set one with /goal <condition>".to_string())?;
    if !matches!(state.status, GoalStatus::Paused | GoalStatus::BudgetLimited) {
        return Err("goal is not paused".into());
    }
    state.status = GoalStatus::Pursuing;
    state.updated_at = chrono::Utc::now().to_rfc3339();
    append_state(harness, &state).await?;
    Ok(state)
}

pub async fn clear(harness: &Arc<AgentHarness>) -> Result<GoalState, String> {
    let mut state = current(harness).await.unwrap_or_else(|| GoalState {
        condition: String::new(),
        status: GoalStatus::Cleared,
        iterations: 0,
        last_reason: None,
        updated_at: chrono::Utc::now().to_rfc3339(),
    });
    state.status = GoalStatus::Cleared;
    state.updated_at = chrono::Utc::now().to_rfc3339();
    append_state(harness, &state).await?;
    Ok(state)
}

async fn append_state(harness: &Arc<AgentHarness>, state: &GoalState) -> Result<(), String> {
    harness
        .session()
        .append_custom(CUSTOM_TYPE, Some(json!(state)))
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn latest_from_entries(entries: &[SessionTreeEntry]) -> Option<GoalState> {
    entries.iter().rev().find_map(|entry| {
        let SessionTreeEntry::Custom {
            custom_type, data, ..
        } = entry
        else {
            return None;
        };
        if custom_type != CUSTOM_TYPE {
            return None;
        }
        serde_json::from_value(data.clone()?).ok()
    })
}

fn transcript_from_messages(messages: &[AgentMessage], max_chars: usize) -> String {
    let mut lines = Vec::new();
    for message in messages {
        if let Some(line) = agent_message_text(message) {
            lines.push(line);
        }
    }
    let text = lines.join("\n\n");
    tail_chars(&text, max_chars)
}

fn agent_message_text(message: &AgentMessage) -> Option<String> {
    let AgentMessage::Llm(message) = message else {
        return None;
    };
    match message {
        Message::User(user) => Some(format!("User: {}", user_content_text(&user.content))),
        Message::Assistant(assistant) => {
            let text = assistant
                .content
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::Text(t) => Some(t.text.as_str()),
                    ContentBlock::Thinking(t) => Some(t.thinking.as_str()),
                    ContentBlock::ToolCall(t) => Some(t.name.as_str()),
                    ContentBlock::Image(_) => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            if text.trim().is_empty() {
                None
            } else {
                Some(format!("Assistant: {text}"))
            }
        }
        Message::ToolResult(result) => {
            let text = result
                .content
                .iter()
                .filter_map(|block| match block {
                    UserContentBlock::Text(t) => Some(t.text.as_str()),
                    UserContentBlock::Image(_) => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            if text.trim().is_empty() {
                None
            } else {
                Some(format!(
                    "ToolResult({} error={}): {text}",
                    result.tool_name, result.is_error
                ))
            }
        }
    }
}

fn user_content_text(content: &UserContent) -> String {
    match content {
        UserContent::Text(text) => text.clone(),
        UserContent::Blocks(blocks) => blocks
            .iter()
            .map(|block| match block {
                UserContentBlock::Text(t) => t.text.as_str(),
                UserContentBlock::Image(_) => "[image]",
            })
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

/// Build the runtime stop hook used by `/goal`.
///
/// The harness owns hook execution, but the hook itself needs a handle back to the live
/// harness so it can read goal state, run a tool-less evaluator, and persist the updated
/// goal state. `main.rs` fills the cell immediately after constructing the harness. The
/// DAG engine is shared with the dag_* tools: each activated goal registers one `goal-N`
/// run (single-node self-loop) whose lifecycle mirrors the goal state machine.
pub fn stop_hook(
    harness_cell: Arc<OnceLock<Arc<AgentHarness>>>,
    dag_engine: std::sync::Arc<DagEngine>,
    run_resolver: AgentRunResolver,
    registry: AgentJobRegistry,
    stream_fn: Option<theway_core::StreamFn>,
) -> OnTurnEndHook {
    Arc::new(move |ctx, cancel| {
        let harness_cell = harness_cell.clone();
        let dag_engine = dag_engine.clone();
        let run_resolver = run_resolver.clone();
        let registry = registry.clone();
        let stream_fn = stream_fn.clone();
        Box::pin(async move {
            let Some(harness) = harness_cell.get().cloned() else {
                return TurnEndDecision::from(TurnEndAction::Pause {
                    reason: "goal hook was not initialized".into(),
                });
            };
            evaluate_stop_hook(
                harness,
                dag_engine,
                run_resolver,
                registry,
                stream_fn,
                ctx,
                cancel,
            )
            .await
        })
    })
}

/// The engine run id for this session's goal (one `plan_goal` per session).
/// `Some` from the first activation on; `None` only if planning was skipped.
static GOAL_RUN: OnceLock<Option<String>> = OnceLock::new();

/// Session id from the harness storage metadata (mirrors main.rs's startup
/// read); `None` if the metadata is unavailable.
async fn session_id_from_harness(harness: &Arc<AgentHarness>) -> Option<String> {
    let metadata = harness.session().storage().get_metadata_json().await.ok()?;
    metadata
        .get("id")
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

/// Register the goal run in the engine on first activation (status turns
/// Pursuing). One run per session — the engine self-loop keeps ticking until
/// the goal terminates. If the registered run is missing (engine restore
/// dropped it / test reset), re-plan instead, reusing a live goal run of this
/// session when one exists.
async fn ensure_goal_run(
    dag_engine: &DagEngine,
    harness: &Arc<AgentHarness>,
    condition: &str,
) -> Option<String> {
    let session_id = session_id_from_harness(harness).await;
    let run = GOAL_RUN.get_or_init(|| Some(dag_engine.plan_goal(condition, session_id.clone())));
    match run {
        Some(run_id) if dag_engine.get_run(run_id).is_some() => Some(run_id.clone()),
        Some(_) => {
            let existing = dag_engine.list_runs().into_iter().find(|run| {
                run.kind == RunKind::Goal
                    && run.status == DagStatus::Running
                    && run.session_id == session_id
            });
            Some(existing.map_or_else(
                || dag_engine.plan_goal(condition, session_id.clone()),
                |run| run.id,
            ))
        }
        None => None,
    }
}

/// One loop iteration for a goal run (call after the goal state was persisted).
fn goal_tick(
    dag_engine: &DagEngine,
    run_id: Option<&str>,
    iteration: u32,
    done: bool,
    reason: Option<&str>,
) {
    if let Some(run_id) = run_id {
        dag_engine.on_goal_tick(run_id, iteration, done, reason.map(str::to_string));
    }
}

/// Terminal outcome for a paused goal (evaluator failure / cancel): cancel the
/// engine run so the transport shows a stopped goal instead of a running loop.
fn complete_goal_cancelled(dag_engine: &DagEngine, run_id: Option<&str>, reason: &str) {
    if let Some(run_id) = run_id {
        dag_engine.complete_goal(run_id, DagStatus::Cancelled, Some(reason.to_string()));
    }
}

/// Persist a paused goal + cancel the engine run; returns the Pause decision.
async fn pause_for(
    harness: &Arc<AgentHarness>,
    dag_engine: &DagEngine,
    run_id: Option<&str>,
    state: &mut GoalState,
    reason: String,
) -> TurnEndDecision {
    persist_pause(harness, state, reason.clone()).await;
    complete_goal_cancelled(dag_engine, run_id, &reason);
    pause_decision(reason, state)
}

async fn evaluate_stop_hook(
    harness: Arc<AgentHarness>,
    dag_engine: Arc<DagEngine>,
    run_resolver: AgentRunResolver,
    registry: AgentJobRegistry,
    stream_fn: Option<theway_core::StreamFn>,
    ctx: OnTurnEndContext,
    cancel: tokio_util::sync::CancellationToken,
) -> TurnEndDecision {
    let Some(mut state) = current(&harness).await else {
        return TurnEndDecision::from(TurnEndAction::Noop);
    };
    if state.status != GoalStatus::Pursuing {
        return TurnEndDecision::from(TurnEndAction::Noop);
    }

    // Activate: register the goal run in the engine (once per session).
    let run_id = ensure_goal_run(&dag_engine, &harness, &state.condition).await;

    let transcript = transcript_from_messages(&ctx.transcript, TRANSCRIPT_CHAR_LIMIT);
    let model = {
        let agent_state = harness.agent().state();
        agent_state.model.clone()
    };
    let model = match model {
        Some(model) => model,
        None => {
            let reason = "goal evaluator has no current model".to_string();
            return pause_for(&harness, &dag_engine, run_id.as_deref(), &mut state, reason).await;
        }
    };

    // The evaluator is the goal run's node: a real agent run (tool-less judge) so
    // the graph surface sees a live node job — transcript via GetNodeOutput,
    // GraphNodeInterrupt/GraphNodeSteer, retry semantics, all for free.
    let launch = match run_resolver("goal-evaluator") {
        Some(launch) => launch,
        None => {
            let reason = "no goal-evaluator spec registered (app-side agent_specs)".to_string();
            return pause_for(&harness, &dag_engine, run_id.as_deref(), &mut state, reason).await;
        }
    };
    let session_id = session_id_from_harness(&harness).await;
    let result = run_agent(AgentRunOptions {
        launch,
        tools: Vec::new(),
        prompt: evaluator_user_prompt(&state.condition, &transcript),
        model,
        stream_fn,
        timeout: None,
        thinking: Some("off".into()),
        registry,
        source: "dag".into(),
        run_id: run_id.clone(),
        node_id: Some("main".into()),
        session_id,
        cancel,
        system_prompt_extra: None,
        on_turn_end: None,
    })
    .await;
    // Link the evaluator's job to the goal node so the graph surface can pull
    // its transcript after the fact.
    if let Some(run_id) = run_id.as_deref() {
        dag_engine.on_goal_evaluator_finished(run_id, result.job_id.clone());
    }
    if result.error.as_deref() == Some("cancelled") {
        let reason = "goal evaluator cancelled".to_string();
        return pause_for(&harness, &dag_engine, run_id.as_deref(), &mut state, reason).await;
    }
    if !result.success {
        let reason = result
            .error
            .unwrap_or_else(|| "goal evaluator failed".to_string());
        return pause_for(&harness, &dag_engine, run_id.as_deref(), &mut state, reason).await;
    }
    let text = result.text;
    let decision = match parse_decision(&text) {
        Ok(decision) => decision,
        Err(reason) => {
            let reason = format!("goal evaluator failed: {reason}");
            return pause_for(&harness, &dag_engine, run_id.as_deref(), &mut state, reason).await;
        }
    };

    state.iterations = state.iterations.saturating_add(1);
    state.last_reason = Some(decision.reason.clone());
    state.updated_at = chrono::Utc::now().to_rfc3339();

    if decision.ok {
        state.status = GoalStatus::Achieved;
        persist_state_best_effort(&harness, &state).await;
        // done=true succeeds the node and completes the run — no extra complete_goal.
        goal_tick(
            &dag_engine,
            run_id.as_deref(),
            state.iterations,
            true,
            Some(&decision.reason),
        );
        return TurnEndDecision {
            action: TurnEndAction::Stop,
            payload: Some(goal_payload(&state, Some(true))),
        };
    }

    if state.iterations >= MAX_CONTINUATIONS {
        state.status = GoalStatus::BudgetLimited;
        persist_state_best_effort(&harness, &state).await;
        let reason = format!(
            "goal continuation limit reached ({MAX_CONTINUATIONS}); resume with /goal resume"
        );
        goal_tick(
            &dag_engine,
            run_id.as_deref(),
            state.iterations,
            false,
            Some(&decision.reason),
        );
        if let Some(run_id) = run_id.as_deref() {
            dag_engine.complete_goal(run_id, DagStatus::Failed, Some(reason.clone()));
        }
        return TurnEndDecision {
            action: TurnEndAction::Pause { reason },
            payload: Some(goal_payload(&state, Some(false))),
        };
    }

    persist_state_best_effort(&harness, &state).await;
    goal_tick(
        &dag_engine,
        run_id.as_deref(),
        state.iterations,
        false,
        Some(&decision.reason),
    );
    TurnEndDecision {
        action: TurnEndAction::Continue {
            prompt: continuation_prompt(&state.condition, &decision.reason),
        },
        payload: Some(goal_payload(&state, Some(false))),
    }
}

async fn persist_pause(harness: &Arc<AgentHarness>, state: &mut GoalState, reason: String) {
    state.status = GoalStatus::Paused;
    state.last_reason = Some(reason);
    state.updated_at = chrono::Utc::now().to_rfc3339();
    persist_state_best_effort(harness, state).await;
}

async fn persist_state_best_effort(harness: &Arc<AgentHarness>, state: &GoalState) {
    if let Err(e) = append_state(harness, state).await {
        tracing::warn!("persist goal state failed: {e}");
    }
}

fn pause_decision(reason: String, state: &GoalState) -> TurnEndDecision {
    TurnEndDecision {
        action: TurnEndAction::Pause { reason },
        payload: Some(goal_payload(state, None)),
    }
}

fn goal_payload(state: &GoalState, ok: Option<bool>) -> serde_json::Value {
    json!({
        "goal_status": state.status.as_str(),
        "condition": state.condition,
        "ok": ok,
        "reason": state.last_reason,
        "iterations": state.iterations,
        "max_continuations": MAX_CONTINUATIONS,
        "updated_at": state.updated_at,
    })
}

fn evaluator_user_prompt(condition: &str, transcript: &str) -> String {
    format!("Goal condition:\n{condition}\n\nConversation transcript:\n{transcript}")
}

/// The goal evaluator's system prompt. App-side specs reference this for the
/// `goal-evaluator` agent (`theway` crate's `agent_specs.rs`).
pub const fn evaluator_system_prompt() -> &'static str {
    r#"You are evaluating a stop-condition hook in theway.
Read the conversation transcript carefully, then judge whether the user-provided condition is satisfied.
You cannot call tools. Only use explicit evidence in the transcript.
Your response must be a JSON object with one of these shapes:
{"ok": true, "reason": "<quote evidence from the transcript that satisfies the condition>"}
{"ok": false, "reason": "<quote what is missing or what blocks the condition>"}
Always include a reason field, quoting specific text from the transcript whenever possible.
If the transcript does not contain clear evidence that the condition is satisfied, return {"ok": false, "reason": "insufficient evidence in transcript"}."#
}

fn parse_decision(text: &str) -> Result<EvaluatorDecision, String> {
    let trimmed = text.trim();
    let parsed = serde_json::from_str::<EvaluatorDecision>(trimmed)
        .or_else(|_| {
            let start = trimmed.find('{').ok_or(())?;
            let end = trimmed.rfind('}').ok_or(())?;
            serde_json::from_str::<EvaluatorDecision>(&trimmed[start..=end]).map_err(|_| ())
        })
        .map_err(|_| {
            format!(
                "goal evaluator returned invalid JSON: {}",
                tail_chars(trimmed, 300)
            )
        })?;
    if parsed.reason.trim().is_empty() {
        return Err("goal evaluator returned an empty reason".into());
    }
    Ok(parsed)
}

fn continuation_prompt(condition: &str, reason: &str) -> String {
    format!(
        "The current /goal is not satisfied yet.\n\nGoal condition:\n{condition}\n\nGoal evaluator says what is missing or blocking completion:\n{reason}\n\nContinue working toward the goal. Do not claim completion until the transcript contains explicit evidence that satisfies the condition."
    )
}

fn tail_chars(text: &str, max_chars: usize) -> String {
    let count = text.chars().count();
    if count <= max_chars {
        return text.to_string();
    }
    let tail = text
        .chars()
        .skip(count.saturating_sub(max_chars))
        .collect::<String>();
    format!("[transcript truncated to last {max_chars} chars]\n{tail}")
}

#[cfg(test)]
tests_bridge_macro::tests_bridge!("multiagent/goal");
