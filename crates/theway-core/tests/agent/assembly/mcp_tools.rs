//! MCP tool provisioning through `replace_mcp_tools`: Arc-identity removal,
//! empty swap no-op, and name-collision handling.

use super::*;

#[test]
fn replace_mcp_tools_removes_matching_arcs_and_appends_new() {
    let old_a = mcp_tool("mcp_a");
    let old_b = mcp_tool("mcp_b");
    let builtin = mcp_tool("builtin");
    let h = harness_with_tools(vec![old_a.clone(), old_b.clone(), builtin.clone()]);
    assert_eq!(h.agent().state().tools.len(), 3);

    let new_a = mcp_tool("mcp_a_v2");
    let new_b = mcp_tool("mcp_b_v2");
    h.replace_mcp_tools(&[old_a.clone(), old_b.clone()], vec![new_a.clone(), new_b.clone()]);

    let tools = &h.agent().state().tools;
    assert_eq!(tools.len(), 3, "two removed, two added, builtin kept");
    // Old MCP tools (the exact Arc instances provisioned before) must be gone.
    assert!(!tools.iter().any(|t| Arc::ptr_eq(t, &old_a)));
    assert!(!tools.iter().any(|t| Arc::ptr_eq(t, &old_b)));
    // Built-in harness tool — even though it was not in `old` — must survive.
    assert!(tools.iter().any(|t| Arc::ptr_eq(t, &builtin)));
    // Freshly connected tools must be appended.
    assert!(tools.iter().any(|t| Arc::ptr_eq(t, &new_a)));
    assert!(tools.iter().any(|t| Arc::ptr_eq(t, &new_b)));
}

#[test]
fn replace_mcp_tools_leaves_non_old_tools_of_same_name_untouched() {
    // Identity semantics: a tool sharing a name with a removed MCP tool but backed by a
    // different Arc (here: another provisioning round, or a builtin collision) is kept,
    // because removal keys on `Arc::ptr_eq`, not on the name. Construction dedups by
    // name, so seed the live catalog directly to exercise the replacement path alone.
    let stale_a = mcp_tool("mcp_a");
    let stale_b = mcp_tool("mcp_a");
    let h = harness_with_tools(vec![mcp_tool("builtin")]);
    h.agent().state().tools = vec![stale_a.clone(), stale_b.clone()];

    h.replace_mcp_tools(std::slice::from_ref(&stale_a), Vec::new());

    let tools = &h.agent().state().tools;
    assert_eq!(tools.len(), 1);
    assert!(!tools.iter().any(|t| Arc::ptr_eq(t, &stale_a)));
    assert!(tools.iter().any(|t| Arc::ptr_eq(t, &stale_b)));
}

#[test]
fn replace_mcp_tools_with_empty_old_and_empty_new_is_a_noop() {
    let tool = mcp_tool("mcp_x");
    let h = harness_with_tools(vec![tool.clone()]);
    h.replace_mcp_tools(&[], Vec::new());
    let tools = &h.agent().state().tools;
    assert_eq!(tools.len(), 1);
    assert!(tools.iter().any(|t| Arc::ptr_eq(t, &tool)));
}

#[test]
fn replace_mcp_tools_drops_new_tools_with_colliding_names() {
    // A provisioned server exposing a tool that collides with an existing name
    // must not land in the request catalog: DeepSeek rejects duplicate tool
    // names with HTTP 400 and breaks every turn.
    let builtin = mcp_tool("shared_tool");
    let h = harness_with_tools(vec![builtin.clone()]);
    let mcp_ws = mcp_tool("shared_tool");
    let mcp_ws_dup = mcp_tool("shared_tool");
    let ok = mcp_tool("list_sessions");

    h.replace_mcp_tools(
        &[],
        vec![mcp_ws.clone(), mcp_ws_dup.clone(), ok.clone()],
    );

    let tools = &h.agent().state().tools;
    assert_eq!(tools.len(), 2, "builtin kept, unique MCP tool added");
    assert!(tools.iter().any(|t| Arc::ptr_eq(t, &builtin)));
    assert!(
        !tools.iter().any(|t| Arc::ptr_eq(t, &mcp_ws)),
        "MCP duplicate of a builtin name is dropped"
    );
    assert!(
        !tools.iter().any(|t| Arc::ptr_eq(t, &mcp_ws_dup)),
        "second MCP duplicate is dropped"
    );
    assert!(tools.iter().any(|t| Arc::ptr_eq(t, &ok)));
    assert_eq!(
        tools
            .iter()
            .filter(|t| t.definition().name == "shared_tool")
            .count(),
        1,
        "exactly one shared_tool remains"
    );
}
