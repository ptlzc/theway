//! Goal + session state (`App` methods split out of `ui/mod.rs`).
//!
//! Goal-state refresh, session switch (harness rebuild), and the live current-session
//! state cell shared with [`crate::session_ops::AppSessionOps`].

use anyhow::{Context as _, Result};

use super::App;
use super::current_model_label;

impl App {
    pub(super) async fn refresh_goal_state(&mut self) {
        self.latest_goal = theway_core::multiagent::goal::current(self.kernel.harness()).await;
    }

    /// session-resource-model: swap the runtime to a different session.
    ///
    /// Builds a fresh harness via the [`crate::session_ops::SessionFactory`] (resume
    /// semantics — the factory rehydrates the transcript and re-wires session-stamped
    /// tools), replaces the kernel's harness, and resets every piece of per-session UI
    /// state. Must run inside the serialized event loop (it is driven by
    /// `WebCommand::SwitchSession`); a turn in flight on the old harness must be aborted
    /// by the caller before this runs.
    pub(crate) async fn switch_session(&mut self, id: String) -> Result<()> {
        let harness = (self.session_factory)(id.clone())
            .await
            .with_context(|| format!("build harness for session {id}"))?;
        self.kernel.replace_harness(harness);
        self.session_id = id.clone();
        // Feed is UI-transient: clear it and mark the boundary; the transcript itself stays
        // in the JSONL store (design decision: 清空 + 系统提示行).
        self.feed.clear();
        self.system_line(format!("switched to session {id}"));
        self.busy = false;
        self.queued_turns.clear();
        // A pending control-plane prompt belongs to the old harness's tool call — drop it
        // so the UI never waits on a decision the new harness will never consume.
        self.control_plane_prompt = None;
        // Goal state belongs to the previous session's harness; re-read from the new one.
        self.refresh_goal_state().await;
        self.sync_current_session_state();
        Ok(())
    }

    /// Push the live current-session state (id / busy / model / cwd) into the shared cell
    /// that backs [`crate::session_ops::AppSessionOps`]. Called on every published snapshot
    /// and on session switch.
    pub(super) fn sync_current_session_state(&self) {
        let mut state = self.current_session_state.lock();
        state.session_id = self.session_id.clone();
        state.busy = self.busy;
        state.model = current_model_label(self.kernel.harness());
        state.cwd = self.cwd.display().to_string();
    }
}
