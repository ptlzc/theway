// `Configure` — the `WireDaemonConfig` patch applier. Each field area is
// applied by its own applier under `commands/configure/`; this file owns the
// patch-level admission checks and the final commit into the shared config.
//
// `include!` fragment of the `turn::daemon` module — see `commands.rs`.

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/turn/daemon/commands/configure/model.rs"
));

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/turn/daemon/commands/configure/catalog.rs"
));

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/turn/daemon/commands/configure/mcp.rs"
));

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/turn/daemon/commands/configure/knobs.rs"
));

impl TurnHost {
    /// Apply a configuration patch on the serialized event loop. Only values
    /// whose runtime applier succeeds are committed to the shared GetConfig
    /// view; transport admission never mutates that view optimistically.
    async fn handle_configure(&mut self, mut config: WireDaemonConfig, turn: &mut TurnState) {
        tracing::info!(
            target: "mcp",
            "configure received: mcp_servers={} clear={:?}",
            config.mcp_servers.len(),
            config.clear_fields
        );
        let unknown = config.unknown_clear_fields();
        if !unknown.is_empty() {
            self.error_line(format!(
                "configure: unknown clear field(s): {}",
                unknown.join(", ")
            ));
            return;
        }

        let mut applied = WireDaemonConfig::default();

        // Issue #136: register controller-provisioned custom models before the
        // model pair is resolved, then seed the credential overlay.
        self.configure_models(&config, &mut applied);
        self.configure_api_key(&config, &mut applied);
        self.configure_auto_fetch_models(&mut config, &mut applied).await;

        self.configure_model_pair(&config, &mut applied).await;
        self.configure_thinking(&config, &mut applied).await;
        self.configure_skills(&config, &mut applied).await;
        self.configure_templates(&config, &mut applied);
        self.configure_mcp_servers(&config, &mut applied).await;
        self.configure_builtin_skills(&config, &mut applied);
        self.configure_skills_dirs(&config, turn, &mut applied).await;
        self.configure_runtime_knobs(&config, &mut applied);

        let touched = write_lock(&self.runtime.config).merge_from(&applied);
        if touched == 0 {
            self.system_line("configure: no applicable settings changed");
        } else {
            self.system_line(format!("configure: applied {touched} setting(s)"));
        }
        // Configure may have supplied the first model for a model-less session;
        // release any queued message that was waiting for one.
        self.start_next_queued_turn(turn);
    }
}
