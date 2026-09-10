// `Configure` applier for controller-provisioned MCP servers (issue #73).
//
// `include!` fragment of the `turn::daemon` module — see `commands.rs`.

impl TurnHost {
    /// Issue #73: the controller owns MCP server discovery and
    /// provisions the full list here. The daemon connects each
    /// server (stdio spawn / streamable HTTP with the auth.json
    /// keychain refs), swaps the live harness's MCP tools, registers
    /// the fresh notification hooks on the live trigger executor,
    /// and publishes per-server failures through the snapshot so
    /// the TUI shows its 3s banner + red panel rows.
    async fn configure_mcp_servers(
        &mut self,
        config: &WireDaemonConfig,
        applied: &mut WireDaemonConfig,
    ) {
        if !config.mcp_servers.is_empty() || config.clears("mcp_servers") {
            let requested: Vec<crate::mcp_loader::ServerConfig> = config
                .mcp_servers
                .iter()
                .map(crate::mcp_loader::server_config_from_wire)
                .collect();
            match crate::mcp_loader::validate_unique_names(&requested) {
                Ok(()) => {
                    let old_tools = read_lock(&self.runtime.mcp_provision).tools.clone();
                    let result = crate::mcp_loader::connect_servers(
                        &requested,
                        &self.runtime.cwd,
                        &self.runtime.paths.base.join("auth.json"),
                    )
                    .await;
                    let (new_tools, new_hooks, capabilities_update) = {
                        let mut slot = write_lock(&self.runtime.mcp_provision);
                        slot.replace_connection_result(requested, result);
                        let capabilities_update = (
                            slot.server_names.len(),
                            slot.tool_names.len(),
                            slot.server_names.clone(),
                            slot.tool_names.clone(),
                            slot.errors.clone(),
                        );
                        (slot.tools.clone(), slot.hooks.clone(), capabilities_update)
                    };
                    if self.session.mcp_overlay.is_some() {
                        // session-scoped-mcp: this session's MCP set is its own
                        // overlay slot. Re-merge it over the new daemon layer
                        // instead of swapping the raw daemon tools in, so a
                        // session server still shadows a same-name daemon one.
                        self.remerge_active_session_mcp();
                    } else {
                        self.session
                            .kernel
                            .harness()
                            .replace_mcp_tools(&old_tools, new_tools);
                        self.projection.capabilities.mcp_servers = capabilities_update.0;
                        self.projection.capabilities.mcp_tools = capabilities_update.1;
                        self.projection.capabilities.mcp_server_names = capabilities_update.2;
                        self.projection.capabilities.mcp_tool_names = capabilities_update.3;
                        self.projection.capabilities.mcp_server_errors = capabilities_update.4;
                        // Register fresh hooks (new instances every connection)
                        // onto the live session's trigger executor. Hooks are
                        // one-shot; each generation is registered exactly once.
                        {
                            use crate::orchestration::session::NotificationHookSink;
                            use crate::trigger_engine::notification_hook::NotificationHook;
                            let executor = self.runtime.trigger_executor.clone();
                            let mut slot = write_lock(&self.runtime.mcp_provision);
                            for hook in &new_hooks {
                                let label = hook.label().to_string();
                                if slot.registered_labels.insert(label) {
                                    executor.register(hook.clone());
                                }
                            }
                        }
                    }
                    if config.mcp_servers.is_empty() {
                        applied.clear_fields.push("mcp_servers".into());
                    } else {
                        applied.mcp_servers = config.mcp_servers.clone();
                    }
                    let (connected, failed_count, tool_count, failed_names) = {
                        let slot = read_lock(&self.runtime.mcp_provision);
                        (
                            slot.server_names.len(),
                            slot.errors.len(),
                            slot.tool_names.len(),
                            slot.errors
                                .iter()
                                .map(|(name, _)| name.as_str())
                                .collect::<Vec<_>>()
                                .join(", "),
                        )
                    };
                    if failed_count == 0 {
                        self.system_line(format!(
                            "MCP: connected {connected} server(s), {tool_count} tool(s)"
                        ));
                    } else {
                        self.system_line(format!(
                            "MCP: connected {connected} server(s), {failed_count} failed: {failed_names}"
                        ));
                    }
                }
                Err(message) => self.error_line(format!("configure mcp_servers: {message}")),
            }
        }
    }
}
