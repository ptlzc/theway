use std::sync::Arc;

use parking_lot::Mutex;
use theway_llm_provider::Model;

use crate::agent::session::session::{BranchSummaryInput, Session};
use crate::agent::{AgentRunError, LoopEvent};
use crate::types::ThinkingLevel;

use super::{AgentHarness, SessionEvent};

impl AgentHarness {
    /// Switch model and persist the choice so resumed sessions restore it.
    pub async fn set_model(
        &self,
        model: Model,
    ) -> Result<String, crate::agent::types::SessionError> {
        let provider = model.provider.0.clone();
        let model_id = model.id.clone();
        let id = self.session.append_model_change(provider, model_id).await?;
        self.agent.state().model = Some(model);
        Ok(id)
    }

    pub async fn set_thinking_level(
        &self,
        level: ThinkingLevel,
    ) -> Result<String, crate::agent::types::SessionError> {
        let id = self
            .session
            .append_thinking_level_change(level.as_str())
            .await?;
        self.agent.state().thinking_level = Some(level);
        Ok(id)
    }

    /// Move the session leaf and replay the selected branch into the in-memory agent state.
    pub async fn move_to(
        &self,
        entry_id: Option<&str>,
        summary: Option<BranchSummaryInput>,
    ) -> Result<Option<String>, crate::agent::types::SessionError> {
        let from = self.session.leaf_id().await.ok().flatten();
        let result = self.session.move_to(entry_id, summary).await?;
        self.rehydrate_from_session().await?;
        self.emit_harness_event(SessionEvent::Branch {
            from_entry_id: from,
            to_entry_id: entry_id.map(str::to_string),
            summary_entry_id: result.clone(),
        });
        Ok(result)
    }

    /// Replace in-memory state with the active session branch.
    pub async fn rehydrate_from_session(
        &self,
    ) -> Result<crate::agent::session::session::SessionContext, crate::agent::types::SessionError>
    {
        let context = self.session.build_context().await?;
        let mut state = self.agent.state();
        state.messages = context.messages.clone();
        if let Some(model) = &context.model
            && let Some(restored) = theway_llm_provider::get_model(
                &theway_llm_provider::Provider::from(model.provider.clone()),
                &model.model_id,
            )
        {
            state.model = Some(restored);
        }
        if let Ok(level) = context.thinking_level.parse::<ThinkingLevel>() {
            state.thinking_level = Some(level);
        }
        Ok(context)
    }
}

pub(super) fn make_session_listener(
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
                    if let Err(error) = session.append_message(message).await {
                        listener_errors.lock().push(error);
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
                    if let Err(error) = session
                        .append_custom("control_plane_prompt", Some(data))
                        .await
                    {
                        listener_errors.lock().push(error);
                    }
                }
                _ => {}
            }
        })
    });
    (listener, errors)
}

const CONTROL_PLANE_PROMPT_LABEL_CAP_CHARS: usize = 200;

pub(super) fn cap_control_plane_audit_label(label: &str) -> String {
    if label.chars().count() <= CONTROL_PLANE_PROMPT_LABEL_CAP_CHARS {
        return label.to_string();
    }
    let mut output: String = label
        .chars()
        .take(CONTROL_PLANE_PROMPT_LABEL_CAP_CHARS.saturating_sub(1))
        .collect();
    output.push('…');
    output
}

pub(super) fn finish_persisted_run(
    result: Result<(), AgentRunError>,
    persist_errors: Arc<Mutex<Vec<crate::agent::types::SessionError>>>,
) -> Result<(), AgentRunError> {
    result?;
    if let Some(error) = persist_errors.lock().first() {
        return Err(AgentRunError::Other(format!(
            "session append message: {error}"
        )));
    }
    Ok(())
}
