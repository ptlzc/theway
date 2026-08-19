//! Process-scoped daemon services and their explicit ownership.

use crate::commands::CommandOutput;
use crate::tools::assembly::reload::ReloadRuntimeSlot;
use crate::triggers::cron::CronRegistry;
use crate::triggers::dynamic::DynamicTriggerRegistry;

/// Shared services owned by one daemon application instance.
#[derive(Clone)]
pub struct DaemonServices {
    pub command_output: CommandOutput,
    pub dynamic_triggers: DynamicTriggerRegistry,
    pub cron: CronRegistry,
    pub reload: ReloadRuntimeSlot,
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
        }
    }
}

impl DaemonServices {
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_command_output(mut self, command_output: CommandOutput) -> Self {
        self.command_output = command_output;
        self
    }
}
