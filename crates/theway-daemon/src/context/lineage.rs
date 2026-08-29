//! Lineage / handoff rendering for collapse child sessions.

use theway_core::agent::context::collapse::CompactContext;

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
    }
    block.push_str(
        "Use session_graph_list / session_graph_read / session_graph_status / \
         session_graph_wait / session_graph_attach to inspect or take over the old session graph.",
    );
    Some(block)
}
