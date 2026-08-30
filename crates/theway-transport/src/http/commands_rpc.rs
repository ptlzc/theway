use super::*;

pub(super) fn handles(method: &str) -> bool {
    matches!(
        method,
        "ping"
            | "get_node_output"
            | "graph.get_node_output"
            | "send_message"
            | "command.send_message"
            | "set_model"
            | "command.set_model"
            | "set_thinking"
            | "command.set_thinking"
            | "complete"
            | "abort"
            | "command.cancel"
            | "trigger_immediate"
            | "control_plane_resolve"
            | "command.approve"
    )
}

pub(super) async fn dispatch(
    state: &HttpState,
    method: &str,
    params: Option<&serde_json::Value>,
) -> RpcResult {
    match method {
        "ping" => Ok(serde_json::Value::Null),
        "get_node_output" | "graph.get_node_output" => {
            let run_id = param(params, "run_id")?
                .as_str()
                .unwrap_or_default()
                .to_string();
            let node_id = param(params, "node_id")?
                .as_str()
                .unwrap_or_default()
                .to_string();
            let offset = params
                .and_then(|p| p.get("offset"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let node_output = state.job_ops.node_output(&run_id, &node_id);
            let messages = node_output
                .messages
                .map(|m| serde_json::to_value(m).unwrap_or_default())
                .unwrap_or(serde_json::Value::Null);
            let messages_truncated = node_output.messages_truncated;
            match node_output.output {
                Some(output) => {
                    let (offset, text) = crate::text_cursor::slice_from(&output, offset);
                    Ok(serde_json::json!({
                        "text": text,
                        "offset": offset,
                        "total": output.len(),
                        "truncated": node_output.truncated,
                        "messages": messages,
                        "messages_truncated": messages_truncated,
                    }))
                }
                None => Ok(serde_json::json!({
                    "text": "",
                    "offset": offset,
                    "total": 0,
                    "truncated": false,
                    "messages": messages,
                    "messages_truncated": messages_truncated,
                })),
            }
        }
        "send_message" | "command.send_message" => {
            let text = param(params, "text")?
                .as_str()
                .unwrap_or_default()
                .to_string();
            let images = params
                .and_then(|p| p.get("images"))
                .and_then(|v| serde_json::from_value::<Vec<WirePromptImage>>(v.clone()).ok())
                .unwrap_or_default();
            let current = state.latest.lock().session_id.clone();
            let session_id = params
                .and_then(|p| p.get("session_id"))
                .and_then(|v| v.as_str())
                .filter(|id| !id.is_empty())
                .map(String::from)
                .unwrap_or_else(|| current.clone());
            let accepted = state
                .external_ops
                .submit(&session_id, &text, images, false)
                .await
                .unwrap_or(false);
            Ok(serde_json::json!({ "accepted": accepted }))
        }
        "set_model" | "command.set_model" => {
            let spec = param(params, "model")?
                .as_str()
                .unwrap_or_default()
                .to_string();
            let session_id = params
                .and_then(|p| p.get("session_id"))
                .and_then(|v| v.as_str())
                .filter(|id| !id.is_empty())
                .map(String::from)
                .unwrap_or_else(|| state.latest.lock().session_id.clone());
            let accepted = state
                .external_ops
                .set_model(&session_id, &spec)
                .await
                .unwrap_or(false);
            Ok(serde_json::json!({ "accepted": accepted }))
        }
        "set_thinking" | "command.set_thinking" => {
            let level = param(params, "level")?
                .as_str()
                .unwrap_or_default()
                .to_string();
            let session_id = params
                .and_then(|p| p.get("session_id"))
                .and_then(|v| v.as_str())
                .filter(|id| !id.is_empty())
                .map(String::from)
                .unwrap_or_else(|| state.latest.lock().session_id.clone());
            let accepted = state
                .external_ops
                .set_thinking(&session_id, &level)
                .await
                .unwrap_or(false);
            Ok(serde_json::json!({ "accepted": accepted }))
        }
        "complete" => {
            let text = param(params, "text")?
                .as_str()
                .unwrap_or_default()
                .to_string();
            Ok(serde_json::json!({ "completions": state.completer.matches(&text) }))
        }
        "abort" | "command.cancel" => {
            let session_id = params
                .and_then(|p| p.get("session_id"))
                .and_then(|v| v.as_str())
                .filter(|id| !id.is_empty())
                .map(String::from)
                .unwrap_or_else(|| state.latest.lock().session_id.clone());
            let accepted = state.external_ops.abort(&session_id).await.unwrap_or(false);
            Ok(serde_json::json!({ "accepted": accepted }))
        }
        "trigger_immediate" => {
            let id = param(params, "id")?
                .as_str()
                .unwrap_or_default()
                .to_string();
            let accepted = state.external_ops.trigger_now(&id).await.unwrap_or(false);
            Ok(serde_json::json!({ "accepted": accepted }))
        }
        "control_plane_resolve" | "command.approve" => {
            let approve = param(params, "approve")?.as_bool().unwrap_or(false);
            let session_id = params
                .and_then(|p| p.get("session_id"))
                .and_then(|v| v.as_str())
                .filter(|id| !id.is_empty())
                .map(String::from)
                .unwrap_or_else(|| state.latest.lock().session_id.clone());
            let accepted = state
                .external_ops
                .resolve_control_plane(&session_id, approve)
                .await
                .unwrap_or(false);
            Ok(serde_json::json!({ "accepted": accepted }))
        }
        _ => Err((-32601, format!("method not found: {method}"))),
    }
}
