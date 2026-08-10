//! Result promotion: mirrors a completed trigger's `trigger_result` back into the parent
//! session / transcript per the [`PromoteAction`] contract (RFC 1 §5.C).
//!
//! Rendering is fail-closed against a sealed allowlisted template context; every
//! promotion outcome (success / pending / skipped / failed) is emitted as an event and
//! audited as a `trigger_promotion` custom session entry.

use std::sync::Arc;

use parking_lot::Mutex;
use theway_core::agent::session::session::Session;
use theway_core::types::AgentMessage;
use theway_core::{Agent, AgentRunError, AgentState};
use theway_llm_provider::Message as PiMessage;

use crate::trigger_engine::event::{TriggerEvent, TriggerListener};
use crate::trigger_engine::types::{Trigger, TriggerSource};

use super::types::PromoteAction;
use super::utils::emit_from_listeners;

/// Inputs allowlisted for the promotion template per RFC 1 §5.C. Constructed once per
/// promotion and exposed to the renderer as a sealed map; references to anything not in
/// this set fail the render (fail-closed).
fn build_template_context(
    trace_id: &str,
    trigger: &Trigger,
    success: bool,
    summary: &Option<String>,
    message_count: usize,
) -> std::collections::HashMap<String, String> {
    use std::collections::HashMap;
    let mut ctx: HashMap<String, String> = HashMap::new();
    ctx.insert("trace_id".into(), trace_id.to_string());
    let (source_kind_str, source_server, source_method, source_subkind) = match &trigger.source {
        TriggerSource::Mcp {
            server_name,
            method,
        } => (
            "mcp".to_string(),
            Some(server_name.clone()),
            Some(method.clone()),
            None,
        ),
        TriggerSource::Local { subkind } => {
            ("local".to_string(), None, None, Some(subkind.clone()))
        }
        TriggerSource::AgentDelegate { .. } => ("agent_delegate".to_string(), None, None, None),
    };
    ctx.insert("trigger.source.kind".into(), source_kind_str);
    if let Some(v) = source_server {
        ctx.insert("trigger.source.server_name".into(), v);
    }
    if let Some(v) = source_method {
        ctx.insert("trigger.source.method".into(), v);
    }
    if let Some(v) = source_subkind {
        ctx.insert("trigger.source.subkind".into(), v);
    }
    ctx.insert("trigger.source_label".into(), trigger.source_label.clone());
    ctx.insert("trigger.event_label".into(), trigger.event_label.clone());
    if let Some(s) = &trigger.payload_summary {
        ctx.insert("trigger.payload_summary".into(), s.clone());
    } else {
        ctx.insert("trigger.payload_summary".into(), String::new());
    }
    ctx.insert(
        "trigger.received_at".into(),
        trigger.received_at.to_rfc3339(),
    );
    ctx.insert(
        "trigger.idempotency_key".into(),
        trigger.idempotency_key.clone(),
    );
    ctx.insert(
        "trigger.authority.principal_id".into(),
        trigger.authority.principal_id.clone(),
    );
    ctx.insert(
        "trigger.authority.principal_label".into(),
        trigger.authority.principal_label.clone(),
    );
    ctx.insert(
        "trigger.authority.credential_scope".into(),
        format!("{:?}", trigger.authority.credential_scope),
    );
    ctx.insert("result.summary".into(), summary.clone().unwrap_or_default());
    ctx.insert(
        "result.status".into(),
        if success { "success" } else { "failed" }.into(),
    );
    ctx.insert("result.message_count".into(), message_count.to_string());
    ctx.insert("result.cost_usd".into(), "null".into());
    ctx.insert("result.branch_id".into(), "null".into());
    ctx
}

/// Forbidden field references — referencing any of these via `{{name}}` in a promotion
/// template fails the render at validation time (independent of whether the field happens
/// to exist in the allowlist). RFC 1 §5.C: explicitly redacted boundary.
const FORBIDDEN_TEMPLATE_FIELDS: &[&str] = &[
    "trigger.payload",
    "trigger.authority.allowed_source_actions",
];

