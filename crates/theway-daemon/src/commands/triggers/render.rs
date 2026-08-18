//! Render and audit helpers shared by the automation commands (`/triggers`, `/cron`,
//! `/inbox`).

use super::*;

pub(crate) fn render_cron_jobs(jobs: &[crate::triggers::cron::CronJob]) -> Vec<String> {
    if jobs.is_empty() {
        return vec!["Cron jobs (session): none".into()];
    }
    let mut lines = vec![format!("Cron jobs (session, {}):", jobs.len())];
    for job in jobs {
        let state = if job.enabled { "enabled" } else { "disabled" };
        let running = job
            .running_trace_id
            .as_ref()
            .map(|trace| format!(", running {trace}"))
            .unwrap_or_default();
        let stateful = if job.stateful { "  [stateful]" } else { "" };
        lines.push(format!(
            "  {}  {}  {}{}{}",
            job.id, state, job.schedule, stateful, running
        ));
        lines.push(format!("    action: {}", preview_cron_action(&job.action)));
        if job.skipped_overlap_count > 0 {
            lines.push(format!("    overlap skips: {}", job.skipped_overlap_count));
        }
        if let Some(err) = &job.last_error {
            lines.push(format!("    last: {err}"));
        } else if let Some(last) = job.last_fired_at {
            lines.push(format!("    last fired: {}", last.to_rfc3339()));
        }
    }
    lines
}

pub(super) fn preview_cron_action(action: &str) -> String {
    preview_cron_text(&crate::bug_report::redact(action), 120)
}

fn preview_cron_text(input: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (idx, ch) in input.chars().enumerate() {
        if idx == max_chars {
            out.push('…');
            return out;
        }
        out.push(ch);
    }
    out
}

pub(crate) fn render_triggers_status(snapshot: &NotificationStatusSnapshot) -> Vec<String> {
    let mut lines = Vec::new();
    let runtime = snapshot.runtime;
    let dynamic_rules = crate::triggers::global_registry().list();
    let enabled_count = dynamic_rules.iter().filter(|rule| rule.enabled).count();
    let disabled_count = dynamic_rules.len().saturating_sub(enabled_count);
    let fire_once_count = dynamic_rules.iter().filter(|rule| rule.fire_once).count();
    let repeat_count = dynamic_rules.len().saturating_sub(fire_once_count);
    let promote_count = dynamic_rules
        .iter()
        .filter(|rule| rule.promote_to_chat)
        .count();
    lines.push("Trigger status:".into());
    lines.push(format!(
        "  dynamic rules: {} total, {} enabled, {} disabled ({} fire_once, {} repeat, {} promote_to_chat)",
        dynamic_rules.len(),
        enabled_count,
        disabled_count,
        fire_once_count,
        repeat_count,
        promote_count
    ));
    let dynamic_checker_count = snapshot
        .hooks
        .iter()
        .filter(|hook| {
            hook.subscription_labels
                .iter()
                .any(|label| label.contains("dynamic trigger periodic check"))
        })
        .count();
    let notification_hook_count = snapshot.hooks.len().saturating_sub(dynamic_checker_count);
    lines.push(format!(
        "  local dynamic checker: {} registered, polls every {}s while enabled rules exist",
        dynamic_checker_count,
        crate::triggers::dynamic::dynamic_trigger_poll_interval_secs()
    ));
    lines.push(format!(
        "  push trigger sources: {} configured source(s) feed server-pushed events into the same trigger runtime",
        notification_hook_count
    ));
    let storage = crate::triggers::global_registry()
        .storage_path()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "memory".into());
    lines.push(format!("  storage: {storage}"));
    lines.push("  output: default is TUI + audit only; rules marked promote_to_chat also enter the main chat context".into());
    lines.push(format!(
        "  engine: accepted={} deduped={} cycle_suppressed={} recent_traces={} dedup_entries={} running={}",
        runtime.accepted_total,
        runtime.deduped_total,
        runtime.cycle_suppressed_total,
        runtime.active_traces,
        runtime.dedup_entries,
        snapshot.running.len()
    ));
    let attention_count = snapshot
        .hooks
        .iter()
        .filter(|h| h.requires_attention.is_some())
        .count();
    let connected_count = snapshot
        .hooks
        .iter()
        .filter(|h| matches!(h.state, HookState::Connected))
        .count();
    lines.push(format!(
        "  sources: {} total, {} connected, {} require attention",
        snapshot.hooks.len(),
        connected_count,
        attention_count
    ));
    lines.extend(
        render_dynamic_trigger_rules(&dynamic_rules, 3)
            .into_iter()
            .skip(1),
    );
    lines.push(
        "  commands: /triggers rules | /triggers sources | /triggers disable <id> | /triggers enable <id> | /triggers remove <id> | /triggers audit".into(),
    );
    lines
}

