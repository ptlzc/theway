//! Skills catalog hot-reload tests (split out of control_plane.rs).

use super::helpers::{faux_model, faux_stream_fn};
use super::*;

// ─────────────────────────────────────────────────────────────────────────────────────────
// handle_trigger — RFC 1 sub-PR 2 (moved to crates/server/tests/trigger_execution.rs)
// ─────────────────────────────────────────────────────────────────────────────────────────

#[test]
fn control_plane_write_category_defaults_to_allow_at_runtime_layer() {
    use theway_core::{PermissionCategory, PermissionDecision, PermissionPolicy};

    let policy = PermissionPolicy::default_for_coding_agent();
    // Even with bash-tool name + a normally-dangerous arg, the ControlPlaneWrite category
    // should fall through to Allow because the runtime policy has no category-specific
    // classifier wired. Tools-MCP's follow-up PR adds the danger classifier here.
    let args = serde_json::json!({ "command": "rm -rf /tmp/foo" });
    match policy.evaluate_with_category(PermissionCategory::ControlPlaneWrite, "bash", &args) {
        PermissionDecision::Allow => {}
        other => panic!("ControlPlaneWrite must default to Allow at runtime; got {other:?}"),
    }
    // Sanity check the legacy `evaluate` still uses the Tool category (bash classifier)
    // so backwards compatibility holds.
    match policy.evaluate("bash", &args) {
        PermissionDecision::Deny { .. } => {}
        other => panic!("Tool-category bash danger classifier must still deny; got {other:?}"),
    }
}

// ─────────────────────────────────────────────────────────────────────────────────────────
// Skill catalog hot-reload (issue #87 sub-PR A — `reload_skills_from_disk` API).
//
// Pins the invariants every downstream consumer (`InstallSkillTool`, `/skills reload`,
// future Web UI) needs to trust:
//   - `reload_skills_from_disk` calls the embedder-supplied loader exactly once per call
//     and applies the result via `replace_skills` so the system prompt rebuilds.
//   - Loader diagnostics propagate to the caller so install tools can surface "loaded N
//     skills, M warnings" without parsing free-form text.
//   - When no loader is configured, the API errors with `NotConfigured` instead of
//     silently no-opping.
//   - Reload only swaps the skill catalog + system prompt — it never touches
//     `state.messages` or `is_streaming`. (In-flight turn isn't interrupted.)
// ─────────────────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn reload_skills_from_disk_invokes_loader_and_replaces_catalog() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use theway_core::{LoadSkillsOutput, Skill};

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let mut opts = AgentHarnessOptions::new(faux_model(), session.clone());
    opts.skills = vec![Skill {
        name: "original".into(),
        content: "original body".into(),
        description: "the skill we ship with".into(),
        disable_model_invocation: false,
        source: SkillSource::User,
        file_path: "/tmp/original".into(),
    }];

    let call_count = Arc::new(AtomicUsize::new(0));
    let call_count_for_loader = call_count.clone();
    opts.reload_skills_fn = Some(Arc::new(move || {
        let call_count = call_count_for_loader.clone();
        Box::pin(async move {
            call_count.fetch_add(1, Ordering::SeqCst);
            LoadSkillsOutput {
                skills: vec![
                    Skill {
                        name: "fresh-one".into(),
                        content: "after install".into(),
                        description: "newly installed".into(),
                        disable_model_invocation: false,
                        source: SkillSource::User,
                        file_path: "/tmp/fresh-one".into(),
                    },
                    Skill {
                        name: "fresh-two".into(),
                        content: "second new".into(),
                        description: "also newly installed".into(),
                        disable_model_invocation: false,
                        source: SkillSource::User,
                        file_path: "/tmp/fresh-two".into(),
                    },
                ],
                diagnostics: Vec::new(),
            }
        })
    }));
    let harness = AgentHarness::new(opts);

    // Before reload: original catalog present.
    let before = harness.skills();
    assert_eq!(before.len(), 1);
    assert_eq!(before[0].name, "original");

    let result = harness
        .reload_skills_from_disk()
        .await
        .expect("loader configured");
    assert_eq!(result.skills.len(), 2);
    assert_eq!(call_count.load(Ordering::SeqCst), 1, "loader called once");

    // After reload: catalog replaced, system prompt rebuilt with new <skills> block.
    let after = harness.skills();
    assert_eq!(after.len(), 2);
    assert!(after.iter().any(|s| s.name == "fresh-one"));
    assert!(after.iter().any(|s| s.name == "fresh-two"));
    assert!(
        after.iter().all(|s| s.name != "original"),
        "old skill must be gone — single source of truth is the loader",
    );
    let prompt = harness.system_prompt();
    assert!(
        prompt.contains("fresh-one") && prompt.contains("fresh-two"),
        "system_prompt must rebuild with new <skills> block; got: {prompt}"
    );
    assert!(
        !prompt.contains("the skill we ship with"),
        "original skill description must not leak into rebuilt prompt: {prompt}",
    );
}

