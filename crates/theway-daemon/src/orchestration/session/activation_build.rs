//! Detached activation-build seam for session-scoped runtime assembly.

use std::sync::Arc;

use anyhow::Result;
use theway_contract::dag::PersistedRun;
use theway_contract::session::SessionStore;

use super::{SessionExecutionContext, SessionRuntime, SessionRuntimeBuilder};
use crate::tools;

impl SessionRuntimeBuilder {
    /// Build a runtime for an already-opened store without installing any
    /// process registries/launchers or restoring persisted DAG runs.
    #[allow(dead_code)] // Consumed by the future atomic activation transaction.
    pub(crate) async fn build_opened_detached(
        &self,
        ctx: &SessionExecutionContext,
        store: Arc<dyn SessionStore>,
        rehydrate: bool,
    ) -> Result<SessionRuntime> {
        let (ctx, session_id, store) = self.opened_context(ctx, store).await?;
        let skill_harness_cell = new_skill_harness_cell();
        self.assemble_opened(ctx, store, session_id, rehydrate, skill_harness_cell)
            .await
    }

    /// Install all process mappings and DAG restores after callers finish
    /// fallible loading. This commit phase is nonfallible.
    pub(crate) fn install_execution_context(
        &self,
        ctx: Arc<SessionExecutionContext>,
        restored: Vec<PersistedRun>,
    ) -> crate::tools::skill::SkillHarnessCell {
        self.services
            .session_execution
            .set_context(ctx.session_id.clone(), Arc::clone(&ctx));
        self.subagent_registry.set_session_transcript_store(
            Some(ctx.session_id.clone()),
            ctx.transcript_store.clone(),
        );

        let skill_harness_cell = new_skill_harness_cell();
        self.session_cells
            .lock()
            .insert(ctx.session_id.clone(), skill_harness_cell.clone());
        self.dag_engine.set_session_launcher(
            Some(ctx.session_id.clone()),
            tools::node_launcher(
                self.dag_engine.clone(),
                ctx.model.clone(),
                Some(self.stream_fn.clone()),
                ctx.cwd.clone(),
                self.subagent_registry.clone(),
                ctx.resources.memory_dir.clone(),
                ctx.paths.base.clone(),
                skill_harness_cell.clone(),
                ctx.executor.clone(),
            ),
        );

        // `restore` skips live ids, keeping repeated session installation idempotent.
        let restored = self.dag_engine.restore(restored);
        if !restored.is_empty() {
            tracing::info!(
                "session {}: restored {} in-flight DAG run(s): {}",
                ctx.session_id,
                restored.len(),
                restored.join(", ")
            );
        }
        skill_harness_cell
    }

    /// Rebuild the DAG launcher for one installed session with a new parent model,
    /// reusing the session's original skill harness cell so subagent tool sets keep
    /// the already-populated skill source. `SetModel` after activation must reach
    /// future DAG node launches; the launcher otherwise keeps the model snapshot
    /// from activation time. Returns `false` when the session was never installed.
    pub(crate) fn refresh_dag_launcher(
        &self,
        session_id: &str,
        model: theway_llm_provider::Model,
    ) -> bool {
        let Some(cell) = self.session_cells.lock().get(session_id).cloned() else {
            return false;
        };
        let Some(ctx) = self.services.session_execution.get_context(session_id) else {
            return false;
        };
        self.dag_engine.set_session_launcher(
            Some(session_id.to_string()),
            tools::node_launcher(
                self.dag_engine.clone(),
                model,
                Some(self.stream_fn.clone()),
                ctx.cwd.clone(),
                self.subagent_registry.clone(),
                ctx.resources.memory_dir.clone(),
                ctx.paths.base.clone(),
                cell,
                ctx.executor.clone(),
            ),
        );
        true
    }
}

/// Load the only fallible persisted state needed before installing a session.
pub(crate) async fn load_persisted_dag_runs(
    ctx: &SessionExecutionContext,
    session_id: &str,
) -> Result<Vec<PersistedRun>> {
    ctx.storage.load_dag_runs(&ctx.cwd, session_id).await
}

fn new_skill_harness_cell() -> crate::tools::skill::SkillHarnessCell {
    std::sync::Arc::new(once_cell::sync::OnceCell::new())
}