pub(crate) fn render_dynamic_trigger_rules(
    rules: &[crate::triggers::dynamic::DynamicTriggerRule],
    limit: usize,
) -> Vec<String> {
    if rules.is_empty() {
        return vec!["Dynamic trigger rules: none".into()];
    }
    let shown = rules.len().min(limit);
    let mut lines = vec![format!("Dynamic trigger rules ({}):", rules.len())];
    for rule in rules.iter().take(shown) {
        let state = if rule.enabled { "enabled" } else { "disabled" };
        let fire_mode = if rule.fire_once {
            "fire_once"
        } else {
            "repeat"
        };
        let output_mode = if rule.promote_to_chat {
            "promote_to_chat"
        } else {
            "audit_only"
        };
        lines.push(format!(
            "  - {} [{state}, {fire_mode}, {output_mode}{}] when {} -> {}",
            rule.id,
            rule.fired_at
                .map(|at| format!(", fired_at={}", at.to_rfc3339()))
                .unwrap_or_default(),
            preview_text(&rule.condition, 80),
            preview_text(&rule.action, 80)
        ));
    }
    if shown < rules.len() {
        lines.push(format!(
            "  ... {} more; run /triggers rules",
            rules.len() - shown
        ));
    }
    lines
}

pub(in crate::commands) fn render_trigger_sources(hooks: &[NotificationHookStatus]) -> Vec<String> {
    if hooks.is_empty() {
        return vec!["(no trigger sources registered)".into()];
    }
    let mut lines = vec![format!("Trigger sources ({}):", hooks.len())];
    for (idx, hook) in hooks.iter().enumerate() {
        let labels = if hook.subscription_labels.is_empty() {
            "subscriptions: none".into()
        } else {
            format!("subscriptions: {}", hook.subscription_labels.join(", "))
        };
        lines.push(format!(
            "  - source #{}: {} queued={} dropped={} deduped={} last_event={}{}",
            idx + 1,
            render_hook_state(&hook.state),
            hook.queued_count,
            hook.dropped_count,
            hook.deduped_count,
            hook.last_event_at
                .map(|d| d.to_rfc3339())
                .unwrap_or_else(|| "never".into()),
            render_requires_attention(hook)
        ));
        lines.push(format!("      {labels}"));
        if let Some(err) = &hook.last_error {
            lines.push(format!("      last error: {}", preview_text(err, 160)));
        }
    }
    lines
}

fn render_hook_state(state: &HookState) -> String {
    match state {
        HookState::Connected => "connected".into(),
        HookState::Reconnecting => "reconnecting".into(),
        HookState::Disconnected { reason } => {
            format!("disconnected ({})", preview_text(reason, 80))
        }
        HookState::Disabled => "disabled".into(),
        HookState::AuthFailed { reason } => format!("auth_failed ({})", preview_text(reason, 80)),
    }
}

fn render_requires_attention(hook: &NotificationHookStatus) -> String {
    hook.requires_attention
        .as_ref()
        .map(|message| format!("  attention: {}", preview_text(message, 120)))
        .unwrap_or_default()
}

pub(in crate::commands) fn render_running_triggers(running: &[RunningTriggerState]) -> Vec<String> {
    if running.is_empty() {
        return vec!["(no running triggers)".into()];
    }
    let mut lines = vec![format!("Running triggers ({}):", running.len())];
    for trigger in running {
        lines.push(format!(
            "  - {}  {} / {}  since {}",
            trigger.trace_id,
            trigger.source_label,
            trigger.event_label,
            trigger.started_at.to_rfc3339()
        ));
        lines.push(format!(
            "      prompt: {}",
            preview_text(&trigger.prompt_preview, 120)
        ));
    }
    lines
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::commands) struct TriggerAuditRow {
    pub(in crate::commands) custom_type: String,
    pub(in crate::commands) timestamp: String,
    pub(in crate::commands) trace_id: Option<String>,
    pub(in crate::commands) state: String,
    pub(in crate::commands) source_label: Option<String>,
    pub(in crate::commands) event_label: Option<String>,
    pub(in crate::commands) summary: Option<String>,
    pub(in crate::commands) details: Vec<String>,
}

pub(in crate::commands) fn collect_trigger_audit_rows(
    entries: &[SessionTreeEntry],
    limit: usize,
) -> Vec<TriggerAuditRow> {
    entries
        .iter()
        .rev()
        .filter_map(trigger_audit_row)
        .take(limit)
        .collect()
}

