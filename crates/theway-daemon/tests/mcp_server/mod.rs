//! Mirrored unit tests for the private MCP `ToolDispatcher` helpers.
//!
//! The full stdio handshake is covered by `tests/mcp_e2e.rs`; these tests focus
//! on the pure mapping helpers that the e2e path also exercises.

use std::sync::Arc;

use rmcp::model::Implementation;
use theway_core::{AgentTool, AgentToolError, AgentToolResult};
use theway_llm_provider::{Tool, UserContentBlock};

use super::*;

struct FakeTool {
    def: Tool,
    result: AgentToolResult,
}


impl FakeTool {
    fn ok(name: &str) -> Self {
        Self {
            def: Tool {
                name: name.into(),
                description: format!("{name} description"),
                parameters: serde_json::json!({ "type": "object" }),
            },
            result: AgentToolResult {
                content: vec![UserContentBlock::text("ok")],
                details: serde_json::Value::Null,
                terminate: None,
            },
        }
    }
}

#[async_trait::async_trait]
impl AgentTool for FakeTool {
    fn definition(&self) -> &Tool {
        &self.def
    }

    fn label(&self) -> &str {
        "fake"
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        _params: serde_json::Value,
        _cancel: tokio_util::sync::CancellationToken,
        _on_update: Option<theway_core::AgentToolUpdate>,
    ) -> Result<AgentToolResult, AgentToolError> {
        Ok(self.result.clone())
    }
}

#[test]
fn dispatcher_find_locates_tool_by_name() {
    let dispatcher = ToolDispatcher {
        tools: vec![Arc::new(FakeTool::ok("alpha"))],
    };

    assert!(dispatcher.find("alpha").is_some());
    assert!(dispatcher.find("beta").is_none());
}

#[test]
fn mcp_tool_uses_definition_schema_and_falls_back_to_empty_object() {
    let dispatcher = ToolDispatcher {
        tools: vec![Arc::new(FakeTool::ok("schema-tool"))],
    };
    let tool = dispatcher.mcp_tool(dispatcher.tools[0].as_ref());
    assert_eq!(tool.name.as_ref(), "schema-tool");
    assert_eq!(tool.description.as_deref(), Some("schema-tool description"));
    assert_eq!(
        *tool.input_schema,
        serde_json::json!({ "type": "object" })
            .as_object()
            .cloned()
            .unwrap_or_default()
    );

    let bad = FakeTool {
        def: Tool {
            name: "bad-schema".into(),
            description: String::new(),
            parameters: serde_json::json!("not-an-object"),
        },
        result: AgentToolResult::default(),
    };
    let tool = dispatcher.mcp_tool(&bad);
    assert!(tool.input_schema.is_empty());
}

#[test]
fn render_tool_result_joins_text_blocks_and_details() {
    let result = AgentToolResult {
        content: vec![
            UserContentBlock::text("hello"),
            UserContentBlock::text("world"),
        ],
        details: serde_json::json!({ "exit_code": 0 }),
        terminate: None,
    };

    let text = render_tool_result(&result);

    assert_eq!(text, "hello\nworld\n{\"exit_code\":0}\n");
}

#[test]
fn render_tool_result_formats_non_text_blocks() {
    let result = AgentToolResult {
        content: vec![UserContentBlock::Image(theway_llm_provider::ImageContent {
            data: String::new(),
            mime_type: "image/png".into(),
        })],
        details: serde_json::Value::Null,
        terminate: None,
    };

    let text = render_tool_result(&result);

    assert!(text.contains("Image"), "{text}");
    assert!(text.ends_with('\n'), "{text}");
}

#[test]
fn get_info_reports_theway_implementation() {
    let dispatcher = ToolDispatcher { tools: vec![] };
    let info = dispatcher.get_info();
    assert_eq!(info.server_info.name, "theway");
    assert_eq!(
        info.server_info,
        Implementation::new("theway", env!("CARGO_PKG_VERSION"))
    );
    assert!(info.instructions.is_none());
}
