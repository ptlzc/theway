//! Cron error types, the 5-field schedule parser, and control-plane audit/render helpers.

use std::collections::BTreeSet;

use chrono::{DateTime, Datelike, Local, Timelike, Utc};
use serde_json::{Value, json};
use theway_core::AgentToolError;

use super::{CronJob, CronJobExt, HarnessCell, MAX_ACTION_PREVIEW_CHARS};

#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
pub enum AddCronJobError {
    #[error("cron action cannot be empty")]
    EmptyAction,
    #[error("cron action exceeds {max_bytes} bytes")]
    ActionTooLarge { max_bytes: usize },
    #[error("{0}")]
    Schedule(#[from] CronScheduleError),
    #[error("{0}")]
    Storage(#[from] CronStorageError),
}

#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
pub enum CronStorageError {
    #[error("cron storage io: {0}")]
    Io(String),
    #[error("parse cron storage: {0}")]
    Parse(String),
    #[error("serialize cron storage: {0}")]
    Serialize(String),
    #[error("{0}")]
    Schedule(#[from] CronScheduleError),
}

#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
pub enum CronScheduleError {
    #[error("cron schedule must have 5 fields: minute hour day-of-month month day-of-week")]
    WrongFieldCount,
    #[error("invalid cron field `{field}`: {reason}")]
    InvalidField { field: String, reason: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CronExpression {
    pub(super) minutes: BTreeSet<u32>,
    pub(super) hours: BTreeSet<u32>,
    pub(super) days_of_month: BTreeSet<u32>,
    pub(super) months: BTreeSet<u32>,
    pub(super) days_of_week: BTreeSet<u32>,
}

impl CronExpression {
    pub(super) fn parse(input: &str) -> Result<Self, CronScheduleError> {
        let parts: Vec<_> = input.split_whitespace().collect();
        if parts.len() != 5 {
            return Err(CronScheduleError::WrongFieldCount);
        }
        Ok(Self {
            minutes: parse_field(parts[0], 0, 59)?,
            hours: parse_field(parts[1], 0, 23)?,
            days_of_month: parse_field(parts[2], 1, 31)?,
            months: parse_field(parts[3], 1, 12)?,
            days_of_week: parse_day_of_week(parts[4])?,
        })
    }

    fn matches(&self, dt: DateTime<Utc>) -> bool {
        let local = dt.with_timezone(&Local);
        self.minutes.contains(&local.minute())
            && self.hours.contains(&local.hour())
            && self.days_of_month.contains(&local.day())
            && self.months.contains(&local.month())
            && self
                .days_of_week
                .contains(&local.weekday().num_days_from_sunday())
    }

    pub(super) fn next_after(&self, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let mut candidate = after + chrono::Duration::minutes(1);
        candidate = candidate.with_second(0)?.with_nanosecond(0)?;
        let limit = after + chrono::Duration::days(366 * 5);
        while candidate <= limit {
            if self.matches(candidate) {
                return Some(candidate);
            }
            candidate += chrono::Duration::minutes(1);
        }
        None
    }
}

fn parse_day_of_week(field: &str) -> Result<BTreeSet<u32>, CronScheduleError> {
    let mut set = parse_field(field, 0, 7)?;
    if set.remove(&7) {
        set.insert(0);
    }
    Ok(set)
}

fn parse_field(field: &str, min: u32, max: u32) -> Result<BTreeSet<u32>, CronScheduleError> {
    let mut out = BTreeSet::new();
    for part in field.split(',') {
        let part = part.trim();
        if part.is_empty() {
            return Err(CronScheduleError::InvalidField {
                field: field.into(),
                reason: "empty item".into(),
            });
        }
        let (range_part, step) = match part.split_once('/') {
            Some((range, step)) => {
                let step = step
                    .parse::<u32>()
                    .map_err(|_| CronScheduleError::InvalidField {
                        field: field.into(),
                        reason: "step must be a positive integer".into(),
                    })?;
                if step == 0 {
                    return Err(CronScheduleError::InvalidField {
                        field: field.into(),
                        reason: "step must be at least 1".into(),
                    });
                }
                (range, step)
            }
            None => (part, 1),
        };
        let (start, end) = if range_part == "*" {
            (min, max)
        } else if let Some((start, end)) = range_part.split_once('-') {
            (
                parse_number(field, start, min, max)?,
                parse_number(field, end, min, max)?,
            )
        } else {
            let value = parse_number(field, range_part, min, max)?;
            (value, value)
        };
        if start > end {
            return Err(CronScheduleError::InvalidField {
                field: field.into(),
                reason: "range start must be <= range end".into(),
            });
        }
        for value in (start..=end).step_by(step as usize) {
            out.insert(value);
        }
    }
    Ok(out)
}

fn parse_number(field: &str, raw: &str, min: u32, max: u32) -> Result<u32, CronScheduleError> {
    let value = raw
        .parse::<u32>()
        .map_err(|_| CronScheduleError::InvalidField {
            field: field.into(),
            reason: format!("`{raw}` is not a number"),
        })?;
    if !(min..=max).contains(&value) {
        return Err(CronScheduleError::InvalidField {
            field: field.into(),
            reason: format!("value {value} outside {min}-{max}"),
        });
    }
    Ok(value)
}

pub(super) fn normalize_schedule(input: &str) -> Result<String, AgentToolError> {
    let trimmed = input.trim();
    if CronExpression::parse(trimmed).is_ok() {
        return Ok(trimmed.to_string());
    }

    let normalized = trimmed.to_lowercase();
    let alias = match normalized.as_str() {
        "hourly" | "every hour" | "once an hour" => Some("0 * * * *"),
        "daily" | "every day" | "once a day" => Some("0 9 * * *"),
        "weekly" | "every week" | "once a week" => Some("0 9 * * 1"),
        _ => {
            if trimmed.contains("每小时") || trimmed.contains("每個小時") {
                Some("0 * * * *")
            } else if trimmed.contains("每天") || trimmed.contains("每日") {
                Some("0 9 * * *")
            } else if trimmed.contains("每周") || trimmed.contains("每週") {
                Some("0 9 * * 1")
            } else {
                None
            }
        }
    };
    alias.map(str::to_string).ok_or_else(|| {
        AgentToolError::Message(
            "invalid schedule: provide a 5-field cron expression, or a supported alias such as hourly / every hour / 每小时"
                .into(),
        )
    })
}

pub fn cron_control_plane_audit(
    op: &str,
    actor: &str,
    before: Option<&CronJob>,
    after: Option<&CronJob>,
) -> Value {
    let job = after.or(before);
    let now = Utc::now();
    json!({
        "op": op,
        "actor": actor,
        "job_id": job.map(|job| job.id.as_str()),
        "schedule": job.map(|job| job.schedule.as_str()),
        "action_preview": job.map(|job| preview_redacted(&job.action, MAX_ACTION_PREVIEW_CHARS)),
        "before_enabled": before.map(|job| job.enabled),
        "after_enabled": after.map(|job| job.enabled),
        "next_run": after
            .filter(|job| job.enabled)
            .and_then(|job| job.next_run_after(now))
            .map(|dt| dt.to_rfc3339()),
        "removed": before.is_some() && after.is_none(),
    })
}

pub(super) async fn write_tool_cron_control_audit(
    harness: &Option<HarnessCell>,
    op: &str,
    before: Option<&CronJob>,
    after: Option<&CronJob>,
) -> Option<String> {
    let harness = harness.as_ref().and_then(|cell| cell.get())?;
    let audit = cron_control_plane_audit(op, "tool", before, after);
    match harness
        .session()
        .append_custom("cron_control_plane", Some(audit))
        .await
    {
        Ok(id) => Some(id),
        Err(e) => {
            let job = after.or(before);
            tracing::warn!(
                op,
                actor = "tool",
                job_id = job.map(|job| job.id.as_str()),
                error = %e,
                "cron_control_plane audit write failed; tool cron change itself succeeded"
            );
            None
        }
    }
}

pub(super) fn render_cron_jobs_for_tool(jobs: &[CronJob]) -> String {
    if jobs.is_empty() {
        return "session cron jobs: none".into();
    }

    let now = Utc::now();
    let mut lines = vec![format!("session cron jobs: {}", jobs.len())];
    for job in jobs {
        let state = if job.enabled { "enabled" } else { "disabled" };
        lines.push(format!(
            "- {} [{}] schedule: {} action: {}",
            job.id,
            state,
            job.schedule,
            preview_redacted(&job.action, MAX_ACTION_PREVIEW_CHARS)
        ));
        if let Some(next_run) = job
            .enabled
            .then(|| job.next_run_after(now))
            .flatten()
            .map(|dt| dt.to_rfc3339())
        {
            lines.push(format!("  next_run: {next_run}"));
        }
        if let Some(trace_id) = &job.running_trace_id {
            lines.push(format!("  running_trace_id: {trace_id}"));
        }
        if let Some(last_error) = &job.last_error {
            lines.push(format!(
                "  last_error: {}",
                preview_redacted(last_error, MAX_ACTION_PREVIEW_CHARS)
            ));
        }
        if job.skipped_overlap_count > 0 {
            lines.push(format!(
                "  skipped_overlap_count: {}",
                job.skipped_overlap_count
            ));
        }
    }
    lines.join("\n")
}

pub(super) fn cron_job_details_for_model(job: &CronJob) -> Value {
    let now = Utc::now();
    json!({
        "id": job.id,
        "schedule": job.schedule,
        "action_preview": preview_redacted(&job.action, MAX_ACTION_PREVIEW_CHARS),
        "enabled": job.enabled,
        "scope": "session",
        "running_trace_id": job.running_trace_id,
        "last_due_at": job.last_due_at.map(|dt| dt.to_rfc3339()),
        "last_fired_at": job.last_fired_at.map(|dt| dt.to_rfc3339()),
        "last_completed_at": job.last_completed_at.map(|dt| dt.to_rfc3339()),
        "last_error": job
            .last_error
            .as_ref()
            .map(|err| preview_redacted(err, MAX_ACTION_PREVIEW_CHARS)),
        "skipped_overlap_count": job.skipped_overlap_count,
        "next_run": job
            .enabled
            .then(|| job.next_run_after(now))
            .flatten()
            .map(|dt| dt.to_rfc3339()),
        "created_at": job.created_at.to_rfc3339(),
    })
}

pub(super) fn preview_redacted(input: &str, max_chars: usize) -> String {
    preview(&crate::bug_report::redact(input), max_chars)
}

fn preview(input: &str, max_chars: usize) -> String {
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
