//! MCP stdio server exposing the shared non-streaming external service
//! (`ExternalProtocolOps`) through a static tool manifest. `tools/list` is
//! generated from the manifest; `tools/call` routes to the exact same ops
//! object used by gRPC and JSON-RPC, so all three protocols return the same
//! business results.

use std::sync::Arc;

use rmcp::model::ErrorData as McpError;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ErrorCode,
    Implementation, ListToolsResult, PaginatedRequestParams, ServerInfo, TextContent, Tool,
};
use rmcp::service::{RequestContext, RoleServer, serve_server};
use rmcp::{ServerHandler, transport::io::stdio};
use serde_json::{Value, json};
use theway_transport::transport::{GraphOps, JobOps, SessionOps};
use theway_transport::{ExternalProtocolOps, ListSessionMessagesRequest};

/// Run the MCP stdio server backed by the shared external service. Blocks
/// until stdin closes. `job_ops` backs the one node-output read that lives
/// outside the non-streaming service boundary.
pub async fn run_mcp_server(
    ops: Arc<dyn ExternalProtocolOps>,
    job_ops: Arc<dyn JobOps>,
) -> anyhow::Result<()> {
    let dispatcher = ToolDispatcher { ops, job_ops };
    let (stdin, stdout) = stdio();
    let service = serve_server(dispatcher, (stdin, stdout)).await?;
    service.waiting().await?;
    Ok(())
}

/// One entry of the static tool manifest.
struct ToolSpec {
    name: &'static str,
    description: &'static str,
    schema: Value,
}

