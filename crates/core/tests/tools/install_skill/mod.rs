//! Tests for `install_skill` — split out of src (see docs/RUST_TEST_FILES.md).

    use super::*;
    use once_cell::sync::OnceCell as SyncOnceCell;
    use std::sync::Arc;
    use theway_core::{
        AgentHarness, AgentHarnessOptions, MemorySessionStorage, ReloadSkillsFn, Session,
        SessionStorage, Skill,
    };
    use theway_llm_provider::{Api, Model, ModelCost, Provider};

    fn fake_model() -> Model {
        Model {
            id: "faux".into(),
            name: "Faux".into(),
            api: Api::from("faux"),
            provider: Provider::from("faux"),
            base_url: String::new(),
            reasoning: false,
            thinking_level_map: None,
            input: vec![],
            cost: ModelCost::default(),
            context_window: 0,
            max_tokens: 0,
            headers: None,
            compat: None,
        }
    }

    /// Build a harness whose `reload_skills_from_disk` rescans a single test directory.
    /// Returns the harness handle, the cell to plug into the tool, and the temp dir so
    /// callers can construct `InstallSkillTool::with_skills_root(cell, dir.path().into())`
    /// and exercise the install path against the same dir the harness reloads from.
    fn build_test_harness(
        seed: Vec<Skill>,
    ) -> (Arc<AgentHarness>, SkillHarnessCell, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let dir_path = dir.path().to_path_buf();
        let storage = Arc::new(MemorySessionStorage::new()) as Arc<dyn SessionStorage>;
        let session = Session::new(storage);
        let mut opts = AgentHarnessOptions::new(fake_model(), session);
        opts.skills = seed;
        let dir_clone = dir_path.clone();
        let loader: ReloadSkillsFn = Arc::new(move || {
            let dir_for_fut = dir_clone.clone();
            Box::pin(async move {
                let env = theway_core::NativeEnv::new(
                    std::env::current_dir()
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_default(),
                );
                theway_core::load_skills(
                    &env,
                    &[dir_for_fut.to_string_lossy().as_ref()],
                    CancellationToken::new(),
                )
                .await
            })
        });
        opts.reload_skills_fn = Some(loader);
        let harness = Arc::new(AgentHarness::new(opts));
        let cell: SkillHarnessCell = Arc::new(SyncOnceCell::new());
        // `OnceCell::set` returns `Err(T)` on collision and `T = Arc<AgentHarness>` isn't
        // `Debug`, so use `is_ok()` + assert instead of `.expect(...)`.
        assert!(cell.set(harness.clone()).is_ok(), "set once");
        (harness, cell, dir)
    }

    fn make_skill_md(name: &str, description: &str, body: &str) -> String {
        format!("---\nname: {name}\ndescription: {description}\n---\n{body}\n")
    }

    async fn execute(
        tool: &InstallSkillTool,
        params: Value,
    ) -> Result<AgentToolResult, AgentToolError> {
        tool.execute("call-1", params, CancellationToken::new(), None)
            .await
    }

    fn test_tool(cell: SkillHarnessCell, dir: &tempfile::TempDir) -> InstallSkillTool {
        InstallSkillTool::with_skills_root(cell, dir.path().to_path_buf())
    }

    /// Preview path is read-only — must NOT write anything to the configured skills dir.
    /// Asserts both the absence of side effects AND the preview payload shape.
    #[tokio::test]
    async fn preview_returns_metadata_without_writing() {
        let (_harness, cell, dir) = build_test_harness(vec![]);
        let tool = test_tool(cell, &dir);
        let skill_md = make_skill_md("alpha", "a useful skill", "do alpha things");

        let result = execute(
            &tool,
            json!({ "source": { "type": "content", "content": skill_md } }),
        )
        .await
        .expect("preview should succeed");

        assert_eq!(result.details["phase"], "preview");
        assert_eq!(result.details["name"], "alpha");
        assert_eq!(result.details["description"], "a useful skill");
        assert_eq!(result.details["existing"], false);
        assert_eq!(result.details["overwrite_required"], false);
        // Body must not be echoed verbatim. Hash + size carry the integrity info.
        let preview_text = match &result.content[0] {
            UserContentBlock::Text(t) => &t.text,
            _ => panic!("expected text"),
        };
        assert!(
            !preview_text.contains("do alpha things"),
            "preview must not echo skill body, got: {preview_text}"
        );
        // No file should have been created in the test dir.
        assert!(
            !dir.path().join("alpha").exists(),
            "preview must not create any files"
        );
    }

    /// Path traversal / invalid name in frontmatter must be refused at parse time, BEFORE
    /// any fs path resolution. Belt-and-suspenders: even if validate_name regressed, the
    /// target path is derived strictly from the validated name field, never from a source
    /// path component.
    #[tokio::test]
    async fn rejects_traversal_in_skill_name() {
        let (_harness, cell, dir) = build_test_harness(vec![]);
        let tool = test_tool(cell, &dir);
        let evil = "---\nname: ../etc/passwd\ndescription: x\n---\nbody";
        let err = execute(
            &tool,
            json!({"source": {"type": "content", "content": evil}}),
        )
        .await
        .expect_err("traversal name must fail");
        let AgentToolError::Message(m) = err else {
            panic!("expected typed error");
        };
        assert!(
            m.contains("invalid characters") || m.contains("must contain"),
            "expected name validation error, got: {m}"
        );
    }

    /// http:// (and any non-https scheme) is refused before the request goes out.
    #[tokio::test]
    async fn rejects_http_url() {
        let (_harness, cell, dir) = build_test_harness(vec![]);
        let tool = test_tool(cell, &dir);
        let err = execute(
            &tool,
            json!({"source": {"type": "url", "url": "http://example.com/skill.md"}}),
        )
        .await
        .expect_err("http must fail");
        let AgentToolError::Message(m) = err else {
            panic!("expected typed error");
        };
        assert!(m.contains("https"), "expected https-only error, got: {m}");
    }

    /// The model naturally tried `{type: "https", url: ...}` in the wild. Keep the public
    /// schema canonical (`url`) but accept `https` as a compatibility alias so the retry
    /// path succeeds instead of bouncing on argument decoding.
    #[tokio::test]
    async fn accepts_https_source_alias_for_url() {
        let input: InstallInput = serde_json::from_value(json!({
            "source": { "type": "https", "url": "https://example.com/skill.md" }
        }))
        .expect("https alias should decode");

        match input.source {
            Source::Url { url } => assert_eq!(url, "https://example.com/skill.md"),
            _ => panic!("https alias should decode as Source::Url"),
        }
    }

    /// SSRF guard: loopback / RFC1918 / `.localhost` hostnames are refused.
    #[tokio::test]
    async fn rejects_private_and_loopback_hosts() {
        let (_harness, cell, dir) = build_test_harness(vec![]);
        let tool = test_tool(cell, &dir);
        for host in [
            "https://127.0.0.1/skill.md",
            "https://localhost/skill.md",
            "https://10.0.0.1/skill.md",
            "https://192.168.1.1/skill.md",
            "https://api.localhost/skill.md",
        ] {
            let result = execute(&tool, json!({"source": {"type": "url", "url": host}})).await;
            assert!(
                result.is_err(),
                "host {host} must be refused, got: {result:?}"
            );
            if let Err(AgentToolError::Message(m)) = result {
                assert!(
                    m.contains("SSRF") || m.contains("local") || m.contains("private"),
                    "host {host}: expected SSRF/local/private error, got: {m}"
                );
            }
        }
    }

    /// Real skills can exceed the old 64 KiB cap (`https://db9.ai/skill.md` was ~95 KiB
    /// when this regression was added). Inline/local skill bodies are no longer rejected
    /// by a small fixed artifact-size cap; preview remains metadata-only and bounded.
    #[tokio::test]
    async fn accepts_db9_sized_skill_body_without_echoing_body() {
        let (_harness, cell, dir) = build_test_harness(vec![]);
        let tool = test_tool(cell, &dir);
        let marker = "large-skill-body-marker";
        let body = format!("{marker}\n{}", "x".repeat(128 * 1024));
        let skill_md = make_skill_md("large-skill", "large desc", &body);

        let preview = execute(
            &tool,
            json!({"source": {"type": "content", "content": skill_md.clone()}}),
        )
        .await
        .expect("large skill preview should succeed");

        assert_eq!(preview.details["phase"], "preview");
        assert_eq!(preview.details["name"], "large-skill");
        assert_eq!(preview.details["size"], skill_md.len());
        let preview_text = match &preview.content[0] {
            UserContentBlock::Text(t) => &t.text,
            _ => panic!("expected text"),
        };
        assert!(
            !preview_text.contains(marker),
            "preview must not echo large skill body, got: {preview_text}"
        );
        let preview_details = serde_json::to_string(&preview.details).unwrap();
        assert!(
            !preview_details.contains(marker),
            "preview details must not echo large skill body, got: {preview_details}"
        );
    }

    /// Malformed frontmatter / missing name → error before any write. Missing description is
    /// recoverable and covered below.
    #[tokio::test]
    async fn rejects_skill_missing_frontmatter() {
        let (_harness, cell, dir) = build_test_harness(vec![]);
        let tool = test_tool(cell, &dir);
        for bad in [
            "no frontmatter at all",
            "---\ndescription: only-desc\n---\nbody",
            "---\nname: foo\n",
        ] {
            let result = execute(
                &tool,
                json!({"source": {"type": "content", "content": bad}}),
            )
            .await;
            assert!(result.is_err(), "input {bad:?} must be refused");
        }
    }

    #[tokio::test]
    async fn installs_skill_missing_description_with_warning() {
        let (harness, cell, dir) = build_test_harness(vec![]);
        let tool = test_tool(cell, &dir);
        let skill_md = "---\nname: only-name\n---\n# Heading\nBody body.";

        let preview = execute(
            &tool,
            json!({"source": {"type": "content", "content": skill_md}}),
        )
        .await
        .expect("missing description should preview with warning");
        assert_eq!(preview.details["phase"], "preview");
        assert_eq!(preview.details["name"], "only-name");
        assert_eq!(preview.details["description"], "No description provided.");
        assert!(
            preview.details["warnings"][0]
                .as_str()
                .unwrap()
                .contains("description missing"),
            "expected missing-description warning, got {:?}",
            preview.details["warnings"]
        );

        let installed = execute(
            &tool,
            json!({"source": {"type": "content", "content": skill_md}, "confirm": true}),
        )
        .await
        .expect("missing description should install with fallback");
        assert_eq!(installed.details["phase"], "installed");
        assert_eq!(installed.details["installed_visible_in_catalog"], true);
        assert!(
            installed.details["warnings"][0]
                .as_str()
                .unwrap()
                .contains("description missing"),
            "expected missing-description install warning, got {:?}",
            installed.details["warnings"]
        );

        let written = tokio::fs::read_to_string(dir.path().join("only-name").join("SKILL.md"))
            .await
            .expect("SKILL.md was written");
        assert!(written.contains("description: No description provided."));
        assert!(
            harness
                .skills()
                .iter()
                .any(|s| { s.name == "only-name" && s.description == "No description provided." }),
            "installed skill should be visible after reload"
        );
    }

    #[tokio::test]
    async fn previews_recoverable_description_format_with_warning() {
        let (_harness, cell, dir) = build_test_harness(vec![]);
        let tool = test_tool(cell, &dir);
        let oversized = "x".repeat(MAX_DESCRIPTION_LEN + 1);
        for (skill_md, expected_warning) in [
            (
                "---\nname: empty-desc\ndescription: '   '\n---\nBody.".to_string(),
                "description empty",
            ),
            (
                format!("---\nname: long-desc\ndescription: {oversized}\n---\nBody."),
                "description exceeds",
            ),
        ] {
            let preview = execute(
                &tool,
                json!({"source": {"type": "content", "content": skill_md}}),
            )
            .await
            .expect("recoverable description should preview with warning");
            assert_eq!(preview.details["phase"], "preview");
            assert_eq!(preview.details["description"], "No description provided.");
            assert!(
                preview.details["warnings"][0]
                    .as_str()
                    .unwrap()
                    .contains(expected_warning),
                "expected {expected_warning:?} warning, got {:?}",
                preview.details["warnings"]
            );
        }
    }

    #[tokio::test]
    async fn installs_block_scalar_oversized_description_with_warning() {
        let (harness, cell, dir) = build_test_harness(vec![]);
        let tool = test_tool(cell, &dir);
        let oversized = "x".repeat(MAX_DESCRIPTION_LEN + 1);
        let skill_md = format!(
            "---\nname: block-desc\ndescription: |\n  {oversized}\nx-custom: true\n---\n# Heading\nBody."
        );

        let installed = execute(
            &tool,
            json!({"source": {"type": "content", "content": skill_md}, "confirm": true}),
        )
        .await
        .expect("block scalar oversized description should install with fallback");
        assert_eq!(installed.details["phase"], "installed");
        assert_eq!(installed.details["installed_visible_in_catalog"], true);
        assert!(
            installed.details["warnings"][0]
                .as_str()
                .unwrap()
                .contains("description exceeds"),
            "expected oversized-description install warning, got {:?}",
            installed.details["warnings"]
        );

        let written = tokio::fs::read_to_string(dir.path().join("block-desc").join("SKILL.md"))
            .await
            .expect("SKILL.md was written");
        assert!(written.contains("description: No description provided."));
        assert!(!written.contains(&format!("  {oversized}")));
        assert!(written.contains("x-custom: true"));
        assert!(
            harness
                .skills()
                .iter()
                .any(|s| { s.name == "block-desc" && s.description == "No description provided." }),
            "installed block scalar skill should be visible after reload"
        );
    }

    #[tokio::test]
    async fn accepts_unknown_extra_frontmatter_fields() {
        let (_harness, cell, dir) = build_test_harness(vec![]);
        let tool = test_tool(cell, &dir);
        let skill_md = "---\nname: extra-field\ndescription: useful\nx-custom: true\n---\nBody.";

        let preview = execute(
            &tool,
            json!({"source": {"type": "content", "content": skill_md}}),
        )
        .await
        .expect("unknown frontmatter fields should be ignored");
        assert_eq!(preview.details["phase"], "preview");
        assert_eq!(preview.details["name"], "extra-field");
        assert_eq!(preview.details["warnings"].as_array().unwrap().len(), 0);
    }

    /// Existing skill, same on-disk content → not overwrite_required (idempotent re-install OK).
    /// Existing skill, different on-disk content → overwrite_required=true; without `overwrite`
    /// the install rejects with a clear "use overwrite: true" message.
    #[tokio::test]
    async fn overwrite_required_when_hash_differs() {
        let (_harness, cell, dir) = build_test_harness(vec![]);
        let tool = test_tool(cell, &dir);
        // Pre-write an existing skill at the target path with old content.
        let old_md = make_skill_md("alpha", "desc", "old body");
        atomic_write_skill(&dir.path().join("alpha").join("SKILL.md"), &old_md)
            .await
            .unwrap();

        let new_md = make_skill_md("alpha", "desc", "new body");

        // Preview must signal existing + overwrite_required.
        let preview = execute(
            &tool,
            json!({"source": {"type": "content", "content": new_md.clone()}}),
        )
        .await
        .expect("preview ok");
        assert_eq!(preview.details["existing"], true);
        assert_eq!(preview.details["overwrite_required"], true);

        // Confirm without overwrite must fail with a clear hint.
        let err = execute(
            &tool,
            json!({"source": {"type": "content", "content": new_md.clone()}, "confirm": true}),
        )
        .await
        .expect_err("install without overwrite must fail");
        let AgentToolError::Message(m) = err else {
            panic!("expected typed error");
        };
        assert!(
            m.contains("overwrite: true"),
            "expected overwrite-required hint, got: {m}"
        );

        // Same-bytes re-install is idempotent: existing=true, overwrite_required=false.
        let same_preview = execute(
            &tool,
            json!({"source": {"type": "content", "content": old_md.clone()}}),
        )
        .await
        .expect("idempotent preview ok");
        assert_eq!(same_preview.details["existing"], true);
        assert_eq!(same_preview.details["overwrite_required"], false);
    }

    /// Full happy path: phase 1 preview → phase 2 install via the tool itself →
    /// fs has SKILL.md at the right path with the right content → harness reload picks it up.
    #[tokio::test]
    async fn install_writes_atomic_and_reloads_catalog() {
        let (harness, cell, dir) = build_test_harness(vec![]);
        let tool = test_tool(cell, &dir);
        let skill_md = make_skill_md("beta", "beta desc", "beta body");

        // Phase 2 directly (Phase 1 preview is exercised by another test).
        let install = execute(
            &tool,
            json!({"source": {"type": "content", "content": skill_md.clone()}, "confirm": true}),
        )
        .await
        .expect("install ok");
        assert_eq!(install.details["phase"], "installed");
        assert_eq!(install.details["name"], "beta");
        // Atomic write produced the SKILL.md.
        let written = tokio::fs::read_to_string(dir.path().join("beta").join("SKILL.md"))
            .await
            .expect("SKILL.md was written");
        assert_eq!(written, skill_md);
        // Harness catalog now contains the new skill (install path called
        // reload_skills_from_disk internally).
        assert!(
            harness.skills().iter().any(|s| s.name == "beta"),
            "harness catalog must reflect new skill after install"
        );
        // total_skills_after is reported.
        assert!(install.details["total_skills_after"].as_u64().unwrap_or(0) >= 1);
        // Persistent audit was written (QA acceptance — `--resume`/bug-report path).
        let audit_id = install.details["audit_entry_id"].as_str();
        assert!(
            audit_id.is_some_and(|s| !s.is_empty()),
            "audit_entry_id must be set after a successful install, got: {install:?}"
        );
    }

    /// Audit entry shape: persistent `Custom { custom_type: "skill_install" }` records
    /// the metadata QA acceptance asks for (name, target_path, source_kind, before/after
    /// hash, size, overwrite/idempotent flags). Body is NOT included. Read the session
    /// jsonl back through the harness to confirm.
    #[tokio::test]
    async fn install_writes_skill_install_audit_entry() {
        let (harness, cell, dir) = build_test_harness(vec![]);
        let tool = test_tool(cell, &dir);
        let skill_md = make_skill_md("delta", "delta desc", "delta body");

        let _ = execute(
            &tool,
            json!({"source": {"type": "content", "content": skill_md.clone()}, "confirm": true}),
        )
        .await
        .expect("install ok");

        // Walk the session entries and find the `skill_install` Custom record.
        let session = harness.session();
        let entries = session.entries().await.expect("read session entries");
        let custom = entries.iter().find_map(|e| match e {
            theway_core::SessionTreeEntry::Custom {
                custom_type, data, ..
            } if custom_type == "skill_install" => data.clone(),
            _ => None,
        });
        let data = custom.expect("skill_install audit entry must be written");

        assert_eq!(data["status"], "installed");
        assert_eq!(data["name"], "delta");
        assert_eq!(data["source_kind"], "content");
        // Inline content source MUST NOT echo the body into the audit (QA invariant).
        assert!(
            data["source"].is_null(),
            "inline content source must not echo body into audit, got: {}",
            data["source"]
        );
        assert!(
            data["after_hash"].as_str().is_some_and(|s| s.len() == 64),
            "after_hash should be a 64-char SHA256 hex digest"
        );
        assert_eq!(data["before_hash"], Value::Null);
        assert_eq!(data["overwrote"], false);
        assert_eq!(data["idempotent"], false);
        assert_eq!(data["installed_visible_in_catalog"], true);
        // Body must not leak verbatim.
        let serialized = serde_json::to_string(&data).unwrap();
        assert!(
            !serialized.contains("delta body"),
            "audit must not contain skill body, got: {serialized}"
        );
    }

    #[test]
    fn install_permission_reason_uses_whitelisted_source_kind_only() {
        // Provider/Auth + QA gate on PR #139: the prompt reason is a UI/audit-facing field.
        // It must NOT echo any model-supplied substring — `source.type` is normalized
        // through a fixed whitelist (`url` / `https` → "url", `path`, `content`); anything
        // else becomes "<unknown source>".
        let cell: SkillHarnessCell = Arc::new(SyncOnceCell::new());
        let tool = InstallSkillTool::with_skills_root(
            cell,
            std::env::temp_dir().join("install-skill-pc-test"),
        );

        // Known kinds normalize to their bounded label.
        for (input_type, expected) in [
            ("url", "url"),
            ("https", "url"),
            ("path", "path"),
            ("content", "content"),
        ] {
            let cls = tool.permission_classification(&json!({
                "source": { "type": input_type, "url": "ignored-by-reason" },
            }));
            let PermissionClassification::Prompt { reason } = cls else {
                panic!("InstallSkill must always Prompt; got {cls:?}");
            };
            assert!(
                reason.contains(expected),
                "reason must name normalized source kind {expected}; got: {reason}"
            );
            assert!(
                !reason.contains("ignored-by-reason"),
                "reason must not echo source value; got: {reason}"
            );
        }

        // Hostile / model-smuggled source.type collapses to <unknown source>.
        let evil = json!({
            "source": {
                "type": "https://hub.example/api?token=ABCDEFGHIJKLMNOPQRSTUVWXYZ_super_secret",
                "url": "ignored",
            },
        });
        let cls = tool.permission_classification(&evil);
        let PermissionClassification::Prompt { reason } = cls else {
            panic!("InstallSkill must always Prompt; got {cls:?}");
        };
        assert!(
            reason.contains("<unknown source>"),
            "non-whitelisted source.type must normalize to <unknown source>; got: {reason}"
        );
        assert!(
            !reason.contains("token=") && !reason.contains("super_secret"),
            "reason must NOT echo any payload smuggled through source.type; got: {reason}"
        );
    }

    #[test]
    fn url_audit_reference_redacts_secret_bearing_parts() {
        let reference = audit_url_reference(
            "https://user:pass@example.com/token-path/skill.md?api_key=SECRET#frag",
        );
        let serialized = serde_json::to_string(&reference).unwrap();

        assert_eq!(reference["scheme"], "https");
        assert_eq!(reference["host"], "example.com");
        assert_eq!(reference["redacted"], true);
        assert!(
            reference["path_hash"]
                .as_str()
                .is_some_and(|s| s.len() == 64),
            "path hash should be a SHA256 hex digest: {reference}"
        );
        for forbidden in ["user", "pass", "token-path", "api_key", "SECRET", "frag"] {
            assert!(
                !serialized.contains(forbidden),
                "url audit reference leaked {forbidden}: {serialized}"
            );
        }
    }

    /// Atomic write guarantee: a successful write leaves no `.tmp` sibling in the parent dir.
    #[tokio::test]
    async fn atomic_write_leaves_no_temp_artifact_on_success() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("gamma").join("SKILL.md");
        atomic_write_skill(&target, "---\nname: gamma\ndescription: g\n---\nbody\n")
            .await
            .unwrap();
        let mut rd = tokio::fs::read_dir(target.parent().unwrap()).await.unwrap();
        let mut entries = Vec::new();
        while let Some(e) = rd.next_entry().await.unwrap() {
            entries.push(e.file_name().into_string().unwrap_or_default());
        }
        assert_eq!(
            entries,
            vec!["SKILL.md".to_string()],
            "atomic write must not leave a tempfile sibling, got: {entries:?}"
        );
    }
