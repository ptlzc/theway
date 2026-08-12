//! Cron notification hook (the tick loop that emits due triggers) plus the action hook and
//! trigger listener that map accepted cron triggers into agent turns.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::trigger_engine::event::{TriggerEvent, TriggerListener};
use crate::trigger_engine::execution::{
    BeforeTriggerActionContext, BeforeTriggerActionHook, PromoteAction, TriggerAction,
    TriggerDelivery,
};
use crate::trigger_engine::notification_hook::{
    HookError, HookState, NotificationHook, NotificationHookStatus, TriggerSink,
};
use crate::trigger_engine::types::{
    CredentialScope, PayloadVisibility, ReplacementPolicy, SourceKind, Trigger, TriggerAuthority,
    TriggerSource,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde_json::json;
use tokio::time::{Duration, MissedTickBehavior};
use tokio_util::sync::CancellationToken;

use super::errors::preview_redacted;
use super::{CronJob, CronRegistry, MAX_ACTION_PREVIEW_CHARS};

const CRON_SUBKIND: &str = "cron";
const TICK_SECS: u64 = 30;

pub struct CronNotificationHook {
    registry: CronRegistry,
    status: Arc<Mutex<NotificationHookStatus>>,
}

impl CronNotificationHook {
    pub fn new(registry: CronRegistry) -> Self {
        let mut status = NotificationHookStatus::pending();
        status.subscription_labels = vec!["local crontab".into()];
        Self {
            registry,
            status: Arc::new(Mutex::new(status)),
        }
    }
}

#[async_trait]
impl NotificationHook for CronNotificationHook {
    fn label(&self) -> &str {
        "cron"
    }

    async fn run(&self, sink: TriggerSink) -> Result<(), HookError> {
        {
            let mut status = self.status.lock();
            status.state = HookState::Connected;
            status.last_error = None;
        }
        let mut last_scan = Utc::now();
        let mut interval = tokio::time::interval(Duration::from_secs(TICK_SECS));
        interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
        interval.tick().await;
        loop {
            interval.tick().await;
            let now = Utc::now();
            for (job, due_at) in self.registry.due_jobs(last_scan, now) {
                let Some(trace_id) = job.running_trace_id.clone() else {
                    continue;
                };
                let trigger = cron_trigger_for_job(&job, due_at, trace_id);
                if sink.send(trigger).is_err() {
                    let mut status = self.status.lock();
                    status.state = HookState::Disconnected {
                        reason: "sink closed".into(),
                    };
                    status.last_error = Some("sink closed".into());
                    return Err(HookError::SinkClosed);
                }
                let mut status = self.status.lock();
                status.last_event_at = Some(now);
            }
            last_scan = now;
        }
    }

    fn status(&self) -> NotificationHookStatus {
        let mut status = self.status.lock().clone();
        let jobs = self.registry.list();
        status.queued_count = jobs
            .iter()
            .filter(|job| job.running_trace_id.is_some())
            .count() as u64;
        status.subscription_labels = if jobs.is_empty() {
            vec!["local crontab: 0 jobs".into()]
        } else {
            vec![format!(
                "local crontab: {} job(s), {} enabled",
                jobs.len(),
                jobs.iter().filter(|job| job.enabled).count()
            )]
        };
        status
    }
}

pub(super) fn cron_trigger_for_job(
    job: &CronJob,
    due_at: DateTime<Utc>,
    trace_id: String,
) -> Trigger {
    Trigger {
        source: TriggerSource::Local {
            subkind: CRON_SUBKIND.into(),
        },
        source_kind: SourceKind::Local,
        source_label: "Cron".into(),
        event_label: job.id.clone(),
        payload_visibility: PayloadVisibility::Local,
        payload_summary: Some(format!(
            "cron `{}` due at {}: {}",
            job.id,
            due_at.to_rfc3339(),
            preview_redacted(&job.action, MAX_ACTION_PREVIEW_CHARS)
        )),
        payload: Some(json!({
            "job_id": job.id,
            "due_at": due_at.to_rfc3339(),
        })),
        idempotency_key: format!("cron:{}:{}", job.id, due_at.to_rfc3339()),
        replacement_policy: ReplacementPolicy::Drop,
        trace_id,
        authority: TriggerAuthority {
            principal_id: "local-cron".into(),
            principal_label: "local cron".into(),
            credential_scope: CredentialScope::None,
            allowed_source_actions: Vec::new(),
            expires_at: None,
        },
        received_at: Utc::now(),
    }
}

/// Cap on persisted loop state.
const LOOP_STATE_MAX_CHARS: usize = 2000;
/// At most this many `<inbox>` findings are honored per run.
const INBOX_TAGS_PER_RUN: usize = 16;

/// `<sess>.cron.toml` + `cron-abcdef…` → `<sess>.loop-cron-abcdef12.md` (8-char id prefix
/// after the `cron-` marker keeps names short but unambiguous in practice).
pub(crate) fn loop_state_path(cron_sidecar: &Path, job_id: &str) -> PathBuf {
    let stem = cron_sidecar
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_suffix(".cron.toml"))
        .unwrap_or("session");
    let short: String = job_id.chars().take(13).collect(); // "cron-" + 8 hex
    let file = format!("{stem}.loop-{short}.md");
    cron_sidecar.with_file_name(file)
}