/// Static manifest: MCP tools mirror the shared service's JSON-RPC methods.
fn tool_specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "session_list",
            description: "List sessions (oldest → newest).",
            schema: json!({ "type": "object", "properties": {} }),
        },
        ToolSpec {
            name: "session_create",
            description: "Create a session.",
            schema: json!({
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" },
                    "name": { "type": "string" }
                }
            }),
        },
        ToolSpec {
            name: "session_rename",
            description: "Rename a session (full id or unique prefix).",
            schema: json!({
                "type": "object",
                "required": ["id", "name"],
                "properties": {
                    "id": { "type": "string" },
                    "name": { "type": "string" }
                }
            }),
        },
        ToolSpec {
            name: "session_delete",
            description: "Delete a session; refused while it has running graphs.",
            schema: json!({
                "type": "object",
                "required": ["id"],
                "properties": { "id": { "type": "string" } }
            }),
        },
        ToolSpec {
            name: "session_get_snapshot",
            description: "Authoritative current session snapshot.",
            schema: json!({
                "type": "object",
                "properties": { "session_id": { "type": "string" } }
            }),
        },
        ToolSpec {
            name: "session_list_messages",
            description: "Cursor-paginated full message history.",
            schema: json!({
                "type": "object",
                "required": ["session_id"],
                "properties": {
                    "session_id": { "type": "string" },
                    "limit": { "type": "integer", "minimum": 1 },
                    "before_entry_id": { "type": "string" }
                }
            }),
        },
        ToolSpec {
            name: "graph_list",
            description: "List DAG/goal runs of a session.",
            schema: json!({
                "type": "object",
                "required": ["session_id"],
                "properties": { "session_id": { "type": "string" } }
            }),
        },
        ToolSpec {
            name: "graph_clear",
            description: "Clear the terminal DAG/goal runs of a session, keeping running runs.",
            schema: json!({
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" },
                    "keep": { "type": "integer" }
                }
            }),
        },
        ToolSpec {
            name: "graph_cancel",
            description: "Cancel a DAG/goal run.",
            schema: json!({
                "type": "object",
                "required": ["run_id"],
                "properties": { "run_id": { "type": "string" } }
            }),
        },
        ToolSpec {
            name: "graph_retry",
            description: "Retry a run (optionally one node).",
            schema: json!({
                "type": "object",
                "required": ["run_id"],
                "properties": {
                    "run_id": { "type": "string" },
                    "node_id": { "type": "string" }
                }
            }),
        },
        ToolSpec {
            name: "graph_skip",
            description: "Skip one run node.",
            schema: json!({
                "type": "object",
                "required": ["run_id", "node_id"],
                "properties": {
                    "run_id": { "type": "string" },
                    "node_id": { "type": "string" }
                }
            }),
        },
        ToolSpec {
            name: "get_node_output",
            description: "Read a graph node's output and structured messages.",
            schema: json!({
                "type": "object",
                "required": ["run_id", "node_id"],
                "properties": {
                    "run_id": { "type": "string" },
                    "node_id": { "type": "string" },
                    "offset": { "type": "integer", "minimum": 0 }
                }
            }),
        },
        ToolSpec {
            name: "tool_read",
            description: "Read a file as UTF-8 text with line pagination.",
            schema: json!({
                "type": "object",
                "required": ["path"],
                "properties": {
                    "path": { "type": "string" },
                    "offset": { "type": "integer", "minimum": 1 },
                    "limit": { "type": "integer", "minimum": 1 }
                }
            }),
        },
        ToolSpec {
            name: "tool_write",
            description: "Create/overwrite a file.",
            schema: json!({
                "type": "object",
                "required": ["path", "content"],
                "properties": {
                    "path": { "type": "string" },
                    "content": { "type": "string" }
                }
            }),
        },
        ToolSpec {
            name: "tool_edit",
            description: "Search-and-replace edit a file.",
            schema: json!({
                "type": "object",
                "required": ["path", "old_text", "new_text"],
                "properties": {
                    "path": { "type": "string" },
                    "old_text": { "type": "string" },
                    "new_text": { "type": "string" }
                }
            }),
        },
        ToolSpec {
            name: "tool_exec",
            description: "Run a shell command line (collected unary result).",
            schema: json!({
                "type": "object",
                "required": ["command"],
                "properties": { "command": { "type": "string" } }
            }),
        },
        ToolSpec {
            name: "tool_list_dir",
            description: "List one directory level.",
            schema: json!({
                "type": "object",
                "required": ["path"],
                "properties": { "path": { "type": "string" } }
            }),
        },
        ToolSpec {
            name: "tool_grep",
            description: "Regex content search under a root.",
            schema: json!({
                "type": "object",
                "required": ["path", "pattern"],
                "properties": {
                    "path": { "type": "string" },
                    "pattern": { "type": "string" }
                }
            }),
        },
        ToolSpec {
            name: "tool_find",
            description: "Filename-glob search under a root.",
            schema: json!({
                "type": "object",
                "required": ["path", "pattern"],
                "properties": {
                    "path": { "type": "string" },
                    "pattern": { "type": "string" }
                }
            }),
        },
        ToolSpec {
            name: "tool_memory_save",
            description: "Save a cross-session memory entry.",
            schema: json!({
                "type": "object",
                "required": ["name", "content"],
                "properties": {
                    "name": { "type": "string" },
                    "content": { "type": "string" }
                }
            }),
        },
        ToolSpec {
            name: "tool_memory_list",
            description: "List memory entries.",
            schema: json!({ "type": "object", "properties": {} }),
        },
        ToolSpec {
            name: "tool_memory_read",
            description: "Read one memory entry.",
            schema: json!({
                "type": "object",
                "required": ["name"],
                "properties": { "name": { "type": "string" } }
            }),
        },
        ToolSpec {
            name: "tool_memory_forget",
            description: "Delete one memory entry.",
            schema: json!({
                "type": "object",
                "required": ["name"],
                "properties": { "name": { "type": "string" } }
            }),
        },
        ToolSpec {
            name: "tool_skill_install",
            description: "Two-phase skill install (preview unless confirm).",
            schema: json!({
                "type": "object",
                "required": ["source"],
                "properties": {
                    "source": { "type": "string" },
                    "confirm": { "type": "boolean" }
                }
            }),
        },
        ToolSpec {
            name: "settings_get_config",
            description: "Current daemon configuration view.",
            schema: json!({ "type": "object", "properties": {} }),
        },
        ToolSpec {
            name: "settings_set_config",
            description: "Queue a partial daemon configuration update.",
            schema: json!({
                "type": "object",
                "properties": {
                    "provider": { "type": "string" },
                    "model": { "type": "string" },
                    "base_url": { "type": "string" },
                    "thinking": { "type": "boolean" },
                    "thinking_level": { "type": "string" },
                    "builtin_skills": { "type": "array", "items": { "type": "string" } },
                    "skills_dirs": { "type": "array", "items": { "type": "string" } },
                    "trigger_poll_secs": { "type": "integer" },
                    "tui_max_feed_lines": { "type": "integer" },
                    "tool_service_addr": { "type": "string" },
                    "storage_service_addr": { "type": "string" }
                }
            }),
        },
        ToolSpec {
            name: "settings_get_path_context",
            description: "Daemon path context (home/base/work dir/skill dirs).",
            schema: json!({ "type": "object", "properties": {} }),
        },
        ToolSpec {
            name: "settings_set_skill_dirs",
            description: "Replace extra skill directories and hot-reload.",
            schema: json!({
                "type": "object",
                "required": ["dirs"],
                "properties": {
                    "dirs": { "type": "array", "items": { "type": "string" } }
                }
            }),
        },
        ToolSpec {
            name: "storage_save_dag_run",
            description: "Persist one DAG run snapshot.",
            schema: json!({
                "type": "object",
                "required": ["session_id", "run_id", "snapshot"],
                "properties": {
                    "session_id": { "type": "string" },
                    "run_id": { "type": "string" },
                    "snapshot": { "type": "string" }
                }
            }),
        },
        ToolSpec {
            name: "storage_load_dag_runs",
            description: "Load stored DAG runs for a session.",
            schema: json!({
                "type": "object",
                "required": ["session_id"],
                "properties": {
                    "session_id": { "type": "string" },
                    "run_id": { "type": "string" }
                }
            }),
        },
    ]
}

