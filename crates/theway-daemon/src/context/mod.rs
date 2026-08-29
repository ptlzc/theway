//! Daemon-side context assembly.
//!
//! Reads the persisted collapse context from a core [`Session`], renders a
//! lineage / handoff block, and composes the final system prompt for the agent
//! harness.

use std::path::Path;

use theway_core::Session;
use theway_core::agent::context::collapse::CompactContext;

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
    Ok(crate::system_prompt::compose_system_prompt(
        cwd,
        memory,
        tool_names,
        lineage.as_deref(),
    ))
}

/// Render the lineage / handoff block for a collapse child session.
pub fn render_lineage(
    compact: Option<&CompactContext>,
    collapse_node_id: Option<&str>,
) -> Option<String> {
    if compact.is_none() && collapse_node_id.is_none() {
        return None;
    }

    let mut block = String::from("## Session lineage\n\n");
    if let Some(node_id) = collapse_node_id {
        block.push_str(&format!("Collapse node: {node_id}\n"));
    }
    if let Some(compact) = compact {
        if !compact.source_session_id.is_empty() {
            block.push_str(&format!(
                "This session continues from {}.\n",
                compact.source_session_id
            ));
        }
        if !compact.compact_text.is_empty() {
            block.push_str(&format!(
                "Previous context summary: {}\n",
                compact.compact_text
            ));
        }
    }
    block.push_str(
        "Use session_graph_list / session_graph_read / session_graph_status / \
         session_graph_wait / session_graph_attach to inspect or take over the old session graph.",
    );
    Some(block)
}

#[cfg(test)]
tests_bridge_macro::tests_bridge!("context");
