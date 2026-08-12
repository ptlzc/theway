//! Panel-status helpers shared by the daemon and (mirrored in) the TUI: which hook
//! registrations and trigger-runtime features are wired in this binary. Pure
//! inventory functions — the TUI keeps its own copy in `ui_mode` with the
//! UI-mode resolver; the daemon only needs the inventory.

/// Real `*Hook` trait registrations active in this binary. Only names that map to an actual
/// `AgentHarness` extension point — so users reading the panel learn what hooks they could
/// plug into. `dedup` / `cycle suppress` / `fire-once rules` / `inject-and-run` are
/// trigger-runtime *features*, not hooks, and live in [`active_trigger_features`] instead.
pub fn active_hook_registrations(lsp_lang_count: usize, cli_hooks_loaded: bool) -> Vec<String> {
    let mut points = vec![
        "before_tool_call".to_string(),
        "on_control_plane_prompt".to_string(),
        "before_trigger_action".to_string(),
    ];
    if lsp_lang_count > 0 {
        points.push("after_tool_call".to_string());
    }
    if cli_hooks_loaded {
        points.push("cli_hooks".to_string());
    }
    points
}

/// Trigger-runtime features always wired in the current binary. Distinct from hook
/// registrations — these are pipeline behaviors (dedup, cycle suppression, fire-once rules,
/// inject-and-run delivery), not pluggable callbacks.
pub fn active_trigger_features() -> Vec<String> {
    vec![
        "dedup".to_string(),
        "cycle suppress".to_string(),
        "fire-once rules".to_string(),
        "inject-and-run".to_string(),
    ]
}
