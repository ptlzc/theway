//! Cwd-scoped direct-OS tool tests: separate tool sets isolate bash/exec/ls/grep/find;
//! explicit caller args stay authoritative; no-cwd compatibility keeps process cwd.

use std::sync::Arc;

use serde_json::{Value, json};
use theway_core::executor::ToolExecutor;
use theway_core::{AgentTool, AgentToolResult};
use theway_llm_provider::UserContentBlock;
use tokio_util::sync::CancellationToken;

use crate::tools::{local_tools, local_tools_for_cwd};

fn local_exec() -> Arc<dyn ToolExecutor> {
    Arc::new(crate::executor::local::LocalExecutor::new())
}

fn tool<'a>(tools: &'a [Arc<dyn AgentTool>], name: &str) -> &'a Arc<dyn AgentTool> {
    tools
        .iter()
        .find(|t| t.definition().name == name)
        .unwrap_or_else(|| panic!("missing {name}"))
}

fn text(result: &AgentToolResult) -> String {
    match &result.content[0] {
        UserContentBlock::Text(t) => t.text.clone(),
        _ => panic!("expected text"),
    }
}

async fn call(tool: &Arc<dyn AgentTool>, params: Value) -> String {
    text(
        &tool
            .execute("cwd", params, CancellationToken::new(), None)
            .await
            .expect("tool"),
    )
}

#[tokio::test]
async fn cwd_scoped_tool_sets_isolate_direct_os_tools() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let ca = a.path().canonicalize().unwrap().to_string_lossy().into_owned();
    let cb = b.path().canonicalize().unwrap().to_string_lossy().into_owned();
    std::fs::write(a.path().join("a.rs"), "alpha\n").unwrap();
    std::fs::write(b.path().join("b.rs"), "beta\n").unwrap();
    let ta = local_tools_for_cwd(local_exec(), a.path().to_path_buf());
    let tb = local_tools_for_cwd(local_exec(), b.path().to_path_buf());

    for (set, mine, other) in [(&ta, &ca, &cb), (&tb, &cb, &ca)] {
        let bash = call(tool(set, "bash"), json!({ "command": "pwd" })).await;
        assert!(bash.contains(mine.as_str()) && !bash.contains(other.as_str()), "{bash}");
        let exec = call(tool(set, "exec"), json!({ "command": "pwd" })).await;
        assert!(exec.contains(mine.as_str()) && !exec.contains(other.as_str()), "{exec}");
        let bg = call(
            tool(set, "exec"),
            json!({ "command": "pwd", "run_in_background": true }),
        )
        .await;
        let id = format!(
            "shell-{}",
            bg.split("shell-").nth(1).unwrap().split_whitespace().next().unwrap()
        );
        let out = call(tool(set, "get_output"), json!({ "shell_id": id, "timeout": 5 })).await;
        assert!(out.contains(mine.as_str()) && !out.contains(other.as_str()), "{out}");
        let _ = call(tool(set, "kill_shell"), json!({ "shell_id": id })).await;
    }

    let ls_a = call(tool(&ta, "ls"), json!({})).await;
    assert!(ls_a.starts_with(&format!("{ca} (1 entries)")) && !ls_a.contains("b.rs"), "{ls_a}");
    let ls_b = call(tool(&tb, "ls"), json!({})).await;
    assert!(ls_b.starts_with(&format!("{cb} (1 entries)")) && !ls_b.contains("a.rs"), "{ls_b}");
    let grep_a = call(tool(&ta, "grep"), json!({ "pattern": "alpha" })).await;
    assert!(grep_a.contains("a.rs") && !grep_a.contains("b.rs"), "{grep_a}");
    let grep_b = call(tool(&tb, "grep"), json!({ "pattern": "beta" })).await;
    assert!(grep_b.contains("b.rs") && !grep_b.contains("a.rs"), "{grep_b}");
    let find_a = call(tool(&ta, "find"), json!({ "glob": "*.rs" })).await;
    assert!(find_a.contains("a.rs") && !find_a.contains("b.rs"), "{find_a}");
    let find_b = call(tool(&tb, "find"), json!({ "glob": "*.rs" })).await;
    assert!(find_b.contains("b.rs") && !find_b.contains("a.rs"), "{find_b}");

    let overridden = call(tool(&ta, "ls"), json!({ "cwd": cb.clone() })).await;
    assert!(overridden.starts_with(&format!("{cb} (1 entries)")), "{overridden}");
}

#[tokio::test]
async fn no_cwd_compatibility_keeps_process_cwd_behavior() {
    let cwd = std::env::current_dir().unwrap();
    let tools = local_tools(local_exec());
    let bash = call(tool(&tools, "bash"), json!({ "command": "pwd" })).await;
    assert!(bash.contains(&cwd.to_string_lossy().to_string()), "{bash}");

    let direct_ls = crate::tools::ls::LsTool
        .execute("legacy", json!({}), CancellationToken::new(), None)
        .await
        .unwrap();
    assert!(text(&direct_ls).starts_with(". ("), "{}", text(&direct_ls));
}
