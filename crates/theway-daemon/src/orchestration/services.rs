//! Process-scoped daemon services and their explicit ownership.

use std::sync::{Arc, OnceLock};

use crate::commands::CommandOutput;
use crate::session_activation::SessionActivator;
use crate::session_execution::SessionExecutionRegistry;
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
        }
    }
}

impl DaemonServices {
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub(crate) fn with_command_output(mut self, command_output: CommandOutput) -> Self {
        self.command_output = command_output;
        self
    }
}