/// The UI sidebar repaints off harness events — a catalog hot-reload that emits nothing
/// leaves the skills panel stale (e.g. a sub-agent installing a skill while the parent
/// is idle). Every successful reload must announce itself.
#[tokio::test]
async fn reload_skills_from_disk_emits_skills_reloaded_event() {
    use std::sync::Mutex;
    use theway_core::{LoadSkillsOutput, Skill};

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let mut opts = AgentHarnessOptions::new(faux_model(), session.clone());
    opts.reload_skills_fn = Some(Arc::new(move || {
        Box::pin(async move {
            LoadSkillsOutput {
                skills: vec![Skill {
                    name: "fresh-one".into(),
                    content: "after install".into(),
                    description: "newly installed".into(),
                    disable_model_invocation: false,
                    source: SkillSource::User,
                    file_path: "/tmp/fresh-one".into(),
                }],
                diagnostics: Vec::new(),
            }
        })
    }));
    let harness = AgentHarness::new(opts);

    let received: Arc<Mutex<Vec<HarnessEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let received_for_listener = received.clone();
    let _unsubscribe = harness.subscribe_harness(Arc::new(move |event| {
        received_for_listener.lock().unwrap().push(event);
    }));

    harness
        .reload_skills_from_disk()
        .await
        .expect("loader configured");

    let events = received.lock().unwrap();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, HarnessEvent::SkillsReloaded { total: 1 })),
        "reload must emit SkillsReloaded with the new catalog size; got {} event(s)",
        events.len()
    );
}

#[tokio::test]
async fn reload_skills_from_disk_propagates_loader_diagnostics() {
    use theway_core::{LoadSkillsOutput, Skill, SkillDiagnostic, SkillDiagnosticCode};

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let mut opts = AgentHarnessOptions::new(faux_model(), session.clone());
    opts.reload_skills_fn = Some(Arc::new(move || {
        Box::pin(async move {
            // Realistic mixed result: one good skill + one diagnostic for a bad one. Bad
            // skill doesn't block the good one — that's the existing `load_skills` policy
            // and the embedder relies on it.
            LoadSkillsOutput {
                skills: vec![Skill {
                    name: "good".into(),
                    content: "ok".into(),
                    description: "valid skill".into(),
                    disable_model_invocation: false,
                    source: SkillSource::User,
                    file_path: "/tmp/good".into(),
                }],
                diagnostics: vec![SkillDiagnostic {
                    code: SkillDiagnosticCode::ParseFailed,
                    message: "frontmatter malformed".into(),
                    path: "/tmp/bad/SKILL.md".into(),
                }],
            }
        })
    }));
    let harness = AgentHarness::new(opts);

    let result = harness.reload_skills_from_disk().await.unwrap();

    assert_eq!(result.skills.len(), 1);
    assert_eq!(result.diagnostics.len(), 1);
    assert!(
        result.diagnostics[0]
            .message
            .contains("frontmatter malformed"),
        "diagnostic message must propagate verbatim to the install tool",
    );
    assert_eq!(harness.skills().len(), 1);
}

#[tokio::test]
async fn reload_skills_from_disk_without_loader_errors_with_not_configured() {
    use theway_core::ReloadSkillsError;

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    // No reload_skills_fn — default None.
    let harness = AgentHarness::new(AgentHarnessOptions::new(faux_model(), session.clone()));

    let err = harness
        .reload_skills_from_disk()
        .await
        .expect_err("loader missing should error, not silently no-op");
    assert!(
        matches!(err, ReloadSkillsError::NotConfigured),
        "expected NotConfigured, got {err:?}"
    );
}

#[tokio::test]
async fn reload_skills_from_disk_preserves_message_state_and_streaming_flag() {
    use theway_core::{LoadSkillsOutput, Skill};

    // Pin the "reload doesn't touch loop state" invariant: it only swaps the skill catalog
    // + system prompt. `state.messages` and `is_streaming` are the agent loop's
    // concerns; reload must not perturb them. Downstream consumers (InstallSkillTool
    // called from a sub-agent, `/skills reload` slash command) rely on this — they call
    // reload without coordinating with the parent loop.
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let mut opts = AgentHarnessOptions::new(faux_model(), session.clone());
    opts.stream_fn = Some(faux_stream_fn("ok"));
    opts.reload_skills_fn = Some(Arc::new(move || {
        Box::pin(async move {
            LoadSkillsOutput {
                skills: vec![Skill {
                    name: "reloaded".into(),
                    content: "fresh".into(),
                    description: "post-reload".into(),
                    disable_model_invocation: false,
                    source: SkillSource::User,
                    file_path: "/tmp/reloaded".into(),
                }],
                diagnostics: Vec::new(),
            }
        })
    }));
    let harness = AgentHarness::new(opts);

    // Drive one normal turn so state.messages has content + system_prompt is established.
    harness.prompt("hello").await.unwrap();
    let pre_messages_len = harness.agent().state().messages.len();
    assert!(
        pre_messages_len > 0,
        "expected at least one message after prompt"
    );
    let pre_is_streaming = harness.agent().is_streaming();

    let result = harness.reload_skills_from_disk().await.unwrap();
    assert_eq!(result.skills.len(), 1);

    let post_messages_len = harness.agent().state().messages.len();
    let post_is_streaming = harness.agent().is_streaming();
    assert_eq!(
        post_messages_len, pre_messages_len,
        "reload must not touch state.messages",
    );
    assert_eq!(
        post_is_streaming, pre_is_streaming,
        "reload must not touch is_streaming",
    );

    // The system_prompt DOES get the new <skills> block — that's the whole point.
    assert!(
        harness.system_prompt().contains("reloaded"),
        "rebuilt system_prompt should mention new skill",
    );
}
