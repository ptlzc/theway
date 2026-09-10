//! Project/MCP resource provisioning: controller-mode reloads keep provisioned
//! slots, standalone mode still scans local template roots, MCP diagnostics map
//! to `(name, message)` pairs, and provisioned MCP tools join the tool set.

use std::sync::Arc;

use super::*;
use crate::test_env::{ENV_LOCK, EnvGuard};

#[tokio::test]
async fn controller_mode_reload_closure_keeps_provisioned_skills() {
    let base = tempfile::tempdir().unwrap();
    let templates_dir = base.path().join("templates");
    std::fs::create_dir_all(&templates_dir).unwrap();
    std::fs::write(
        templates_dir.join("ignored.md"),
        "---\nname: ignored-template\n---\nbody",
    )
    .unwrap();
    let paths = crate::DaemonPaths {
        base: base.path().to_path_buf(),
        home: base.path().to_path_buf(),
        work_dir: base.path().to_path_buf(),
        extra_skill_dirs: std::sync::Arc::new(std::sync::RwLock::new(Vec::new())),
    };
    // `false` = controller-provisioned: no local disk scan; the catalog
    // comes from the provisioned slot (issue #95).
    let resources = SessionProjectResources::load(&paths, &[], &[], false)
        .await
        .unwrap();
    assert!(
        resources.skills.is_empty(),
        "controller mode must not scan skill files on disk"
    );
    assert!(
        resources.templates.is_empty(),
        "controller mode must not scan template files on disk"
    );

    *resources.provisioned_skills.write().unwrap() = vec![theway_core::Skill {
        name: "provisioned-skill".into(),
        description: "from the controller".into(),
        file_path: "/tmp/provisioned-skill/SKILL.md".into(),
        content: "body".into(),
        disable_model_invocation: false,
        source: theway_core::SkillSource::User,
    }];
    *resources.provisioned_templates.write().unwrap() = vec![theway_core::PromptTemplate {
        name: "provisioned-template".into(),
        description: Some("from the controller".into()),
        file_path: "/tmp/provisioned-template.md".into(),
        content: "template body".into(),
    }];
    let output = (resources.reload_skills_fn)().await;
    assert!(
        output
            .skills
            .iter()
            .any(|skill| skill.name == "provisioned-skill"),
        "reload must carry the provisioned slot forward instead of wiping it"
    );
    assert!(
        resources
            .provisioned_templates
            .read()
            .unwrap()
            .iter()
            .any(|template| template.name == "provisioned-template"),
        "reload must keep the provisioned template slot instead of wiping it"
    );
}

#[cfg(feature = "local")]
#[tokio::test]
async fn standalone_mode_load_scans_local_templates() {
    let base = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let templates_dir = base.path().join("templates");
    std::fs::create_dir_all(&templates_dir).unwrap();
    std::fs::write(
        templates_dir.join("standalone.md"),
        "---\nname: standalone-template\n---\nbody",
    )
    .unwrap();
    let paths = crate::DaemonPaths {
        base: base.path().to_path_buf(),
        home: base.path().to_path_buf(),
        work_dir: work.path().to_path_buf(),
        extra_skill_dirs: std::sync::Arc::new(std::sync::RwLock::new(Vec::new())),
    };
    let resources = SessionProjectResources::load(&paths, &[], &[], true)
        .await
        .unwrap();
    assert!(
        resources
            .templates
            .iter()
            .any(|template| template.name == "standalone-template"),
        "standalone mode must still scan local template roots"
    );
}

/// MCP loader diagnostics map to structured `(name, message)` pairs: server
/// failures keep the server name, config failures keep the file label,
/// unrecognized text falls back to `mcp`.
#[test]
fn parse_mcp_diagnostic_splits_server_and_config_errors() {
    let server = super::super::parse_mcp_diagnostic(
        "mcp server 'devops-mcp' failed: connect timeout",
    );
    assert_eq!(server, ("devops-mcp".to_string(), "connect timeout".to_string()));

    let config = super::super::parse_mcp_diagnostic(
        "mcp config (user, /root/.theway/mcp.toml): parse failed: bad toml",
    );
    assert_eq!(
        config,
        ("mcp.toml (user)".to_string(), "parse failed: bad toml".to_string())
    );

    let other = super::super::parse_mcp_diagnostic("something unexpected");
    assert_eq!(other, ("mcp".to_string(), "something unexpected".to_string()));
}