#[derive(Debug, PartialEq, Eq)]
enum TemplateRenderError {
    UnknownField(String),
    ForbiddenField(String),
}

/// Render a promotion template against the allowlisted context. Returns
/// `Err(TemplateRenderError::UnknownField | ForbiddenField)` on any unknown or forbidden
/// `{{...}}` reference (fail-closed; the caller must NOT insert anything on Err).
///
/// Whitespace inside `{{...}}` is tolerated (`{{ trace_id }}` works). `_meta.*` references
/// are treated as unknown (the only metadata channel adapters have today flows through
/// `trigger.payload_summary` per PR #56's privacy contract; bypassing that is forbidden).
fn render_promotion_template(
    body: &str,
    ctx: &std::collections::HashMap<String, String>,
) -> Result<String, TemplateRenderError> {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(open) = rest.find("{{") {
        out.push_str(&rest[..open]);
        let after_open = &rest[open + 2..];
        let close = after_open.find("}}").ok_or_else(|| {
            TemplateRenderError::UnknownField("unclosed `{{` placeholder".to_string())
        })?;
        let raw_name = &after_open[..close];
        let name = raw_name.trim();
        if FORBIDDEN_TEMPLATE_FIELDS.contains(&name) || name.starts_with("_meta") {
            return Err(TemplateRenderError::ForbiddenField(name.to_string()));
        }
        let value = ctx
            .get(name)
            .ok_or_else(|| TemplateRenderError::UnknownField(name.to_string()))?;
        out.push_str(value);
        rest = &after_open[close + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Built-in fallback template used when `PromoteSummaryNow { template: None }`.
const DEFAULT_PROMOTE_SUMMARY_TEMPLATE: &str = "[Trigger {{trace_id}}] {{trigger.source_label}} fired {{trigger.event_label}}.\nResult: {{result.summary}}";

/// Same byte cap used for `result.summary` truncation; applied to the rendered promotion
/// body so a runaway template (e.g. summary already at cap + verbose template body) cannot
/// inflate the parent transcript beyond the 4 KiB boundary per RFC 1 §5.B.
pub(super) const PROMOTION_BODY_CAP_BYTES: usize = 4096;

/// Truncate a promotion body to the byte cap on a UTF-8 char boundary. Returns the new
/// string and `truncated: bool`. Walk-back ensures `truncate` never panics on a
/// multi-byte char.
/// Stable hex-encoded SHA-256 of the template body. Used only as a content fingerprint in
/// the `trigger_promotion` audit so RFC 4 rule edits / template version bumps are
/// detectable from JSONL log re-reads. Not used as a credential / authentication
/// primitive — see `sha2` dep comment in `Cargo.toml`.
pub(super) fn sha256_hex(input: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    let out = hasher.finalize();
    // Lowercase hex; the first 8 chars are sliced off by callers for the `inline:` name.
    let mut s = String::with_capacity(out.len() * 2);
    for byte in out.iter() {
        use std::fmt::Write;
        let _ = write!(&mut s, "{byte:02x}");
    }
    s
}

/// Enforce the `[Trigger {trace_id}] ` disambiguation prefix on a promotion body. Per
/// @Tools-MCP-Lead's PR #65 review: trusting template authors to include the prefix is
/// unsafe — a custom template that forgets it would produce a `Message::User` in the
/// parent transcript that looks like human input, polluting the next-turn LLM context
/// without user awareness. Idempotent only for the **current** trace id: if the body
/// already begins with `[Trigger {trace_id}] ` (the form the engine would produce), the
/// prefix is not re-added. A `[Trigger evil] ` prefix carrying a different trace id is
/// NOT trusted — the engine still prepends the real `[Trigger {trace_id}] ` so the
pub(super) fn ensure_trigger_prefix(body: String, trace_id: &str) -> (String, bool) {
    let expected = format!("[Trigger {trace_id}] ");
    if body.starts_with(&expected) {
        (body, false)
    } else {
        (format!("{expected}{body}"), true)
    }
}

/// Truncation marker appended to bodies that overrun `cap_bytes`. Counted toward the cap
/// so the final string length is `<= cap_bytes`.
const TRUNCATION_MARKER: &str = "…[truncated]";

/// Truncate `body` to fit within `cap_bytes` *including* the truncation marker. The body
/// portion is cut on a UTF-8 char boundary so `truncate` never panics on a multi-byte
/// codepoint. The final length is at most `cap_bytes`: we reserve
/// `TRUNCATION_MARKER.len()` from the budget before the boundary walk.
pub(super) fn truncate_on_char_boundary(body: String, cap_bytes: usize) -> (String, bool) {
    if body.len() <= cap_bytes {
        return (body, false);
    }
    // Reserve room for the marker so the final string fits the cap. If the cap is
    // somehow smaller than the marker, fall back to "marker-only" output.
    let budget = cap_bytes.saturating_sub(TRUNCATION_MARKER.len());
    let mut cut = budget.min(body.len());
    while cut > 0 && !body.is_char_boundary(cut) {
        cut -= 1;
    }
    let mut truncated = body;
    truncated.truncate(cut);
    truncated.push_str(TRUNCATION_MARKER);
    (truncated, true)
}

/// Apply the trigger's [`PromoteAction`] after the sub-agent has finished and the
/// `trigger_result` audit was written. RFC 1 §5.C — implements the v1 promotion variants
/// `None` (no-op) and `PromoteSummaryNow { template }` (templated insertion into the
/// parent session; fail-closed on render error; pending state when
/// `promote_requires_approval = true`).
#[allow(clippy::too_many_arguments)]
pub(super) async fn apply_promotion(
    listeners: &Arc<Mutex<Vec<TriggerListener>>>,
    parent_session: &Session,
    parent_agent: &Arc<Agent>,
    trace_id: &str,
    trigger: &Trigger,
    success: bool,
    summary: &Option<String>,
    message_count: usize,
    _failure_reason: Option<&str>,
    promote: &PromoteAction,
    require_approval: bool,
    details: &serde_json::Value,
) {
    // Extract the inline template body (if any). v1 does not look up named templates from
    // any registry; that lands in sub-PR 6 / RFC 4 rule engine work. The body is what we
    // render against — never persisted as `template_name` in the audit.
    let (template_body_arg, promote_kind): (Option<String>, &'static str) = match promote {
        PromoteAction::None => return, // most common path; nothing else to do
        PromoteAction::PromoteSummaryNow { template_body } => {
            (template_body.clone(), "promote_summary_now")
        }
        #[allow(deprecated)]
        PromoteAction::PromoteSummaryWhenSummaryContains {
            template_body,
            required_substrings,
        } => {
            let summary_text = summary.as_deref().unwrap_or_default();
            if !required_substrings
                .iter()
                .any(|needle| summary_text.contains(needle))
            {
                return;
            }
            (template_body.clone(), "promote_summary_now")
        }
        PromoteAction::PromoteSummaryWhenResultDetailsMatch {
            template_body,
            condition,
        } => {
            // Authorization gate. The sub-agent's `summary` is NEVER consulted — promotion
            // fires only when the structured `details` blob satisfies `condition`. Any
            // failure (pointer missing, value not an array, empty intersection) emits a
            // `trigger_promotion { state: "skipped", reason }` audit and returns without
            // touching the parent transcript.
            match condition.evaluate(details) {
                Ok(_matched) => (
                    template_body.clone(),
                    "promote_summary_when_result_details_match",
                ),
                Err(reason) => {
                    let audit_data = serde_json::json!({
                        "state": "skipped",
                        "trace_id": trace_id,
                        "promote_kind": "promote_summary_when_result_details_match",
                        "reason": reason.as_audit_str(),
                        "template_name": serde_json::Value::Null,
                        "template_hash": serde_json::Value::Null,
                        "inserted_entry_id": serde_json::Value::Null,
                        "rule_id": serde_json::Value::Null,
                        "redaction_status": "skipped",
                        "dedup_collapsed": false,
                        "prefix_injected": false,
                    });
                    let _ = parent_session
                        .append_custom("trigger_promotion", Some(audit_data))
                        .await;
                    return;
                }
            }
        }
    };

    // Build the sealed allowlisted template context once. Anything not in here is unknown
    // to the renderer; anything explicitly forbidden fails before substitution.
    let ctx = build_template_context(trace_id, trigger, success, summary, message_count);

    // Resolve the body to render: explicit if provided, otherwise the built-in default.
    // Both flow through the same renderer (per Provider/Auth: no fixed-summary insertion
    // path that bypasses sanitization).
    let body_template: &str = template_body_arg
        .as_deref()
        .unwrap_or(DEFAULT_PROMOTE_SUMMARY_TEMPLATE);

    // `template_name` / `template_hash` for audit + events: stable identifier + content
    // fingerprint per @Tools-MCP-Lead's PR #65 follow-up. v1 categories:
    // - `"default"` when no inline body was provided
    // - `"inline:{hash[..8]}"` when the hook supplied a literal body
    // - (future) `"rules.{rule_id}.template"` when RFC 4 rule engine names a template
    // Provider/Auth blocker: the raw body is NEVER stored as `template_name`.
    let template_hash = sha256_hex(body_template);
    let template_name = match &template_body_arg {
        None => "default".to_string(),
        Some(_) => format!("inline:{}", &template_hash[..8]),
    };
    let template_name = Some(template_name);
    let template_hash = Some(template_hash);

    let rendered = match render_promotion_template(body_template, &ctx) {
        Ok(s) => s,
        Err(err) => {
            // Render failure → fail-closed. Write a `trigger_promotion { state: "failed" }`
            // audit so jsonl-only readers can see what happened, and emit a
            // `PersistenceError` reflux so live subscribers know promotion was lost.
            let redaction_status = match &err {
                TemplateRenderError::UnknownField(_) => "render_error",
                TemplateRenderError::ForbiddenField(_) => "forbidden_field",
            };
            let err_msg = match &err {
                TemplateRenderError::UnknownField(name) => {
                    format!("unknown template field: {name}")
                }
                TemplateRenderError::ForbiddenField(name) => {
                    format!("forbidden template field: {name}")
                }
            };
            let audit_data = serde_json::json!({
                "state": "failed",
                "trace_id": trace_id,
                "promote_kind": promote_kind,
                "template_name": template_name,
                "template_hash": template_hash,
                "inserted_entry_id": serde_json::Value::Null,
                "rule_id": serde_json::Value::Null,
                "redaction_status": redaction_status,
                "dedup_collapsed": false,
                // Render failed before the prefix step ran; record false so the audit shape
                // stays uniform across all promotion states.
                "prefix_injected": false,
            });
            if let Err(e) = parent_session
                .append_custom("trigger_promotion", Some(audit_data))
                .await
            {
                emit_from_listeners(
                    listeners,
                    TriggerEvent::PersistenceError {
                        context: "trigger_promotion".into(),
                        message: format!("trigger_promotion (failed) append failed: {:?}", e.code),
                    },
                );
            }
            emit_from_listeners(
                listeners,
                TriggerEvent::PersistenceError {
                    context: "trigger_promotion".into(),
                    message: err_msg,
                },
            );
            return;
        }
    };

    // Per @Tools-MCP-Lead's PR #65 review: enforce the `[Trigger {trace_id}] ` prefix at
    // the engine level instead of trusting the template author to include it. A custom
    // template that forgets the prefix would otherwise produce a parent-session
    // `Message::User` that looks indistinguishable from human input, polluting the
    // next-turn LLM context without user awareness. Idempotent: if the rendered body
    // already starts with `[Trigger ` (e.g. the built-in default template), the prefix
    // is not added twice.
    let (rendered, prefix_injected) = ensure_trigger_prefix(rendered, trace_id);

    // Pending path: render succeeded so we have a preview, but `promote_requires_approval`
    // is true and there is no `/triggers approve` command in v1 — fail-closed-to-pending.
    if require_approval {
        let (preview, truncated) =
            truncate_on_char_boundary(rendered.clone(), PROMOTION_BODY_CAP_BYTES);
        let redaction_status = if truncated { "truncated" } else { "clean" };
        let audit_data = serde_json::json!({
            "state": "pending",
            "trace_id": trace_id,
            "promote_kind": promote_kind,
            "template_name": template_name,
            "template_hash": template_hash,
            "inserted_entry_id": serde_json::Value::Null,
            "rule_id": serde_json::Value::Null,
            "redaction_status": redaction_status,
            "dedup_collapsed": false,
            "prefix_injected": prefix_injected,
        });
        if let Err(e) = parent_session
            .append_custom("trigger_promotion", Some(audit_data))
            .await
        {
            emit_from_listeners(
                listeners,
                TriggerEvent::PersistenceError {
                    context: "trigger_promotion".into(),
                    message: format!("trigger_promotion (pending) append failed: {:?}", e.code),
                },
            );
        }
        emit_from_listeners(
            listeners,
            TriggerEvent::PromotionPending {
                trace_id: trace_id.to_string(),
                promote_kind: promote_kind.into(),
                template_name,
                preview: Some(preview),
            },
        );
        return;
    }

    // Success path: render OK, no approval gate → insert into parent transcript.
    // theway_llm_provider has no `Message::System` role; use `Message::User` with the rendered body.
    // The engine-injected `[Trigger {trace_id}] ` prefix (above) guarantees the appended
    // entry is visually disambiguated from human input regardless of which template was
    // used.
    let (final_body, truncated) = truncate_on_char_boundary(rendered, PROMOTION_BODY_CAP_BYTES);
    let redaction_status = if truncated { "truncated" } else { "clean" };

    let user_message = AgentMessage::Llm(PiMessage::User(theway_llm_provider::UserMessage {
        role: theway_llm_provider::UserRole::User,
        content: theway_llm_provider::UserContent::Text(final_body),
        timestamp: chrono::Utc::now().timestamp_millis(),
    }));

    // Single persistence path. The promoted message must land in the session JSONL exactly
    // once, with deterministic ordering relative to any in-flight assistant response. Two
    // disjoint branches based on parent loop state:
    //
    // - **Streaming**: parent has an active prompt. Hand the message to the loop's
    //   follow-up queue. The loop drains it at the next turn boundary (after the in-flight
    //   assistant response has emitted its `MessageEnd` and been persisted by the session
    //   listener), pushes it into `state.messages`, and emits a `MessageEnd` whose session
    //   listener writes the single canonical session entry. Order in JSONL: assistant
    //   response → user_promoted, matching what the model actually saw. We do NOT call
    //   `parent_session.append_message` here — that would double-persist and land in the
    //   wrong order. Audit captures the queued state; `inserted_entry_id` is only known
    //   after the loop drains, so it's `Null` here and correlated via `trace_id`.
    //
    // - **Idle**: no active loop, no listener race. Synchronously
    //   `parent_session.append_message` (single write) then push to `state.messages` so
    //   the user's next `prompt()` / `continue_()` sees the promotion without an explicit
    //   rehydrate. Loop isn't running, so no `MessageEnd` fires for this message → no
    //   duplicate listener write.
    let queued_for_followup = parent_agent.is_streaming();
    let (audit_state, inserted_entry_id_value, inserted_entry_id_str) = if queued_for_followup {
        parent_agent.enqueue_follow_up(user_message);
        (
            "queued",
            serde_json::Value::Null,
            String::new(), // event field is set; TUI / /triggers audit join by trace_id
        )
    } else {
        let id = match parent_session.append_message(user_message.clone()).await {
            Ok(id) => id,
            Err(e) => {
                emit_from_listeners(
                    listeners,
                    TriggerEvent::PersistenceError {
                        context: "trigger_promotion".into(),
                        message: format!("promotion message append failed: {:?}", e.code),
                    },
                );
                // Audit the failure so jsonl-only readers know promotion attempted but
                // was lost.
                let audit_data = serde_json::json!({
                    "state": "failed",
                    "trace_id": trace_id,
                    "promote_kind": promote_kind,
                    "template_name": template_name,
                    "template_hash": template_hash,
                    "inserted_entry_id": serde_json::Value::Null,
                    "rule_id": serde_json::Value::Null,
                    "redaction_status": "render_error",
                    "dedup_collapsed": false,
                    "prefix_injected": prefix_injected,
                });
                let _ = parent_session
                    .append_custom("trigger_promotion", Some(audit_data))
                    .await;
                return;
            }
        };
        parent_agent.state().messages.push(user_message);
        ("success", serde_json::Value::String(id.clone()), id)
    };

    let audit_data = serde_json::json!({
        "state": audit_state,
        "trace_id": trace_id,
        "promote_kind": promote_kind,
        "template_name": template_name,
        "template_hash": template_hash,
        "inserted_entry_id": inserted_entry_id_value,
        "rule_id": serde_json::Value::Null,
        "redaction_status": redaction_status,
        "dedup_collapsed": false,
        "prefix_injected": prefix_injected,
    });
    if let Err(e) = parent_session
        .append_custom("trigger_promotion", Some(audit_data))
        .await
    {
        emit_from_listeners(
            listeners,
            TriggerEvent::PersistenceError {
                context: "trigger_promotion".into(),
                message: format!(
                    "trigger_promotion ({audit_state}) append failed: {:?}",
                    e.code
                ),
            },
        );
    }
    emit_from_listeners(
        listeners,
        TriggerEvent::TriggerPromoted {
            trace_id: trace_id.to_string(),
            promote_kind: promote_kind.into(),
            inserted_entry_id: inserted_entry_id_str,
            template_name,
            redaction_status: redaction_status.into(),
        },
    );
}

/// Inspect the sub-agent's terminal state to summarize the outcome. Returns
/// `(success, summary, message_count)`.
///
/// `summary` is the text of the sub-agent's final assistant message when one exists; this
/// is a first-cut heuristic for 5a. Sub-PR 5b can replace this with a model-driven summary
/// or a hook-supplied template-rendered summary.
pub(super) fn compute_sub_agent_outcome(
    sub_agent: &Agent,
    run_outcome: &Result<(), AgentRunError>,
) -> (bool, Option<String>, usize) {
    if let Err(_e) = run_outcome {
        // Try to grab a partial last-assistant-message even on failure for context.
        let state = sub_agent.state();
        let last = last_assistant_text(&state);
        return (false, last, state.messages.len());
    }
    let state = sub_agent.state();
    let summary = last_assistant_text(&state);
    (true, summary, state.messages.len())
}

/// Extract the text of the last assistant message, if any. Returns `None` if the agent
/// produced no assistant content (e.g. aborted before the first turn). Truncated to 4 KiB
/// per RFC 1 §5.B size cap.
fn last_assistant_text(state: &AgentState) -> Option<String> {
    let last = state.messages.iter().rev().find_map(|m| match m {
        AgentMessage::Llm(theway_llm_provider::Message::Assistant(a)) => Some(a),
        _ => None,
    })?;
    let mut text = String::new();
    for block in &last.content {
        if let theway_llm_provider::ContentBlock::Text(t) = block {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&t.text);
        }
    }
    if text.is_empty() {
        return None;
    }
    const SUMMARY_CAP_BYTES: usize = 4096;
    // Per @QA-Release-Lead's PR #65 review: cap must include the truncation marker so
    // the final body fits the documented 4 KiB boundary. Reuse the shared helper for
    // consistency between `trigger_result.summary` and promotion body truncation.
    let (capped, _truncated) = truncate_on_char_boundary(text, SUMMARY_CAP_BYTES);
    Some(capped)
}
