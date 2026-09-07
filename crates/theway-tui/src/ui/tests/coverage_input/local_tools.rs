use super::*;

// ── LocalToolOps ─────────────────────────────────────────────────────

#[tokio::test]
async fn local_tool_ops_filesystem_grep_find_exec() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().to_path_buf();
    let ops = LocalToolOps::new(dir.clone());

    std::fs::write(dir.join("a.txt"), "one\ntwo\nthree\n").unwrap();
    std::fs::create_dir_all(dir.join("sub/deep")).unwrap();
    std::fs::write(dir.join("sub/deep/b.rs"), "fn FOO() {}\nline two\n").unwrap();
    std::fs::write(dir.join("sub/notes.txt"), "foo bar\n").unwrap();
    std::fs::write(dir.join("q1.txt"), "question\n").unwrap();

    // resolve reads relative and absolute paths.
    let rel = ops
        .read_file(&WireToolReadRequest {
            path: "a.txt".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(rel.content, "one\ntwo\nthree");
    assert_eq!(rel.total_lines, 3);
    assert!(!rel.truncated);

    let abs = ops
        .read_file(&WireToolReadRequest {
            path: dir.join("a.txt").display().to_string(),
            offset: Some(2),
            limit: None,
        })
        .await
        .unwrap();
    assert_eq!(abs.content, "two\nthree");

    // pagination: limited window truncates; an absurd offset returns empty.
    let page = ops
        .read_file(&WireToolReadRequest {
            path: "a.txt".into(),
            offset: Some(1),
            limit: Some(2),
        })
        .await
        .unwrap();
    assert_eq!(page.content, "one\ntwo");
    assert!(page.truncated);

    let empty = ops
        .read_file(&WireToolReadRequest {
            path: "a.txt".into(),
            offset: Some(100),
            limit: Some(2),
        })
        .await
        .unwrap();
    assert_eq!(empty.content, "");
    assert!(!empty.truncated);

    // missing file -> NotFound.
    let err = ops
        .read_file(&WireToolReadRequest {
            path: "missing.txt".into(),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert!(err.to_string().contains("not found: "), "{err}");

    // write with a new nested parent and with an existing parent.
    let written = ops
        .write_file(&WireToolWriteRequest {
            path: "nested/deep/out.txt".into(),
            content: "hello".into(),
        })
        .await
        .unwrap();
    assert_eq!(written.bytes_written, 5);
    let written2 = ops
        .write_file(&WireToolWriteRequest {
            path: "plain.txt".into(),
            content: "world".into(),
        })
        .await
        .unwrap();
    assert_eq!(written2.bytes_written, 5);

    // edit: first-only, replace-all, and no-match.
    std::fs::write(dir.join("edit.txt"), "foo bar foo").unwrap();
    let edit1 = ops
        .edit_file(&WireToolEditRequest {
            path: "edit.txt".into(),
            old_string: "foo".into(),
            new_string: "X".into(),
            replace_all: false,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(edit1.replacements, 1);
    let edit2 = ops
        .edit_file(&WireToolEditRequest {
            path: "edit.txt".into(),
            old_string: "foo".into(),
            new_string: "Y".into(),
            replace_all: true,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(edit2.replacements, 1); // only the remaining "foo"
    let edit3 = ops
        .edit_file(&WireToolEditRequest {
            path: "edit.txt".into(),
            old_string: "zzz".into(),
            new_string: "nope".into(),
            replace_all: false,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(edit3.replacements, 0);

    let edit_err = ops
        .edit_file(&WireToolEditRequest {
            path: "missing-edit.txt".into(),
            old_string: "x".into(),
            new_string: "y".into(),
            replace_all: false,
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert!(edit_err.to_string().contains("not found"), "{edit_err}");

    // list_dir: mixed kinds and a limit.
    let listed = ops
        .list_dir(&WireToolListDirRequest {
            path: ".".into(),
            limit: None,
        })
        .await
        .unwrap();
    assert!(listed.entries.iter().any(|e| e.name == "a.txt"));
    assert!(listed.entries.iter().any(|e| e.name == "sub"));
    let limited = ops
        .list_dir(&WireToolListDirRequest {
            path: dir.display().to_string(),
            limit: Some(1),
        })
        .await
        .unwrap();
    assert!(limited.entries.len() <= 1);

    let list_err = ops
        .list_dir(&WireToolListDirRequest {
            path: "missing-dir".into(),
            limit: None,
        })
        .await
        .unwrap_err();
    assert!(list_err.to_string().contains("not found"), "{list_err}");

    // grep: case-insensitive content, glob filtering, files/count modes,
    // invalid regex, and path-omitted default root.
    let content = ops
        .grep(&WireToolGrepRequest {
            pattern: "foo".into(),
            path: None,
            case_insensitive: true,
            output_mode: Some("content".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(content.matches.iter().any(|m| m.path.ends_with("b.rs")));
    assert!(content.matches.iter().any(|m| m.line.contains("FOO")));
    assert!(!content.matches.is_empty());

    let files = ops
        .grep(&WireToolGrepRequest {
            pattern: "^fn ".into(),
            path: Some(dir.join("sub/deep").display().to_string()),
            glob_filter: Some("*.rs".into()),
            output_mode: Some("files_with_matches".into()),
            max_results: Some(10),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(files.files.iter().any(|p| p.ends_with("b.rs")));

    let counts = ops
        .grep(&WireToolGrepRequest {
            pattern: "foo".into(),
            path: Some(dir.display().to_string()),
            output_mode: Some("count".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(counts.counts.iter().any(|c| c.count >= 1));

    let bad = ops
        .grep(&WireToolGrepRequest {
            pattern: "[".into(),
            path: Some(dir.display().to_string()),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert!(bad.to_string().contains("bad regex"), "{bad}");

    let filtered = ops
        .grep(&WireToolGrepRequest {
            pattern: "foo".into(),
            path: Some(dir.display().to_string()),
            glob_filter: Some("*.nomatch".into()),
            output_mode: Some("files_with_matches".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(filtered.files.is_empty());

    // find: recursive glob, root/default and limit.
    let find_all = ops
        .find(&WireToolFindRequest {
            pattern: "*.rs".into(),
            path: Some(dir.display().to_string()),
            limit: None,
        })
        .await
        .unwrap();
    assert!(find_all.paths.iter().any(|p| p.ends_with("b.rs")));

    let find_limited = ops
        .find(&WireToolFindRequest {
            pattern: "*.txt".into(),
            path: None,
            limit: Some(1),
        })
        .await
        .unwrap();
    assert!(find_limited.paths.len() <= 1);

    let find_question = ops
        .find(&WireToolFindRequest {
            pattern: "q?.txt".into(),
            path: Some(dir.display().to_string()),
            limit: None,
        })
        .await
        .unwrap();
    assert!(find_question.paths.iter().any(|p| p.ends_with("q1.txt")));

    // exec: normal cwd + stream output/exit; timed-out branch via sleep.
    let mut stream = ops
        .exec_command(&WireToolExecRequest {
            command: "echo hello".into(),
            cwd: Some(dir.display().to_string()),
            timeout_ms: None,
        })
        .await
        .unwrap();
    let mut saw_output = false;
    let mut saw_exit = false;
    while let Some(frame) = stream.next().await {
        match frame {
            WireToolExecFrame::Output { text } => {
                saw_output |= text.contains("hello");
            }
            WireToolExecFrame::Exit {
                code, timed_out, ..
            } => {
                saw_exit = true;
                assert!(!timed_out);
                assert_eq!(code, 0);
            }
        }
    }
    assert!(saw_output);
    assert!(saw_exit);

    let mut timeout_stream = ops
        .exec_command(&WireToolExecRequest {
            command: "exec sleep 5 >/dev/null 2>&1".into(),
            cwd: Some(dir.display().to_string()),
            timeout_ms: Some(50),
        })
        .await
        .unwrap();
    let mut timeout_exit = false;
    while let Some(frame) = timeout_stream.next().await {
        if let WireToolExecFrame::Exit {
            timed_out, code, ..
        } = frame
        {
            timeout_exit = true;
            assert!(timed_out);
            assert_eq!(code, -1);
        }
    }
    assert!(timeout_exit);
}

// Use the shared process-wide env lock so this suite serializes with the
// CLI/theme tests that also mutate THEWAY_DIR.

struct EnvGuard {
    key: &'static str,
    original: Option<std::ffi::OsString>,
}

impl EnvGuard {
    fn set(key: &'static str, value: &Path) -> Self {
        let original = std::env::var_os(key);
        // SAFETY: ENV_LOCK serializes this test's env mutation.
        unsafe { std::env::set_var(key, value) };
        Self { key, original }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match self.original.take() {
            Some(value) => unsafe { std::env::set_var(self.key, value) },
            None => unsafe { std::env::remove_var(self.key) },
        }
    }
}

#[tokio::test]
async fn local_tool_ops_memory_and_skill_install() {
    let _guard = crate::ui::tests::common::ENV_LOCK.lock().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().join("base");
    std::fs::create_dir_all(&base).unwrap();
    let _theway = EnvGuard::set("THEWAY_DIR", &base);
    let ops = LocalToolOps::new(tmp.path().to_path_buf());

    // save with full metadata, then a bare save.
    let saved = ops
        .memory_save(&WireToolMemorySaveRequest {
            name: "prefs".into(),
            content: "dark".into(),
            description: Some("ui".into()),
            memory_type: Some("preference".into()),
        })
        .await
        .unwrap();
    assert_eq!(saved.name, "prefs");
    let bare = ops
        .memory_save(&WireToolMemorySaveRequest {
            name: "bare".into(),
            content: "body".into(),
            description: None,
            memory_type: None,
        })
        .await
        .unwrap();
    assert_eq!(bare.name, "bare");
    let special = ops
        .memory_save(&WireToolMemorySaveRequest {
            name: "bad name!".into(),
            content: "special".into(),
            description: None,
            memory_type: None,
        })
        .await
        .unwrap();
    assert_eq!(special.name, "bad name!");
    let special_read = ops
        .memory_read(&WireToolMemoryReadRequest {
            name: "bad-name-".into(),
        })
        .await
        .unwrap();
    assert_eq!(special_read.content, "special");

    // list + read + read missing.
    let listed = ops
        .memory_list(&WireToolMemoryListRequest {})
        .await
        .unwrap();
    assert!(listed.entries.iter().any(|e| e.name == "prefs"));
    let read = ops
        .memory_read(&WireToolMemoryReadRequest {
            name: "prefs".into(),
        })
        .await
        .unwrap();
    assert_eq!(
        read.content,
        "---\ndescription: ui\ntype: preference\n---\n\ndark"
    );
    let missing = ops
        .memory_read(&WireToolMemoryReadRequest {
            name: "missing".into(),
        })
        .await
        .unwrap_err();
    assert!(missing.to_string().contains("not found"), "{missing}");

    // forget existing then forget again (NotFound path).
    let forgot = ops
        .memory_forget(&WireToolMemoryForgetRequest {
            name: "bare".into(),
        })
        .await
        .unwrap();
    assert!(forgot.removed);
    let forgotten_again = ops
        .memory_forget(&WireToolMemoryForgetRequest {
            name: "bare".into(),
        })
        .await
        .unwrap();
    assert!(!forgotten_again.removed);

    // skill: URL rejected; content preview then install; path install;
    // existing/overwrite paths; no-heading fallback.
    let url_err = ops
        .skill_install(&WireToolSkillInstallRequest {
            source: WireToolSkillSource::Url("https://example.com/s.md".into()),
            confirm: false,
            overwrite: false,
        })
        .await
        .unwrap_err();
    assert!(url_err.to_string().contains("not supported"), "{url_err}");

    let preview = ops
        .skill_install(&WireToolSkillInstallRequest {
            source: WireToolSkillSource::Content("# My Skill\nbody".into()),
            confirm: false,
            overwrite: false,
        })
        .await
        .unwrap();
    assert_eq!(preview.name, "my-skill");
    assert!(!preview.installed);
    assert!(preview.warning.is_some());

    let install = ops
        .skill_install(&WireToolSkillInstallRequest {
            source: WireToolSkillSource::Content("# My Skill\nbody".into()),
            confirm: true,
            overwrite: false,
        })
        .await
        .unwrap();
    assert!(install.installed);
    assert!(!install.existing); // first real install creates the target

    let existing_err = ops
        .skill_install(&WireToolSkillInstallRequest {
            source: WireToolSkillSource::Content("# My Skill\nbody".into()),
            confirm: true,
            overwrite: false,
        })
        .await
        .unwrap_err();
    assert!(
        existing_err.to_string().contains("already exists"),
        "{existing_err}"
    );

    let overwrite = ops
        .skill_install(&WireToolSkillInstallRequest {
            source: WireToolSkillSource::Content("# My Skill\nreplaced".into()),
            confirm: true,
            overwrite: true,
        })
        .await
        .unwrap();
    assert!(overwrite.installed);

    let skill_file = tmp.path().join("path-skill.md");
    std::fs::write(&skill_file, "# Path Skill\ncontent").unwrap();
    let path_install = ops
        .skill_install(&WireToolSkillInstallRequest {
            source: WireToolSkillSource::Path(skill_file.display().to_string()),
            confirm: true,
            overwrite: false,
        })
        .await
        .unwrap();
    assert_eq!(path_install.name, "path-skill");

    let path_missing_ops = LocalToolOps::new(tmp.path().to_path_buf());
    let path_missing = path_missing_ops
        .skill_install(&WireToolSkillInstallRequest {
            source: WireToolSkillSource::Path("no-such-skill.md".into()),
            confirm: true,
            overwrite: false,
        })
        .await
        .unwrap_err();
    assert!(
        path_missing.to_string().contains("not found"),
        "{path_missing}"
    );

    let no_heading = ops
        .skill_install(&WireToolSkillInstallRequest {
            source: WireToolSkillSource::Content("no heading here".into()),
            confirm: true,
            overwrite: false,
        })
        .await
        .unwrap();
    assert_eq!(no_heading.name, "skill");
}
