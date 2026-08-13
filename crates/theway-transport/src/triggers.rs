//! Session-scoped automation data models (daemon-kernel-layers: moved from the
//! SDK into transport — trigger rules cross the wire, so they are contract).
//!
//! Pure serde models (no trigger-engine logic): the daemon's `triggers::{cron,
//! dynamic}` modules re-export them, and `session_archive` serializes them into
//! `.theway-session` sidecars.

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
    /// Loop mode (issue #23): run in a fresh sub-agent with persistent cross-run state
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
