//! Single entry point for daemon-side context projection.

use std::path::{Path, PathBuf};

use theway_core::{AgentMessage, Session};

use super::lineage::render_lineage;
use super::system_prompt::compose_system_prompt;

/// The materialized context for one session.
pub struct ContextBundle {
    pub system_prompt: String,
    /// Materialized from the session for harness rehydrate and test callers.
    #[allow(dead_code)]
    pub messages: Vec<AgentMessage>,
}

/// Projects a session into the context bundle consumed by the agent harness.
pub struct ContextService {
    cwd: PathBuf,
    memory: String,
    tool_names: Vec<String>,
    harness_intro: Option<String>,
}

impl ContextService {
    pub fn new(
        cwd: &Path,
        memory: &str,
        tool_names: Vec<String>,
        harness_intro: Option<String>,
    ) -> Self {
        Self {
            cwd: cwd.to_path_buf(),
            memory: memory.to_string(),
            tool_names,
            harness_intro,
        }
    }

    pub async fn load(
        &self,
        session: &Session,
    ) -> Result<ContextBundle, theway_core::SessionError> {
        let context = session.build_context().await?;
        let compact = session.compact_context().await?;
        let node_id = session.collapse_node_id().await?;
        let lineage = render_lineage(compact.as_ref(), node_id.as_deref());
        let system_prompt = compose_system_prompt(
            &self.cwd,
            &self.memory,
            &self.tool_names,
            lineage.as_deref(),
            self.harness_intro.as_deref(),
        );
        Ok(ContextBundle {
            system_prompt,
            messages: context.messages,
        })
    }
}
