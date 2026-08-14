use super::*;
use crate::{AgentToolError, AgentToolResult, AgentToolUpdate};

/// Minimal named tool — `filter_tool_set` only ever reads `definition().name`.
struct NamedTool {
    def: theway_llm_provider::Tool,
}

impl NamedTool {
    fn arc(name: &str) -> Arc<dyn AgentTool> {
        Arc::new(Self {
            def: theway_llm_provider::Tool {
                name: name.to_string(),
                description: String::new(),
                parameters: serde_json::Value::Null,
            },
        })
    }
}

#[async_trait::async_trait]
impl AgentTool for NamedTool {
    fn definition(&self) -> &theway_llm_provider::Tool {
        &self.def
    }

    fn label(&self) -> &str {
        &self.def.name
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        _params: serde_json::Value,
        _cancel: CancellationToken,
        _on_update: Option<AgentToolUpdate>,
    ) -> Result<AgentToolResult, AgentToolError> {
        Err(AgentToolError::Message("not exercised by filter tests".into()))
    }
}

fn tool_set(names: &[&str]) -> Vec<Arc<dyn AgentTool>> {
    names.iter().map(|n| NamedTool::arc(n)).collect()
}

fn names(tools: &[Arc<dyn AgentTool>]) -> Vec<String> {
    tools.iter().map(|t| t.definition().name.clone()).collect()
}

#[test]
fn filter_tool_set_empty_allow_returns_full_set() {
    let out = filter_tool_set(tool_set(&["read", "bash", "grep"]), &[]).unwrap();
    assert_eq!(names(&out), vec!["read", "bash", "grep"]);
}

#[test]
fn filter_tool_set_subset_allow_keeps_only_allowed_tools() {
    let out = filter_tool_set(tool_set(&["read", "bash"]), &["bash".to_string()]).unwrap();
    assert_eq!(names(&out), vec!["bash"]);
}

#[test]
fn filter_tool_set_unknown_name_fails_and_lists_available() {
    // `unwrap_err` needs `T: Debug`; the tool set is not Debug, so match.
    let err = match filter_tool_set(tool_set(&["read", "bash"]), &["nope".to_string()]) {
        Err(e) => e,
        Ok(_) => panic!("unknown allowlist name must fail"),
    };
    assert!(
        err.contains("unknown tool in allowlist: nope"),
        "error must name the unknown entry: {err}"
    );
    assert!(
        err.contains("read") && err.contains("bash"),
        "error must list the available names: {err}"
    );
}

#[test]
fn filter_tool_set_result_keeps_tool_set_order() {
    let out = filter_tool_set(
        tool_set(&["alpha", "beta", "gamma"]),
        &["gamma".to_string(), "alpha".to_string()],
    )
    .unwrap();
    assert_eq!(names(&out), vec!["alpha", "gamma"]);
}
