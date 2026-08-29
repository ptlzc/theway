//! Session-scoped context assembly for the daemon harness.

use std::path::Path;

use theway_core::Session;

use super::lineage::render_lineage;
use super::system_prompt::compose_system_prompt;

/// Compose the full system prompt, including any collapse lineage block.
pub async fn system_prompt_for_session(
    cwd: &Path,
    memory: &str,
    tool_names: &[String],
    session: &Session,
) -> Result<String, theway_core::SessionError> {
    let compact = session.compact_context().await?;
    let node_id = session.collapse_node_id().await?;
    let lineage = render_lineage(compact.as_ref(), node_id.as_deref());
    Ok(compose_system_prompt(
        cwd,
        memory,
        tool_names,
        lineage.as_deref(),
    ))
}