pub(crate) fn read_loop_state(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok().map(|text| {
        let trimmed = text.trim();
        if trimmed.chars().count() > LOOP_STATE_MAX_CHARS {
            let mut capped: String = trimmed.chars().take(LOOP_STATE_MAX_CHARS).collect();
            capped.push('…');
            capped
        } else {
            trimmed.to_string()
        }
    })
}

pub(crate) fn write_loop_state(path: &Path, state: &str) -> std::io::Result<()> {
    let trimmed = state.trim();
    let capped: String = if trimmed.chars().count() > LOOP_STATE_MAX_CHARS {
        let mut capped: String = trimmed.chars().take(LOOP_STATE_MAX_CHARS).collect();
        capped.push('…');
        capped
    } else {
        trimmed.to_string()
    };
    std::fs::write(path, capped)
}

/// Assemble the stateful-loop prompt: previous state, the job's action, and the output
/// protocol the listener parses afterwards.
pub(crate) fn compose_stateful_prompt(action: &str, state: Option<&str>) -> String {
    format!(
        "[loop-state] (your notes from the previous run of this recurring job)\n{}\n[/loop-state]\n\n{}\n\nOutput protocol (mandatory):\n- End your reply with <loop-state>notes for the next run</loop-state> — it REPLACES the saved state; keep it under 2000 characters and make it the information your next run needs (baselines, ids already seen, watermarks).\n- For each finding a human should act on, emit <inbox>one concise line</inbox>. No findings → no inbox tags; do not invent work.\n- Keep everything after the last tool call short so the tags are not truncated.",
        state.unwrap_or("(first run)"),
        action
    )
}

/// Remove `<loop-state>`/`<inbox>` protocol blocks for display: the listener persists
/// them; UI lines should show only the human-facing remainder.
pub fn strip_loop_protocol_tags(text: &str) -> String {
    let mut out = String::from(text);
    let mut stripped = false;
    for tag in ["loop-state", "inbox"] {
        let open = format!("<{tag}>");
        let close = format!("</{tag}>");
        while let Some(start) = out.find(&open) {
            let Some(end_rel) = out[start..].find(&close) else {
                break;
            };
            out.replace_range(start..start + end_rel + close.len(), "");
            stripped = true;
        }
    }
    if !stripped {
        // No protocol tags: leave the text untouched so multi-line summaries render as-is.
        return out;
    }
    // Collapse the blank residue the removed blocks leave behind.
    let mut result = String::new();
    let mut prev_blank = false;
    for line in out.lines().map(str::trim_end) {
        let blank = line.trim().is_empty();
        if blank && prev_blank {
            continue;
        }
        if !result.is_empty() {
            result.push('\n');
        }
        result.push_str(line);
        prev_blank = blank;
    }
    result.trim().to_string()
}

/// Last `<tag>…</tag>` block in `text`, trimmed. Unclosed tags are ignored.
pub(crate) fn extract_tag_block(text: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = text.rfind(&open)?;
    let rest = &text[start + open.len()..];
    let end = rest.find(&close)?;
    Some(rest[..end].trim().to_string())
}

