use super::*;

pub(super) fn handles(method: &str) -> bool {
    matches!(
        method,
        "read_file"
            | "tool.read_file"
            | "write_file"
            | "tool.write_file"
            | "edit_file"
            | "tool.edit_file"
            | "exec_command"
            | "tool.exec_command"
            | "list_dir"
            | "tool.list_dir"
            | "grep"
            | "tool.grep"
            | "find"
            | "tool.find"
            | "memory_save"
            | "tool.memory_save"
            | "memory_list"
            | "tool.memory_list"
            | "memory_read"
            | "tool.memory_read"
            | "memory_forget"
            | "tool.memory_forget"
            | "skill_install"
            | "tool.skill_install"
    )
}

pub(super) async fn dispatch(
    state: &HttpState,
    method: &str,
    params: Option<&serde_json::Value>,
) -> RpcResult {
    match method {
        // ── tool operations (issue #75) ────────────────────────────────
        "read_file" | "tool.read_file" => {
            let request: WireToolReadRequest = tool_params(params)?;
            let result = state
                .tool_ops
                .read_file(&request)
                .await
                .map_err(crate::tools::tool_rpc_error)?;
            tool_json(&result)
        }
        "write_file" | "tool.write_file" => {
            let request: WireToolWriteRequest = tool_params(params)?;
            let result = state
                .tool_ops
                .write_file(&request)
                .await
                .map_err(crate::tools::tool_rpc_error)?;
            tool_json(&result)
        }
        "edit_file" | "tool.edit_file" => {
            let request: WireToolEditRequest = tool_params(params)?;
            let result = state
                .tool_ops
                .edit_file(&request)
                .await
                .map_err(crate::tools::tool_rpc_error)?;
            tool_json(&result)
        }
        "exec_command" | "tool.exec_command" => {
            let request: WireToolExecRequest = tool_params(params)?;
            let stream = state
                .tool_ops
                .exec_command(&request)
                .await
                .map_err(crate::tools::tool_rpc_error)?;
            // Unary shape: the frame stream is collected into one result
            // (the gRPC ToolService streams the frames individually).
            let result = crate::tools::collect_exec_stream(stream).await;
            tool_json(&result)
        }
        "list_dir" | "tool.list_dir" => {
            let request: WireToolListDirRequest = tool_params(params)?;
            let result = state
                .tool_ops
                .list_dir(&request)
                .await
                .map_err(crate::tools::tool_rpc_error)?;
            tool_json(&result)
        }
        "grep" | "tool.grep" => {
            let request: WireToolGrepRequest = tool_params(params)?;
            let result = state
                .tool_ops
                .grep(&request)
                .await
                .map_err(crate::tools::tool_rpc_error)?;
            tool_json(&result)
        }
        "find" | "tool.find" => {
            let request: WireToolFindRequest = tool_params(params)?;
            let result = state
                .tool_ops
                .find(&request)
                .await
                .map_err(crate::tools::tool_rpc_error)?;
            tool_json(&result)
        }
        "memory_save" | "tool.memory_save" => {
            let request: WireToolMemorySaveRequest = tool_params(params)?;
            let result = state
                .tool_ops
                .memory_save(&request)
                .await
                .map_err(crate::tools::tool_rpc_error)?;
            tool_json(&result)
        }
        "memory_list" | "tool.memory_list" => {
            let request: WireToolMemoryListRequest = tool_params(params)?;
            let result = state
                .tool_ops
                .memory_list(&request)
                .await
                .map_err(crate::tools::tool_rpc_error)?;
            tool_json(&result)
        }
        "memory_read" | "tool.memory_read" => {
            let request: WireToolMemoryReadRequest = tool_params(params)?;
            let result = state
                .tool_ops
                .memory_read(&request)
                .await
                .map_err(crate::tools::tool_rpc_error)?;
            tool_json(&result)
        }
        "memory_forget" | "tool.memory_forget" => {
            let request: WireToolMemoryForgetRequest = tool_params(params)?;
            let result = state
                .tool_ops
                .memory_forget(&request)
                .await
                .map_err(crate::tools::tool_rpc_error)?;
            tool_json(&result)
        }
        "skill_install" | "tool.skill_install" => {
            let request: WireToolSkillInstallRequest = tool_params(params)?;
            let result = state
                .tool_ops
                .skill_install(&request)
                .await
                .map_err(crate::tools::tool_rpc_error)?;
            tool_json(&result)
        }
        _ => Err((-32601, format!("method not found: {method}"))),
    }
}