/// `ServerHandler` backed by the shared external service.
struct ToolDispatcher {
    ops: Arc<dyn ExternalProtocolOps>,
    job_ops: Arc<dyn JobOps>,
}

impl ToolDispatcher {
    fn mcp_tool(spec: &ToolSpec) -> Tool {
        let schema: Arc<rmcp::model::JsonObject> =
            Arc::new(serde_json::from_value(spec.schema.clone()).unwrap_or_default());
        Tool::new(spec.name.to_string(), spec.description.to_string(), schema)
    }

    async fn execute(&self, name: &str, args: Value) -> Result<Value, String> {
        match name {
            "session_list" => {
                let sessions = SessionOps::list(self.ops.as_ref())
                    .await
                    .map_err(|e| e.to_string())?;
                serde_json::to_value(sessions).map_err(|e| e.to_string())
            }
            "session_create" => {
                let session_id = args.get("session_id").and_then(Value::as_str);
                let name = args.get("name").and_then(Value::as_str).unwrap_or_default();
                let new_id = self
                    .ops
                    .create(session_id, &Default::default())
                    .await
                    .map_err(|e| e.to_string())?;
                if !name.trim().is_empty() {
                    self.ops
                        .rename(&new_id, name)
                        .await
                        .map_err(|e| e.to_string())?;
                }
                Ok(json!({ "session_id": new_id }))
            }
            "session_rename" => {
                let id = arg_str(&args, "id")?;
                let name = arg_str(&args, "name")?;
                self.ops.rename(id, name).await.map_err(|e| e.to_string())?;
                Ok(json!({ "accepted": true }))
            }
            "session_delete" => {
                let id = arg_str(&args, "id")?;
                let running = self.ops.delete(id).await.map_err(|e| e.to_string())?;
                Ok(json!({ "running_run_ids": running }))
            }
            "session_get_snapshot" => {
                let session_id = args
                    .get("session_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let snapshot = self
                    .ops
                    .authoritative_snapshot(session_id)
                    .await
                    .map_err(|e| e.to_string())?;
                serde_json::to_value(snapshot).map_err(|e| e.to_string())
            }
            "session_list_messages" => {
                let session_id = arg_str(&args, "session_id")?;
                let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(50);
                let before_entry_id = args
                    .get("before_entry_id")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let page = self
                    .ops
                    .list_session_messages(&ListSessionMessagesRequest {
                        session_id: session_id.to_string(),
                        before_entry_id,
                        limit: limit.min(u32::MAX as u64) as u32,
                    })
                    .await
                    .map_err(|e| e.to_string())?;
                serde_json::to_value(page).map_err(|e| e.to_string())
            }
            "graph_list" => {
                let runs = GraphOps::list(self.ops.as_ref(), arg_str(&args, "session_id")?);
                serde_json::to_value(runs).map_err(|e| e.to_string())
            }
            "graph_cancel" => {
                let run_id = arg_str(&args, "run_id")?;
                GraphOps::cancel_run(self.ops.as_ref(), run_id, Some("cancelled via mcp"));
                Ok(json!({ "accepted": true }))
            }
            "graph_clear" => {
                let session_id = args
                    .get("session_id")
                    .and_then(Value::as_str)
                    .map(|s| s.to_string());
                let keep = args.get("keep").and_then(Value::as_u64).unwrap_or(0) as usize;
                let removed =
                    GraphOps::clear_session_runs(self.ops.as_ref(), session_id.as_deref(), keep);
                Ok(json!({ "removed": removed }))
            }
            "graph_retry" => {
                let run_id = arg_str(&args, "run_id")?;
                let node_ids = args
                    .get("node_id")
                    .and_then(Value::as_str)
                    .map(|id| vec![id.to_string()]);
                let reset = GraphOps::retry(self.ops.as_ref(), run_id, node_ids.as_deref());
                Ok(json!({ "reset_node_ids": reset }))
            }
            "graph_skip" => {
                let run_id = arg_str(&args, "run_id")?;
                let node_id = arg_str(&args, "node_id")?;
                let skipped = GraphOps::skip(self.ops.as_ref(), run_id, node_id);
                Ok(json!({ "skipped": skipped }))
            }
            "get_node_output" => {
                let run_id = arg_str(&args, "run_id")?;
                let node_id = arg_str(&args, "node_id")?;
                let offset = args.get("offset").and_then(Value::as_u64).unwrap_or(0);
                let output = self.job_ops.node_output(run_id, node_id);
                Ok(json!({
                    "output": output.output,
                    "offset": offset,
                    "truncated": output.truncated,
                    "messages": output.messages,
                    "messages_truncated": output.messages_truncated,
                }))
            }
            "tool_read" => {
                let request = theway_transport::wire::WireToolReadRequest {
                    path: arg_str(&args, "path")?.to_string(),
                    offset: args.get("offset").and_then(Value::as_u64),
                    limit: args.get("limit").and_then(Value::as_u64),
                };
                tool_json(self.ops.read_file(&request).await)
            }
            "tool_write" => {
                let request = theway_transport::wire::WireToolWriteRequest {
                    path: arg_str(&args, "path")?.to_string(),
                    content: arg_str(&args, "content")?.to_string(),
                };
                tool_json(self.ops.write_file(&request).await)
            }
            "tool_edit" => {
                let request = theway_transport::wire::WireToolEditRequest {
                    path: arg_str(&args, "path")?.to_string(),
                    old_string: arg_str(&args, "old_text")?.to_string(),
                    new_string: arg_str(&args, "new_text")?.to_string(),
                    replace_all: false,
                    range_start: None,
                    range_end: None,
                };
                tool_json(self.ops.edit_file(&request).await)
            }
            "tool_exec" => {
                let request = theway_transport::wire::WireToolExecRequest {
                    command: arg_str(&args, "command")?.to_string(),
                    cwd: None,
                    timeout_ms: None,
                };
                let stream = self
                    .ops
                    .exec_command(&request)
                    .await
                    .map_err(|e| e.to_string())?;
                let result = theway_transport::tools::collect_exec_stream(stream).await;
                serde_json::to_value(result).map_err(|e| e.to_string())
            }
            "tool_list_dir" => {
                let request = theway_transport::wire::WireToolListDirRequest {
                    path: arg_str(&args, "path")?.to_string(),
                    limit: None,
                };
                tool_json(self.ops.list_dir(&request).await)
            }
            "tool_grep" => {
                let request = theway_transport::wire::WireToolGrepRequest {
                    path: Some(arg_str(&args, "path")?.to_string()),
                    pattern: arg_str(&args, "pattern")?.to_string(),
                    glob_filter: None,
                    case_insensitive: false,
                    output_mode: None,
                    max_results: None,
                };
                tool_json(self.ops.grep(&request).await)
            }
            "tool_find" => {
                let request = theway_transport::wire::WireToolFindRequest {
                    path: Some(arg_str(&args, "path")?.to_string()),
                    pattern: arg_str(&args, "pattern")?.to_string(),
                    limit: None,
                };
                tool_json(self.ops.find(&request).await)
            }
            "tool_memory_save" => {
                let request = theway_transport::wire::WireToolMemorySaveRequest {
                    name: arg_str(&args, "name")?.to_string(),
                    content: arg_str(&args, "content")?.to_string(),
                    description: None,
                    memory_type: None,
                };
                tool_json(self.ops.memory_save(&request).await)
            }
            "tool_memory_list" => {
                let request = theway_transport::wire::WireToolMemoryListRequest {};
                tool_json(self.ops.memory_list(&request).await)
            }
            "tool_memory_read" => {
                let request = theway_transport::wire::WireToolMemoryReadRequest {
                    name: arg_str(&args, "name")?.to_string(),
                };
                tool_json(self.ops.memory_read(&request).await)
            }
            "tool_memory_forget" => {
                let request = theway_transport::wire::WireToolMemoryForgetRequest {
                    name: arg_str(&args, "name")?.to_string(),
                };
                tool_json(self.ops.memory_forget(&request).await)
            }
            "tool_skill_install" => {
                let request = theway_transport::wire::WireToolSkillInstallRequest {
                    source: theway_transport::wire::WireToolSkillSource::Path(
                        arg_str(&args, "source")?.to_string(),
                    ),
                    confirm: args
                        .get("confirm")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    overwrite: false,
                };
                tool_json(self.ops.skill_install(&request).await)
            }
            "settings_get_config" => {
                let config = self.ops.get_config().await.map_err(|e| e.to_string())?;
                serde_json::to_value(config).map_err(|e| e.to_string())
            }
            "settings_set_config" => {
                let config = serde_json::from_value(args).map_err(|e| e.to_string())?;
                let accepted = self
                    .ops
                    .set_config(&config)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(json!({ "accepted": accepted }))
            }
            "settings_get_path_context" => {
                let ctx = self
                    .ops
                    .get_path_context()
                    .await
                    .map_err(|e| e.to_string())?;
                serde_json::to_value(ctx).map_err(|e| e.to_string())
            }
            "settings_set_skill_dirs" => {
                let dirs = args
                    .get("dirs")
                    .and_then(Value::as_array)
                    .ok_or_else(|| "missing param `dirs`".to_string())?
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect::<Vec<_>>();
                let accepted = self
                    .ops
                    .set_skill_dirs(&dirs)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(json!({ "accepted": accepted }))
            }
            "storage_save_dag_run" => {
                let request = theway_transport::wire::WireSaveDagRunRequest {
                    session_id: arg_str(&args, "session_id")?.to_string(),
                    run_id: arg_str(&args, "run_id")?.to_string(),
                    snapshot: arg_str(&args, "snapshot")?.to_string(),
                };
                let result = self
                    .ops
                    .save_dag_run(&request)
                    .await
                    .map_err(|e| e.to_string())?;
                serde_json::to_value(result).map_err(|e| e.to_string())
            }
            "storage_load_dag_runs" => {
                let request = theway_transport::wire::WireLoadDagRunsRequest {
                    session_id: arg_str(&args, "session_id")?.to_string(),
                    run_id: args
                        .get("run_id")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                };
                let result = self
                    .ops
                    .load_dag_runs(&request)
                    .await
                    .map_err(|e| e.to_string())?;
                serde_json::to_value(result).map_err(|e| e.to_string())
            }
            _ => Err(format!("tool not found: {name}")),
        }
    }
}

fn arg_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing param `{key}`"))
}

