//! Session-scoped automation data models — trigger rules cross the wire and into
//! session archives, so they are contract. Kept in this pure leaf crate so the
//! daemon, transport and storage can share them without layering on each other.
//!
//! Pure serde models (no trigger-engine logic): the daemon's
//! `triggers::{cron, dynamic}` modules re-export them, `session_archive`
//! serializes them into `.theway-session` sidecars, and
//! `theway_transport::triggers` keeps re-exporting them for API
//! compatibility.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Default local dynamic-trigger poll interval (seconds).
pub const DEFAULT_DYNAMIC_TRIGGER_POLL_INTERVAL_SECS: u64 = 10 * 60;

/// A cron job persisted in the session-scoped `cron.toml` sidecar.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CronJob {
    pub id: String,
    /// Standard 5-field cron expression: minute hour day-of-month month day-of-week.
    pub schedule: String,
    pub action: String,
    pub enabled: bool,
    #[serde(default)]
    pub running_trace_id: Option<String>,
    #[serde(default)]
    pub last_due_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub last_fired_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub last_completed_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub skipped_overlap_count: u64,
    /// Loop mode (issue #23): run in a new sub-agent with persistent cross-run state
    /// and the inbox output protocol instead of injecting into the parent conversation.
    #[serde(default)]
    pub stateful: bool,
    pub created_at: DateTime<Utc>,
}

/// A dynamic trigger rule persisted in the session-scoped `triggers.json` sidecar.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DynamicTriggerRule {
    pub id: String,
    pub condition: String,
    pub action: String,
    pub enabled: bool,
    #[serde(default = "default_fire_once")]
    pub fire_once: bool,
    #[serde(default)]
    pub fired_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub promote_to_chat: bool,
    pub created_at: DateTime<Utc>,
}

fn default_fire_once() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dynamic_trigger_rule_defaults_fire_once_when_absent() {
        let rule: DynamicTriggerRule = serde_json::from_str(
            r#"{"id":"t1","condition":"c","action":"a","enabled":true,"created_at":"2026-01-01T00:00:00Z"}"#,
        )
        .unwrap();
        assert!(rule.fire_once);
        assert_eq!(rule.fired_at, None);
        assert!(!rule.promote_to_chat);
    }

    #[test]
    fn cron_job_round_trips_and_defaults_optional_fields() {
        let created = Utc::now();
        let job = CronJob {
            id: "c1".into(),
            schedule: "*/5 * * * *".into(),
            action: "do it".into(),
            enabled: true,
            running_trace_id: None,
            last_due_at: None,
            last_fired_at: None,
            last_completed_at: None,
            last_error: None,
            skipped_overlap_count: 0,
            stateful: false,
            created_at: created,
        };
        let json = serde_json::to_string(&job).unwrap();
        assert_eq!(serde_json::from_str::<CronJob>(&json).unwrap(), job);

        // Sidecars written before the optional fields existed still parse.
        let legacy: CronJob = serde_json::from_str(&format!(
            r#"{{"id":"c2","schedule":"0 0 * * *","action":"a","enabled":false,"created_at":"{created}"}}"#
        ))
        .unwrap();
        assert_eq!(legacy.skipped_overlap_count, 0);
        assert!(!legacy.stateful);
    }
}
