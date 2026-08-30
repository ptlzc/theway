#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct WireGoalSnapshot {
    pub condition: String,
    pub status: String,
    pub iterations: u32,
    pub last_reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct WireControlPlanePromptSnapshot {
    pub tool_name: String,
    pub label: String,
    pub reason: String,
    pub args_hash: String,
    pub payload: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct WireSidebarSnapshot {
    pub inbox_new: usize,
    pub skills: WireSkillsSnapshot,
    pub triggers: WireTriggersSnapshot,
    pub cron: WireCronSnapshot,
    pub mcp: WireMcpSnapshot,
    pub tools: WireToolsSnapshot,
    pub hooks: Vec<String>,
    pub runtime: Vec<String>,
    /// Slash-prefixed file-command names discovered from `.agents/commands`
    /// and `.claude/commands` (claude-code format, issue #37).
    #[serde(default)]
    pub commands: Vec<String>,
    /// Runtime-reload epoch (issue #50): the daemon bumps this after a
    /// successful `reload` tool call; clients cache it and re-read local
    /// resources (e.g. `~/.theway/theme.toml`) when it changes. Serde
    /// default keeps older snapshots decodable.
    #[serde(default)]
    pub runtime_revision: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct WireSkillsSnapshot {
    pub total: usize,
    pub enabled: usize,
    pub disabled: usize,
    pub builtin: usize,
    pub user: usize,
    pub project: usize,
    pub items: Vec<WireSkillSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct WireSkillSnapshot {
    pub name: String,
    pub source: String,
    pub file_path: String,
    pub enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct WireTriggersSnapshot {
    pub total: usize,
    pub enabled: usize,
    pub disabled: usize,
    pub rules: Vec<WireTriggerRuleSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct WireTriggerRuleSnapshot {
    pub id: String,
    pub full_id: String,
    pub enabled: bool,
    pub mode: String,
    pub condition: String,
    pub action: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct WireCronSnapshot {
    pub total: usize,
    pub enabled: usize,
    pub disabled: usize,
    pub jobs: Vec<WireCronJobSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct WireCronJobSnapshot {
    pub id: String,
    pub enabled: bool,
    pub schedule: String,
    pub action: String,
    pub skipped_overlap_count: u64,
    pub last_error: Option<String>,
}
