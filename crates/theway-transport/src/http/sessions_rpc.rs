use super::*;

pub(super) fn handles(method: &str) -> bool {
    matches!(
        method,
        "list_sessions"
            | "session.list"
            | "state.list_sessions"
            | "storage.list_sessions"
            | "create_session"
            | "session.create"
            | "state.create_session"
            | "storage.create_session"
            | "rename_session"
            | "session.rename"
            | "state.rename_session"
            | "storage.rename_session"
            | "update_session_metadata"
            | "session.update_metadata"
            | "state.update_metadata"
            | "storage.update_metadata"
            | "delete_session"
            | "session.delete"
            | "state.delete_session"
            | "storage.delete_session"
            | "get_path_context"
            | "session.get_path_context"
            | "get_config"
            | "settings.get_config"
            | "set_config"
            | "settings.set_config"
            | "configure"
            | "settings.configure"
            | "set_skill_dirs"
            | "session.set_skill_dirs"
    )
}

pub(super) async fn dispatch(
    state: &HttpState,
    method: &str,
    params: Option<&serde_json::Value>,
) -> RpcResult {
    match method {
        "list_sessions" | "session.list" | "state.list_sessions" | "storage.list_sessions" => {
            let current_session_id = state.latest.lock().session_id.clone();
            let sessions = state
                .session_ops
                .list()
                .await
                .map_err(|e| (-32000, e.to_string()))?;
            Ok(
                serde_json::json!({ "sessions": sessions, "current_session_id": current_session_id }),
            )
        }
        "create_session" | "session.create" | "state.create_session" | "storage.create_session" => {
            let name = params
                .and_then(|p| p.get("name"))
                .and_then(|v| v.as_str())
                .map(String::from);
            let session_id = params
                .and_then(|p| p.get("session_id"))
                .and_then(|v| v.as_str())
                .map(String::from);
            let metadata = params
                .and_then(|p| p.get("metadata"))
                .and_then(|v| {
                    serde_json::from_value::<std::collections::HashMap<String, String>>(v.clone())
                        .ok()
                })
                .unwrap_or_default();
            let new_id = state
                .session_ops
                .create(session_id.as_deref(), &metadata)
                .await
                .map_err(|e| {
                    if e.to_string().contains("already exists") {
                        (-32005, e.to_string())
                    } else {
                        (-32000, e.to_string())
                    }
                })?;
            if let Some(name) = name.as_deref()
                && !name.trim().is_empty()
                && let Err(e) = state.session_ops.rename(&new_id, name).await
            {
                return Err((-32602, e.to_string()));
            }
            let summary = state
                .session_ops
                .list()
                .await
                .map_err(|e| (-32000, e.to_string()))?
                .into_iter()
                .find(|s| s.session_id == new_id)
                .map(|s| serde_json::json!({ "session_id": s.session_id, "name": s.name, "metadata": s.metadata }))
                .unwrap_or_else(|| serde_json::json!({ "session_id": new_id, "metadata": metadata }));
            Ok(summary)
        }
        "rename_session" | "session.rename" | "state.rename_session" | "storage.rename_session" => {
            let id = param(params, "id")?
                .as_str()
                .unwrap_or_default()
                .to_string();
            let name = param(params, "name")?
                .as_str()
                .unwrap_or_default()
                .to_string();
            let sessions = state
                .session_ops
                .list()
                .await
                .map_err(|e| (-32000, e.to_string()))?;
            let target = crate::proto::resolve_session_id(&sessions, &id)
                .ok_or_else(|| (-32004, format!("no session matches id {id}")))?;
            state
                .session_ops
                .rename(&target, &name)
                .await
                .map_err(|e| (-32602, e.to_string()))?;
            Ok(serde_json::json!({ "accepted": true }))
        }
        "update_session_metadata"
        | "session.update_metadata"
        | "state.update_metadata"
        | "storage.update_metadata" => {
            let id = param(params, "id")?
                .as_str()
                .unwrap_or_default()
                .to_string();
            let metadata = params
                .and_then(|p| p.get("metadata"))
                .and_then(|v| {
                    serde_json::from_value::<std::collections::HashMap<String, String>>(v.clone())
                        .ok()
                })
                .unwrap_or_default();
            state
                .session_ops
                .update_metadata(&id, &metadata)
                .await
                .map_err(|e| {
                    if e.to_string().contains("no session matches") {
                        (-32004, e.to_string())
                    } else {
                        (-32602, e.to_string())
                    }
                })?;
            Ok(serde_json::json!({ "accepted": true }))
        }
        "delete_session" | "session.delete" | "state.delete_session" | "storage.delete_session" => {
            let id = param(params, "id")?
                .as_str()
                .unwrap_or_default()
                .to_string();
            let sessions = state
                .session_ops
                .list()
                .await
                .map_err(|e| (-32000, e.to_string()))?;
            let target = crate::proto::resolve_session_id(&sessions, &id)
                .ok_or_else(|| (-32004, format!("no session matches id {id}")))?;
            let running = state
                .session_ops
                .delete(&target)
                .await
                .map_err(|e| (-32000, e.to_string()))?;
            if !running.is_empty() {
                return Err((
                    -32009,
                    format!(
                        "session {target} still has running graphs: {}; cancel them before deleting",
                        running.join(", ")
                    ),
                ));
            }
            let was_current = state.latest.lock().session_id == target;
            if was_current {
                let remaining = state.session_ops.list().await.unwrap_or_default();
                let fallback = remaining
                    .last()
                    .map(|s| s.session_id.clone())
                    .unwrap_or_default();
                state.latest.lock().session_id = fallback.clone();
            }
            // The event loop drops the deleted session's runtime (and swaps
            // the active runtime when the deleted session was current).
            let _ = state
                .commands
                .send(WireCommand::SessionDeleted { id: target });
            Ok(serde_json::json!({ "deleted": true }))
        }
        "get_path_context" | "session.get_path_context" => {
            let ctx = state.path_context.read().unwrap();
            Ok(serde_json::to_value(&*ctx).unwrap_or_default())
        }
        "get_config" | "settings.get_config" => {
            let config = state.daemon_config.read().unwrap();
            Ok(serde_json::to_value(&*config).unwrap_or_default())
        }
        "set_config" | "settings.set_config" | "configure" | "settings.configure" => {
            let config = parse_daemon_config(params)?;
            let accepted = state
                .commands
                .send(WireCommand::Configure { config })
                .is_ok();
            Ok(serde_json::json!({ "accepted": accepted }))
        }
        "set_skill_dirs" | "session.set_skill_dirs" => {
            let dirs = params
                .and_then(|p| p.get("dirs"))
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            state.path_context.write().unwrap().skills_dirs = dirs.clone();
            let accepted = state
                .commands
                .send(WireCommand::SetSkillDirs { dirs })
                .is_ok();
            Ok(serde_json::json!({ "accepted": accepted }))
        }
        _ => Err((-32601, format!("method not found: {method}"))),
    }
}