/// Every `<tag>…</tag>` block in order, capped at `max`.
pub(crate) fn extract_tag_all(text: &str, tag: &str, max: usize) -> Vec<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut out = Vec::new();
    let mut rest = text;
    while out.len() < max {
        let Some(start) = rest.find(&open) else { break };
        let after = &rest[start + open.len()..];
        let Some(end) = after.find(&close) else { break };
        let body = after[..end].trim();
        if !body.is_empty() {
            out.push(body.to_string());
        }
        rest = &after[end + close.len()..];
    }
    out
}

pub fn cron_action_hook(
    registry: CronRegistry,
    inner: BeforeTriggerActionHook,
) -> BeforeTriggerActionHook {
    Arc::new(
        move |ctx: BeforeTriggerActionContext, cancel: CancellationToken| {
            let registry = registry.clone();
            let is_cron = matches!(
                &ctx.trigger.source,
                TriggerSource::Local { subkind } if subkind == CRON_SUBKIND
            );
            if !is_cron {
                return inner(ctx, cancel);
            }
            let job_id = ctx
                .trigger
                .payload
                .as_ref()
                .and_then(|payload| payload.get("job_id"))
                .and_then(|v| v.as_str())
                .map(str::to_string);
            Box::pin(async move {
                let Some(job_id) = job_id else {
                    return TriggerAction::default_for(&ctx.trigger);
                };
                let Some(job) = registry.list().into_iter().find(|job| job.id == job_id) else {
                    return TriggerAction::default_for(&ctx.trigger);
                };
                if job.stateful {
                    // Loop mode (issue #23): fresh sub-agent, state injected, findings
                    // routed to the inbox by the harness listener — the main
                    // conversation is never interrupted.
                    let state = registry
                        .storage_path()
                        .map(|sidecar| loop_state_path(&sidecar, &job.id))
                        .and_then(|path| read_loop_state(&path));
                    return TriggerAction {
                        prompt: compose_stateful_prompt(&job.action, state.as_deref()),
                        promote: PromoteAction::None,
                        promote_requires_approval: false,
                        delivery: TriggerDelivery::SubAgent,
                    };
                }
                TriggerAction {
                    prompt: job.action,
                    promote: PromoteAction::None,
                    promote_requires_approval: false,
                    delivery: TriggerDelivery::InjectAndRun,
                }
            })
        },
    )
}

pub fn cron_trigger_listener(registry: CronRegistry, inbox_path: PathBuf) -> TriggerListener {
    Arc::new(move |event| match event {
        TriggerEvent::TriggerCompleted {
            trace_id, summary, ..
        } => {
            // Resolve the job BEFORE mark_completed clears the trace binding.
            let job = registry.job_for_trace(&trace_id);
            registry.mark_completed(&trace_id, None);
            let (Some(job), Some(summary)) = (job, summary) else {
                return;
            };
            if !job.stateful {
                return;
            }
            if let Some(state) = extract_tag_block(&summary, "loop-state")
                && let Some(sidecar) = registry.storage_path()
            {
                let path = loop_state_path(&sidecar, &job.id);
                if let Err(err) = write_loop_state(&path, &state) {
                    tracing::warn!(error = %err, job = %job.id, "loop state write failed");
                }
            }
            let session_stem = registry
                .storage_path()
                .and_then(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .and_then(|n| n.strip_suffix(".cron.toml"))
                        .map(str::to_string)
                })
                .unwrap_or_default();
            let source = format!("cron:{}", job.id.chars().take(13).collect::<String>());
            for finding in extract_tag_all(&summary, "inbox", INBOX_TAGS_PER_RUN) {
                if let Err(err) =
                    crate::inbox::append(&inbox_path, &source, &finding, &trace_id, &session_stem)
                {
                    tracing::warn!(error = %err, "inbox append failed");
                }
            }
        }
        TriggerEvent::TriggerFailed { trace_id, reason } => {
            registry.mark_completed(&trace_id, Some(reason.clone()));
        }
        _ => {}
    })
}
