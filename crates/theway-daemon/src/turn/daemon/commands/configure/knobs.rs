// `Configure` appliers for the runtime-tunable knobs and the startup-only
// fields that only report an error.
//
// `include!` fragment of the `turn::daemon` module — see `commands.rs`.

impl TurnHost {
    /// Apply the runtime-tunable knobs; reject the fields bound at startup.
    fn configure_runtime_knobs(
        &mut self,
        config: &WireDaemonConfig,
        applied: &mut WireDaemonConfig,
    ) {
        if let Some(secs) = config.trigger_poll_secs {
            if secs == 0 {
                self.error_line("configure: trigger_poll_secs must be greater than zero");
            } else {
                self.automation.services.dynamic_triggers.set_poll_interval_secs(secs);
                applied.trigger_poll_secs = Some(secs);
            }
        } else if config.clears("trigger_poll_secs") {
            self.automation.services.dynamic_triggers.set_poll_interval_secs(
                theway_transport::triggers::DEFAULT_DYNAMIC_TRIGGER_POLL_INTERVAL_SECS,
            );
            applied.clear_fields.push("trigger_poll_secs".into());
        }

        if let Some(lines) = config.tui_max_feed_lines {
            if lines == 0 {
                self.error_line("configure: tui_max_feed_lines must be greater than zero");
            } else {
                self.runtime.feed_history_limit = Some(lines);
                applied.tui_max_feed_lines = Some(lines);
            }
        } else if config.clears("tui_max_feed_lines") {
            self.runtime.feed_history_limit = None;
            applied.clear_fields.push("tui_max_feed_lines".into());
        }

        if let Some(addr) = config.tool_service_addr.as_ref() {
            if addr.trim().is_empty() {
                self.error_line("configure: tool_service_addr must not be empty; clear it instead");
            } else {
                applied.tool_service_addr = Some(addr.clone());
            }
        } else if config.clears("tool_service_addr") {
            applied.clear_fields.push("tool_service_addr".into());
        }

        if config.storage_service_addr.is_some() || config.clears("storage_service_addr") {
            self.error_line(
                "configure: storage_service_addr is startup-only and cannot be changed at runtime",
            );
        }

        if config.executor_kind.is_some() || config.clears("executor_kind") {
            self.error_line(
                "configure: executor_kind is startup-only and cannot be changed at runtime; set `[executor] kind` in config.toml and restart the daemon",
            );
        }

        if config.tgrep.is_some() || config.clears("tgrep") {
            self.error_line(
                "configure: tgrep is startup-only and cannot be changed at runtime; set `[tools] tgrep` in config.toml and restart the daemon",
            );
        }
    }
}