fn tool_json<T: serde::Serialize, E: std::fmt::Display>(
    result: Result<T, E>,
) -> Result<Value, String> {
    result
        .map_err(|e| e.to_string())
        .and_then(|value| serde_json::to_value(value).map_err(|e| e.to_string()))
}

impl ServerHandler for ToolDispatcher {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::default()
            .with_server_info(Implementation::new("theway", env!("CARGO_PKG_VERSION")))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult {
            tools: tool_specs().iter().map(Self::mcp_tool).collect(),
            next_cursor: None,
            ..Default::default()
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let name = request.name;
        let arguments = request
            .arguments
            .map(|arguments| serde_json::to_value(arguments).unwrap_or_default())
            .unwrap_or_else(|| Value::Object(Default::default()));
        if !tool_specs().iter().any(|spec| spec.name == name) {
            return Err(McpError::new(
                ErrorCode::METHOD_NOT_FOUND,
                format!("tool not found: {name}"),
                None,
            ));
        }
        match self.execute(&name, arguments).await {
            Ok(result) => Ok(CallToolResponse::Complete(CallToolResult::success(vec![
                ContentBlock::Text(TextContent::new(result.to_string())),
            ]))),
            Err(message) => Ok(CallToolResponse::Complete(CallToolResult::error(vec![
                ContentBlock::Text(TextContent::new(message)),
            ]))),
        }
    }
}

#[cfg(test)]
tests_bridge_macro::tests_bridge!("mcp_server");
