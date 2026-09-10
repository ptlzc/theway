// `SetSkillDirs` / `Configure.skills_dirs`: the authoritative extra-skill-dir
// apply path (issue #68).
//
// `include!` fragment of the `turn::daemon` module — see `commands.rs`.

impl TurnHost {
    /// Apply a `SetSkillDirs` command authoritatively (issue #68): replace
    /// the daemon's extra skill dirs, refresh the shared wire path context,
    /// abort any in-flight turn (its context predates the new catalog), and
    /// hot-reload skills from disk through the harness's reload closure. The
    /// gRPC server applies an optimistic `path_context` update with the same
    /// dirs before enqueuing this command; this step makes it durable.
    async fn handle_set_skill_dirs(&mut self, dirs: Vec<String>, turn: &mut TurnState) {
        let dirs: Vec<PathBuf> = dirs.into_iter().map(PathBuf::from).collect();
        self.runtime.paths.set_extra_skill_dirs(dirs);
        // Keep the shared wire path context in sync with the authoritative
        // value (`GetPathContext` readers observe it immediately).
        write_lock(&self.runtime.path_context).skills_dirs = self
            .runtime
            .paths
            .current_extra_skill_dirs()
            .into_iter()
            .map(|dir| dir.to_string_lossy().into_owned())
            .collect();
        if turn.fut.is_some() {
            self.request_abort(turn);
        }
        match self.session.kernel.harness().reload_skills_from_disk().await {
            Ok(out) => self.system_line(format!(
                "set skill dirs: {} loaded, {} diagnostics",
                out.skills.len(),
                out.diagnostics.len()
            )),
            Err(e) => self.error_line(format!("set skill dirs: {e:#}")),
        }
    }
}