fn trigger_audit_row(entry: &SessionTreeEntry) -> Option<TriggerAuditRow> {
    let SessionTreeEntry::Custom {
        timestamp,
        custom_type,
        data,
        ..
    } = entry
    else {
        return None;
    };
    if !matches!(
        custom_type.as_str(),
        "trigger" | "trigger_result" | "trigger_promotion"
    ) {
        return None;
    }
    let data = data.as_ref()?;
    let trace_id = string_field(data, "trace_id");
    let state = match custom_type.as_str() {
        "trigger" => string_field(data, "state").unwrap_or_else(|| "unknown".into()),
        "trigger_result" => match data.get("success").and_then(|v| v.as_bool()) {
            Some(true) => "completed".into(),
            Some(false) => "failed".into(),
            None => "unknown".into(),
        },
        "trigger_promotion" => string_field(data, "state").unwrap_or_else(|| "unknown".into()),
        _ => "unknown".into(),
    };
    let summary = match custom_type.as_str() {
        "trigger" => string_field(data, "payload_summary"),
        "trigger_result" => string_field(data, "summary").or_else(|| string_field(data, "reason")),
        "trigger_promotion" => {
            string_field(data, "redaction_status").map(|s| format!("redaction_status={s}"))
        }
        _ => None,
    };
    let details = match custom_type.as_str() {
        "trigger" => trigger_decision_details(data),
        "trigger_result" => trigger_result_details(data),
        "trigger_promotion" => trigger_promotion_details(data),
        _ => Vec::new(),
    };
    Some(TriggerAuditRow {
        custom_type: custom_type.clone(),
        timestamp: timestamp.clone(),
        trace_id,
        state,
        source_label: string_field(data, "source_label"),
        event_label: string_field(data, "event_label"),
        summary,
        details,
    })
}

pub(in crate::commands) fn render_trigger_audit(rows: &[TriggerAuditRow]) -> Vec<String> {
    if rows.is_empty() {
        return vec!["(no trigger audit entries in this session)".into()];
    }
    let mut lines = vec![format!("Recent trigger audit ({}):", rows.len())];
    for row in rows {
        let trace = row.trace_id.as_deref().unwrap_or("unknown-trace");
        let source = row.source_label.as_deref().unwrap_or("-");
        let event = row.event_label.as_deref().unwrap_or("-");
        lines.push(format!(
            "  - {}  {}/{}  trace={}  {} / {}",
            row.timestamp, row.custom_type, row.state, trace, source, event
        ));
        if let Some(summary) = &row.summary {
            lines.push(format!("      {}", preview_text(summary, 160)));
        }
        for detail in &row.details {
            lines.push(format!("      {detail}"));
        }
    }
    lines
}

pub(in crate::commands) fn trigger_decision_details(data: &serde_json::Value) -> Vec<String> {
    let Some(decision) = data.get("evaluator_decision") else {
        return Vec::new();
    };
    let Some(outcome) = string_field(decision, "outcome") else {
        return vec!["decision: present".into()];
    };
    let mut fields = vec![format!("decision: {outcome}")];
    match outcome.as_str() {
        "accept" => {
            if let Some(permission) = string_field(decision, "permission") {
                fields.push(format!("permission: {}", preview_text(&permission, 80)));
            }
            if let Some(reason) = string_field(decision, "reason") {
                fields.push(format!("reason: {}", preview_text(&reason, 160)));
            }
        }
        "deduped" => {
            if let Some(previous) = string_field(decision, "previous_trace_id") {
                fields.push(format!(
                    "previous_trace_id: {}",
                    preview_text(&previous, 80)
                ));
            }
            if let Some(policy) = string_field(decision, "replacement_policy") {
                fields.push(format!("replacement_policy: {}", preview_text(&policy, 80)));
            }
        }
        "cycle_suppressed" => {
            if let Some(hops) = number_field(decision, "hop_count") {
                fields.push(format!("hop_count: {hops}"));
            }
        }
        _ => {}
    }
    fields
}

fn trigger_result_details(data: &serde_json::Value) -> Vec<String> {
    let mut fields = Vec::new();
    if let Some(branch_id) = string_field(data, "branch_id") {
        fields.push(format!("branch_id: {}", preview_text(&branch_id, 80)));
    }
    if let Some(count) = number_field(data, "message_count") {
        fields.push(format!("message_count: {count}"));
    }
    fields
}

fn trigger_promotion_details(data: &serde_json::Value) -> Vec<String> {
    let mut fields = Vec::new();
    if let Some(kind) = string_field(data, "promote_kind") {
        fields.push(format!("promote_kind: {}", preview_text(&kind, 80)));
    }
    if let Some(inserted) = string_field(data, "inserted_entry_id") {
        fields.push(format!(
            "inserted_entry_id: {}",
            preview_text(&inserted, 80)
        ));
    }
    fields
}

fn string_field(data: &serde_json::Value, name: &str) -> Option<String> {
    data.get(name)
        .and_then(|v| v.as_str())
        .map(ToString::to_string)
}

fn number_field(data: &serde_json::Value, name: &str) -> Option<u64> {
    data.get(name).and_then(|v| v.as_u64())
}

#[cfg(test)]
tests_bridge_macro::tests_bridge!("commands/triggers/render");
