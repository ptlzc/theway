//! Tool-name dedup invariants (issue: DeepSeek rejects duplicate tool names
//! with HTTP 400, which breaks every turn). Split out of `mod.rs` to stay
//! under the repo's per-file line limit.

use std::sync::Arc;

use super::{harness_with_tools, mcp_tool};

#[test]
fn harness_construction_drops_duplicate_tool_names_first_wins() {
    // Initial session assembly appends provisioned MCP tools after the built-in
    // harness tools. A colliding MCP name must never reach the provider.
    let builtin = mcp_tool("web_search");
    let mcp_duplicate = mcp_tool("web_search");
    let ok = mcp_tool("read");

    let h = harness_with_tools(vec![builtin.clone(), mcp_duplicate.clone(), ok.clone()]);

    let tools = &h.agent().state().tools;
    assert_eq!(tools.len(), 2, "duplicate dropped, unique tools kept");
    assert!(tools.iter().any(|t| Arc::ptr_eq(t, &builtin)));
    assert!(
        !tools.iter().any(|t| Arc::ptr_eq(t, &mcp_duplicate)),
        "the later registration must be dropped, not the built-in"
    );
    assert!(tools.iter().any(|t| Arc::ptr_eq(t, &ok)));
    assert_eq!(
        tools
            .iter()
            .filter(|t| t.definition().name == "web_search")
            .count(),
        1
    );
}

#[test]
fn replace_tools_drops_duplicate_names_from_full_reload() {
    let first = mcp_tool("reload_a");
    let duplicate = mcp_tool("reload_a");
    let second = mcp_tool("reload_b");
    let h = harness_with_tools(vec![mcp_tool("existing")]);

    h.replace_tools(vec![first.clone(), duplicate.clone(), second.clone()]);

    let tools = &h.agent().state().tools;
    assert_eq!(tools.len(), 2, "full replacement dedups by name");
    assert!(tools.iter().any(|t| Arc::ptr_eq(t, &first)));
    assert!(!tools.iter().any(|t| Arc::ptr_eq(t, &duplicate)));
    assert!(tools.iter().any(|t| Arc::ptr_eq(t, &second)));
}
