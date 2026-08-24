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
