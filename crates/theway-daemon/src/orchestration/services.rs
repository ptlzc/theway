//! Process-scoped daemon services and their explicit ownership.

use std::sync::{Arc, OnceLock};

use crate::commands::CommandOutput;
use crate::session_activation::SessionActivator;
use crate::session_execution::SessionExecutionRegistry;
use crate::stream_auth::ConfiguredApiKeys;
use crate::subagent_settings::SubagentSettingsRegistry;
use crate::tgrep_server::TgrepServerRegistry;
use crate::tools::assembly::reload::ReloadRuntimeSlot;
use crate::triggers::cron::CronRegistry;
use crate::triggers::dynamic::DynamicTriggerRegistry;

/// Shared services owned by one daemon application instance.
#[derive(Clone)]
pub struct DaemonServices {
    pub(crate) command_output: CommandOutput,
    pub(crate) dynamic_triggers: DynamicTriggerRegistry,
    pub(crate) cron: CronRegistry,
    pub(crate) reload: ReloadRuntimeSlot,
    #[allow(dead_code)]
    pub(crate) session_execution: SessionExecutionRegistry,
    pub(crate) session_activator: Arc<OnceLock<Arc<SessionActivator>>>,
    /// Process-scoped `tgrep serve` registry for the built-in grep tool
    /// (issue #121): lazily spawned per project root, shared across sessions.
    pub(crate) tgrep: TgrepServerRegistry,
    /// Project-level last-set subagent model/thinking memory: one shared
    /// settings store per project root, injected into every session's
    /// `dag_plan` / `subagent` tools.
    pub(crate) subagent_settings: SubagentSettingsRegistry,
    /// Controller-provided API keys (issue #136, `config.toml [model]
    /// api_key`), keyed by provider. Consulted by the stream wrapper between
    /// the environment and `auth.json`; `Configure` updates it at runtime.
    pub(crate) configured_api_keys: ConfiguredApiKeys,
}

impl Default for DaemonServices {
    fn default() -> Self {
        #[cfg(test)]
        let dynamic_triggers = crate::triggers::global_registry().clone();
        #[cfg(not(test))]
        let dynamic_triggers = DynamicTriggerRegistry::default();

        #[cfg(test)]
        let cron = crate::triggers::global_cron_registry().clone();
        #[cfg(not(test))]
        let cron = CronRegistry::default();

        Self {
            command_output: CommandOutput::default(),
            dynamic_triggers,
            cron,
            reload: ReloadRuntimeSlot::default(),
            session_execution: SessionExecutionRegistry::default(),
            session_activator: Arc::new(OnceLock::new()),
            tgrep: TgrepServerRegistry::new(),
            subagent_settings: SubagentSettingsRegistry::new(),
            configured_api_keys: ConfiguredApiKeys::default(),
        }
    }
}

impl DaemonServices {
    pub fn new() -> Self {
        Self::default()
    }

    /// Share the controller-provided API-key overlay (issue #136) with the
    /// stream wrapper and the settings applier.
    #[must_use]
    pub(crate) fn with_configured_api_keys(mut self, keys: ConfiguredApiKeys) -> Self {
        self.configured_api_keys = keys;
        self
    }

    #[must_use]
    pub(crate) fn with_command_output(mut self, command_output: CommandOutput) -> Self {
        self.command_output = command_output;
        self
    }

    /// Disable the tgrep grep backend (issue #135): the registry reports
    /// `Missing` for every root, so `grep` always takes the walker path and no
    /// `tgrep serve` process or `.tgrep` index is created.
    #[must_use]
    pub(crate) fn with_tgrep_enabled(mut self, enabled: bool) -> Self {
        if !enabled {
            self.tgrep = TgrepServerRegistry::disabled();
        }
        self
    }
}
