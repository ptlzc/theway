//! Free helper functions of the harness layer: system-prompt assembly, the session
//! persistence listener, audit-label capping, banner previews, and user-prompt text
//! extraction. Split out of `assembly/mod.rs` by domain.

use std::sync::Arc;

use parking_lot::Mutex;
use theway_llm_provider::Message as PiMessage;

use crate::agent::session::session::Session;
use crate::agent::system_prompt::format_skills_for_system_prompt;
use crate::agent::types::Skill;
use crate::agent::{AgentEvent, AgentRunError};
use crate::types::AgentMessage;

pub(super) fn build_system_prompt(base: &str, skills: &[Skill]) -> String {
    let skills_block = format_skills_for_system_prompt(skills);
    if base.is_empty() {
        return skills_block;
    }
    if skills_block.is_empty() {
        return base.to_string();
    }
    format!("{base}\n\n{skills_block}")
}

/// Build an `AgentListener` that persists every emitted `MessageEnd` to the session log.
pub(super) fn make_session_listener(
    session: Session,
) -> (
    crate::agent::AgentListener,
    Arc<Mutex<Vec<crate::agent::types::SessionError>>>,
) {
    let errors = Arc::new(Mutex::new(Vec::new()));
    let listener_errors = errors.clone();
    let listener: crate::agent::AgentListener = Arc::new(move |event, _cancel| {
        let session = session.clone();
        let listener_errors = listener_errors.clone();
        Box::pin(async move {
            match event {
                AgentEvent::MessageEnd { message } => {
                    if let Err(e) = session.append_message(message).await {
                        listener_errors.lock().push(e);
                    }
                }
                AgentEvent::ControlPlanePromptResolved {
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

pub(super) fn finish_persisted_run(
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

/// Emit a [`super::HarnessEvent`] to a snapshot of the listener registry, isolating each listener
/// with `catch_unwind` so a single panicking listener cannot poison the others. Mirrors
/// the contract of `AgentHarness::emit_harness_event` but operates on a cloned `Arc` of
/// listeners (so the spawned sub-agent task does not need an `AgentHarness` reference).
pub(super) fn preview_for_banner(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max_chars).collect();
    out.push('…');
    out
}

/// Extract the text body of a `Message::User`, joining `Blocks` text content. Returns
/// `None` for image-only messages or empty text. Used to fill
/// [`super::OnTurnEndContext::last_user_prompt`] for the most recent user message in the
/// transcript.
pub(super) fn extract_user_message_text(u: &theway_llm_provider::UserMessage) -> Option<String> {
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
/// content. Used to fill [`super::OnTurnEndContext::last_user_prompt`] for the freshly-arrived
/// user prompt before the transcript has been mutated.
pub(super) fn extract_user_prompt_text(msg: &AgentMessage) -> Option<String> {
    match msg {
        AgentMessage::Llm(PiMessage::User(u)) => extract_user_message_text(u),
        _ => None,
    }
}
