#![cfg(feature = "local")]

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;
use tempfile::TempDir;
use theway_core::{AgentTool, AgentToolError, AgentToolResult, AgentToolUpdate};
use theway_llm_provider::{Tool, UserContentBlock};
use tokio_util::sync::CancellationToken;

use super::*;
use crate::orchestration::SessionRuntime;
use crate::test_env::{ENV_LOCK, EnvGuard};

struct FakeMcpTool {
    definition: Tool,
}

impl FakeMcpTool {
    fn new(name: &str) -> Self {
        Self {
            definition: Tool {
                name: name.to_string(),
                description: format!("{name} fake"),
                parameters: json!({ "type": "object" }),
            },
        }
    }
}

#[async_trait]
impl AgentTool for FakeMcpTool {
    fn definition(&self) -> &Tool {
        &self.definition
    }

    fn label(&self) -> &str {
        self.definition.name.as_str()
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        _params: serde_json::Value,
        _cancel: CancellationToken,
        _on_update: Option<AgentToolUpdate>,
    ) -> Result<AgentToolResult, AgentToolError> {
        Ok(AgentToolResult::default())
    }
}

fn bash_tool(runtime: &SessionRuntime) -> Arc<dyn AgentTool> {
    runtime
        .harness
        .agent()
        .state()
        .tools
        .iter()
        .find(|tool| tool.definition().name == "bash")
        .expect("built local runtime must expose a real bash tool")
        .clone()
}

fn result_text(result: &AgentToolResult) -> String {
    match &result.content[0] {
        UserContentBlock::Text(text) => text.text.clone(),
        _ => panic!("expected text content"),
    }
}

async fn run_pwd(tool: &Arc<dyn AgentTool>) -> String {
    result_text(
        &tool
            .execute("pwd", json!({ "command": "pwd" }), CancellationToken::new(), None)
            .await
            .expect("bash pwd executes"),
    )
}

#[tokio::test]
async fn built_runtimes_keep_bash_and_mcp_tools_cwd_isolated() {
    let _serial = ENV_LOCK.lock().unwrap();
    let home = TempDir::new().unwrap();
    let _theway_dir = EnvGuard::set("THEWAY_DIR", home.path());

    let work_a = TempDir::new().unwrap();
    let work_b = TempDir::new().unwrap();
    let base_a = TempDir::new().unwrap();
    let base_b = TempDir::new().unwrap();
    let repo_root_a = TempDir::new().unwrap();
    let repo_root_b = TempDir::new().unwrap();
    let repo_a = theway_storage::sqlite_repo::SqliteSessionRepo::new(repo_root_a.path());
    let repo_b = theway_storage::sqlite_repo::SqliteSessionRepo::new(repo_root_b.path());
    let id_a = create_session_with_cwd(&repo_a, work_a.path().to_str().unwrap()).await;
    let id_b = create_session_with_cwd(&repo_b, work_b.path().to_str().unwrap()).await;

    let (factory, storage, _state) = test_factory();
    let mut ctx_a = session_context(work_a.path(), repo_a, storage.clone(), base_a.path()).await;
    ctx_a.mcp.tools.push(Arc::new(FakeMcpTool::new("mcp-a")));
    let mut ctx_b = session_context(work_b.path(), repo_b, storage, base_b.path()).await;
    ctx_b.mcp.tools.push(Arc::new(FakeMcpTool::new("mcp-b")));

    let runtime_a = factory
        .build(&ctx_a, &id_a)
        .await
        .expect("cwd A runtime builds");
    let runtime_b = factory
        .build(&ctx_b, &id_b)
        .await
        .expect("cwd B runtime builds");

    let cwd_a = runtime_a.cwd.to_string_lossy().into_owned();
    let cwd_b = runtime_b.cwd.to_string_lossy().into_owned();
    let bash_a = bash_tool(&runtime_a);
    let bash_b = bash_tool(&runtime_b);
    let (out_a, out_b) = tokio::join!(run_pwd(&bash_a), run_pwd(&bash_b));

    assert!(
        out_a.contains(&cwd_a) && !out_a.contains(&cwd_b),
        "runtime A bash must stay in its own cwd: {out_a}"
    );
    assert!(
        out_b.contains(&cwd_b) && !out_b.contains(&cwd_a),
        "runtime B bash must stay in its own cwd: {out_b}"
    );

    assert!(runtime_a.tool_names.iter().any(|name| name == "mcp-a"));
    assert!(!runtime_a.tool_names.iter().any(|name| name == "mcp-b"));
    assert!(runtime_b.tool_names.iter().any(|name| name == "mcp-b"));
    assert!(!runtime_b.tool_names.iter().any(|name| name == "mcp-a"));
}