/// Issue #73: in controller mode the session build reads provisioned MCP
/// tools from the slot — a session started after `Configure` gets the
/// currently connected servers' tools.
#[tokio::test]
async fn build_reads_provisioned_mcp_tools_from_slot() {
    let _serial = ENV_LOCK.lock().unwrap();
    let home = TempDir::new().unwrap();
    let _theway_dir = EnvGuard::set("THEWAY_DIR", home.path());

    let work_dir = TempDir::new().unwrap();
    let repo_root = TempDir::new().unwrap();
    let repo = theway_storage::sqlite_repo::SqliteSessionRepo::new(repo_root.path());
    let id = create_session_with_cwd(&repo, work_dir.path().to_str().unwrap()).await;

    let (factory, storage, _state) = test_factory();

    // Build the context with a provision slot carrying one tool.
    let repo_arc: Arc<dyn SessionRepository> = Arc::new(repo);
    let base_dir = _state.path().join("base");
    let paths = crate::DaemonPaths {
        base: base_dir.clone(),
        home: base_dir.clone(),
        work_dir: base_dir.to_path_buf(),
        extra_skill_dirs: std::sync::Arc::new(std::sync::RwLock::new(Vec::new())),
    }
    .with_work_dir(work_dir.path());
    let resources = SessionProjectResources::load(&paths, &[], &[], true)
        .await
        .unwrap();
    let hooks = SessionHookResources::load(&paths, true).await;

    // A provisioned MCP tool: the adapter stores the client + definition
    // without connecting, so an un-initialized mock-transport client is fine.
    let (_client_side, _server_side) = mcp_transport_pair();
    let client = Arc::new(theway_mcp::McpClient::new(_client_side));
    let provisioned_tool = Arc::new(theway_daemon::tools::mcp_adapter::McpAgentTool::new(
        client,
        &theway_mcp::protocol::McpTool {
            name: "provisioned_tool".into(),
            description: Some("from the provision slot".into()),
            input_schema: serde_json::json!({ "type": "object" }),
        },
    ));
    let slot = std::sync::Arc::new(std::sync::RwLock::new(
        crate::mcp_loader::McpProvisionState {
            tools: vec![provisioned_tool],
            server_names: vec!["provisioned-server".into()],
            tool_names: vec!["provisioned_tool".into()],
            inject_summary: ["inject-me".into()].into_iter().collect(),
            ..Default::default()
        },
    ));
    let mcp_resources = SessionMcpResources {
        provision: Some(slot.clone()),
        ..SessionMcpResources::default()
    };

    let ctx = SessionExecutionContext::new(
        "test-context",
        work_dir.path().to_path_buf(),
        repo_arc,
        storage,
        paths,
        crate::executor::executor_for_cwd(work_dir.path()),
        theway_core::executor::ExecutorKind::Local,
        faux_model(),
        theway_core::ThinkingLevel::Off,
        resources,
        mcp_resources,
        hooks,
    );

    let runtime = factory.build(&ctx, &id).await.expect("session builds");
    assert!(
        runtime
            .tool_names
            .iter()
            .any(|name| name == "provisioned_tool"),
        "slot tools must join the session tool set: {:?}",
        runtime.tool_names
    );
}

/// Pipe-transport pair mirroring `theway-mcp`'s mock pattern — the client
/// is never initialized in these tests, the adapter just needs the handle.
fn mcp_transport_pair() -> (
    Arc<dyn theway_mcp::Transport>,
    Arc<dyn theway_mcp::Transport>,
) {
    use tokio::sync::mpsc;
    struct PipeTransport {
        tx: tokio::sync::Mutex<mpsc::UnboundedSender<String>>,
        rx: tokio::sync::Mutex<mpsc::UnboundedReceiver<String>>,
    }
    #[async_trait::async_trait]
    impl theway_mcp::Transport for PipeTransport {
        async fn send_line(&self, line: String) -> Result<(), theway_mcp::McpError> {
            self.tx
                .lock()
                .await
                .send(line)
                .map_err(|e| theway_mcp::McpError::Transport(e.to_string()))
        }
        async fn recv_line(&self) -> Result<Option<String>, theway_mcp::McpError> {
            Ok(self.rx.lock().await.recv().await)
        }
        async fn close(&self) {}
    }
    let (a_tx, b_rx) = mpsc::unbounded_channel();
    let (b_tx, a_rx) = mpsc::unbounded_channel();
    let a = PipeTransport {
        tx: tokio::sync::Mutex::new(a_tx),
        rx: tokio::sync::Mutex::new(a_rx),
    };
    let b = PipeTransport {
        tx: tokio::sync::Mutex::new(b_tx),
        rx: tokio::sync::Mutex::new(b_rx),
    };
    (Arc::new(a), Arc::new(b))
}
