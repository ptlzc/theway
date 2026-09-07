//! Goal + session state (`App` methods split out of `ui/mod.rs`).
//!
//! Client mode: goal state arrives in snapshots (`latest.goal`); sessions are
//! addressed explicitly by session id, and control-plane resolution is an RPC
//! call. The import-activation card is resolved locally (it writes sidecar
//! files, no daemon round-trip).

use anyhow::Result;

use super::App;

impl App {
    /// Select another session client-side. There is no daemon session switch;
    /// subsequent RPCs use explicit session ids, and the daemon publishes
    /// per-session state when the client subscribes to that session.
    pub(crate) async fn select_session(&mut self, id: String) -> Result<()> {
        // An explicit selection supersedes any pending deferred fresh attach
        // (issue #46): the user picked a real session, so nothing is created
        // on the next message.
        self.pending_fresh_attach = false;
        self.session_id = id.clone();
        self.latest.session_id = id.clone();
        // Load the selected session's authoritative feed immediately; the
        // current frame stream is still filtered to the previous session until
        // the event loop resubscribes below. Bounded (#99): a hung daemon
        // must not freeze the selection.
        match crate::ui::daemon_call(
            "get_snapshot_for_session",
            self.client.get_snapshot_for_session(&id),
        )
        .await
        {
            Ok(state) => {
                self.apply_snapshot(theway_transport::proto::wire_status_from_session_snapshot(
                    &state,
                ));
            }
            Err(error) => {
                self.error_line(format!("load session {id}: {error}"));
            }
        }
        // A newly created session (`/new`) may not have a daemon runtime in
        // the snapshot map yet; that is fine, the resubscribed stream will
        // publish its first authoritative snapshot once it exists.
        self.resubscribe_session = Some(id.clone());
        self.refresh_session_snapshot().await;
        self.system_line(format!("selected session {id}"));
        Ok(())
    }

    /// Refresh the nested session snapshot for the currently selected session.
    /// The TUI uses it to render session lineage and collapsed graph nodes;
    /// failures are non-fatal (the legacy `WireStatus` view remains usable).
    pub(super) async fn refresh_session_snapshot(&mut self) {
        match crate::ui::daemon_call(
            "get_snapshot_for_session",
            self.client.get_snapshot_for_session(&self.session_id),
        )
        .await
        {
            Ok(snapshot) => {
                self.session_snapshot = Some(
                    theway_transport::proto::wire_session_snapshot_from_proto(&snapshot),
                );
            }
            Err(_) => {
                self.session_snapshot = None;
            }
        }
    }

    /// Issue #46 + #97: create + select the fresh session for a reused-daemon
    /// attach (issue #56). Called right before the first submitted message
    /// reaches the daemon, and at startup when the fresh attach is eager
    /// (issue #97: the new session exists immediately — the TUI never shows
    /// the previous session's id anywhere). Idempotent: no-op once the flag
    /// is cleared. Returns the created session id.
    pub(crate) async fn ensure_fresh_session(&mut self) -> Result<String> {
        if !self.pending_fresh_attach {
            return Ok(self.session_id.clone());
        }
        let id = self.create_session_with_configured_defaults().await?;
        Ok(id)
    }

    /// Create, select, and default-configure a new session in one step.
    ///
    /// The daemon's settings view holds the configured default model (from
    /// `config.toml` or the controller payload). Session creation is storage
    /// only, so a newly created session would otherwise start model-less even
    /// when a default is configured. Non-fatal when the default cannot be
    /// applied: the session still exists and the error is visible in the feed.
    pub(crate) async fn create_session_with_configured_defaults(&mut self) -> Result<String> {
        let summary = crate::ui::daemon_call(
            "create_session",
            self.client
                .create_session_with_metadata(None, None, Default::default()),
        )
        .await?;
        // Clear before select_session (which also clears it); the message
        // right after this call must not re-trigger creation.
        self.pending_fresh_attach = false;
        let id = summary.session_id;
        self.select_session(id.clone()).await?;
        self.apply_configured_session_defaults(&id).await;
        self.system_line(format!("new session {id}"));
        Ok(id)
    }

    /// Apply the daemon-configured default model and thinking level to a
    /// session that has just been created. Failures are feed lines, not
    /// errors: a session without a default still opens, and the user can pick
    /// a model interactively.
    async fn apply_configured_session_defaults(&mut self, session_id: &str) {
        let config = match crate::ui::daemon_call("get_config", self.client.get_config()).await {
            Ok(config) => config,
            Err(error) => {
                self.error_line(format!("read default model config: {error}"));
                return;
            }
        };
        if let (Some(provider), Some(model)) = (config.provider.as_deref(), config.model.as_deref())
        {
            let spec = format!("{provider}:{model}");
            match crate::ui::daemon_call(
                "set_model",
                self.client.set_model_for_session(session_id, &spec),
            )
            .await
            {
                Ok(true) => {}
                Ok(false) => {
                    self.error_line(format!(
                        "daemon rejected default model {spec} for session {session_id}"
                    ));
                }
                Err(error) => {
                    self.error_line(format!("set default model on {session_id}: {error}"));
                }
            }
        }
        if let Some(level) = config
            .thinking_level
            .as_deref()
            .filter(|level| !level.trim().is_empty() && *level != "off")
        {
            match crate::ui::daemon_call(
                "set_thinking",
                self.client.set_thinking_for_session(session_id, level),
            )
            .await
            {
                Ok(true) => {}
                Ok(false) => {
                    self.error_line(format!(
                        "daemon rejected default thinking {level} for session {session_id}"
                    ));
                }
                Err(error) => {
                    self.error_line(format!("set default thinking on {session_id}: {error}"));
                }
            }
        }
    }

    /// Issue #97: mark the eagerly-created fresh-attach session as the
    /// startup-auto session so the idle reap (issue #47) deletes it on exit
    /// when no message ever reached it.
    pub(crate) fn set_auto_session(&mut self, id: String) {
        self.auto_session = Some(id);
    }

    /// Issue #47: on exit, delete the session the SPAWNED daemon created at
    /// startup when no message ever reached it — an idle TUI must not leave
    /// an empty conversation behind. Best-effort: the daemon may already be
    /// gone, or the session may be protected (running graphs).
    pub(crate) async fn reap_empty_auto_session(&mut self) {
        let Some(id) = self.auto_session.clone() else {
            return;
        };
        if self.messaged_sessions.contains(&id) {
            return;
        }
        match crate::ui::daemon_call("delete_session", self.client.delete_session(&id)).await {
            Ok(running) if running.is_empty() => {
                tracing::debug!("reaped empty startup session {id}");
            }
            Ok(running) => {
                tracing::debug!("startup session {id} kept: active graphs {running:?}");
            }
            Err(error) => {
                tracing::debug!("startup session {id} reap skipped: {error}");
            }
        }
    }

    /// Resolve the pending daemon control-plane prompt through the `approve`
    /// RPC (the snapshot clears the card on the next frame).
    pub(super) fn resolve_control_plane_prompt(&mut self, approve: bool) {
        let Some(prompt) = self.control_plane_prompt.take() else {
            return;
        };
        self.system_line(format!(
            "permission {}: {}",
            if approve { "allowed" } else { "denied" },
            prompt.tool_name
        ));
        let client = self.client.clone();
        let session_id = self.session_id.clone();
        tokio::spawn(async move {
            let mut client = client;
            if let Err(e) = client.approve_for_session(&session_id, approve).await {
                eprintln!("approve: {e}");
            }
        });
    }
}
