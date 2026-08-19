// ──────────────────────────────────────────────────────────────────────────
// Runtime state externalization (issue #84): serde twins of `state.proto`
// `StorageService` messages. These are the portable wire forms used by the
// JSON-RPC state methods, the gRPC `StorageService` codecs (`crate::state`),
// and the [`crate::transport::StorageOps`] handler seam the daemon implements
// behind the `RuntimeStorage` boundary.
// ──────────────────────────────────────────────────────────────────────────

/// One persisted DAG run: `snapshot` is the portable JSON `PersistedRun`
/// shape used by GraphEngineService checkpoint/restore.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WireStoredDagRun {
    pub session_id: String,
    pub run_id: String,
    pub snapshot: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WireSaveDagRunRequest {
    pub session_id: String,
    pub run_id: String,
    pub snapshot: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WireSaveDagRunResult {
    pub saved: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WireLoadDagRunsRequest {
    pub session_id: String,
    /// `None` = load every stored run for the session; `Some` = load one run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WireLoadDagRunsResult {
    pub runs: Vec<WireStoredDagRun>,
}

/// Session-scoped dynamic trigger rule in portable storage form.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WireStoredTriggerRule {
    pub id: String,
    pub condition: String,
    pub action: String,
    pub enabled: bool,
    #[serde(default = "default_fire_once")]
    pub fire_once: bool,
    /// RFC3339, when the rule has fired.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fired_at: Option<String>,
    #[serde(default)]
    pub promote_to_chat: bool,
    /// RFC3339.
    pub created_at: String,
}

impl Default for WireStoredTriggerRule {
    fn default() -> Self {
        Self {
            id: String::new(),
            condition: String::new(),
            action: String::new(),
            enabled: false,
            fire_once: true,
            fired_at: None,
            promote_to_chat: false,
            created_at: String::new(),
        }
    }
}

fn default_fire_once() -> bool {
    true
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WireSaveTriggerRulesRequest {
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<WireStoredTriggerRule>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WireSaveTriggerRulesResult {
    pub count: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WireLoadTriggerRulesRequest {
    pub session_id: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WireLoadTriggerRulesResult {
    pub rules: Vec<WireStoredTriggerRule>,
}

/// Session-scoped cron job in portable storage form.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WireStoredCronJob {
    pub id: String,
    /// Standard 5-field cron expression.
    pub schedule: String,
    pub action: String,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub running_trace_id: Option<String>,
    /// RFC3339.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_due_at: Option<String>,
    /// RFC3339.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_fired_at: Option<String>,
    /// RFC3339.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_completed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(default)]
    pub skipped_overlap_count: u64,
    #[serde(default)]
    pub stateful: bool,
    /// RFC3339.
    pub created_at: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WireSaveCronJobsRequest {
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub jobs: Vec<WireStoredCronJob>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WireSaveCronJobsResult {
    pub count: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WireLoadCronJobsRequest {
    pub session_id: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WireLoadCronJobsResult {
    pub jobs: Vec<WireStoredCronJob>,
}

#[derive(Clone, Debug, Serialize)]
pub struct WireMcpSnapshot {
    pub servers: usize,
    pub tools: usize,
    pub notification_hooks: usize,
    pub server_names: Vec<String>,
    pub tool_names: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct WireToolsSnapshot {
    pub total: usize,
    pub names: Vec<String>,
}
