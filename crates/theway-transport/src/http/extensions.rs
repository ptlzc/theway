use super::{HttpState, RpcResult, param, tool_params};
use crate::wire::{WireCommand, WireExtensionTrustRequest};

pub(super) async fn dispatch(
    state: &HttpState,
    method: &str,
    params: Option<&serde_json::Value>,
) -> Option<RpcResult> {
    let result = match method {
        "extensions.get" => serde_json::to_value(&state.latest.lock().extensions)
            .map_err(|error| (-32000, error.to_string())),
        "extensions.invoke" => invoke(state, params).await,
        "extensions.reload" => reload(state, params).await,
        "extensions.decide_trust" => decide_trust(state, params).await,
        _ => return None,
    };
    Some(result)
}

async fn invoke(state: &HttpState, params: Option<&serde_json::Value>) -> RpcResult {
    let name = param(params, "name")?
        .as_str()
        .unwrap_or_default()
        .to_string();
    let arguments = params
        .and_then(|value| value.get("arguments"))
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    let has_interactive_client = params
        .and_then(|value| value.get("hasInteractiveClient"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let (response, result) = tokio::sync::oneshot::channel();
    state
        .commands
        .send(WireCommand::InvokeExtensionCommand {
            name,
            arguments,
            has_interactive_client,
            response,
        })
        .map_err(|_| (-32003, "event loop command channel closed".into()))?;
    let outcome = result
        .await
        .map_err(|_| (-32003, "extension command response channel closed".into()))?
        .map_err(|error| (-32009, error))?;
    serde_json::to_value(outcome).map_err(|error| (-32000, error.to_string()))
}

async fn reload(state: &HttpState, params: Option<&serde_json::Value>) -> RpcResult {
    let cancel_active = params
        .and_then(|value| value.get("cancelActive"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let (response, result) = tokio::sync::oneshot::channel();
    state
        .commands
        .send(WireCommand::ReloadExtensions {
            cancel_active,
            response,
        })
        .map_err(|_| (-32003, "event loop command channel closed".into()))?;
    let reload = result
        .await
        .map_err(|_| (-32003, "extension reload response channel closed".into()))?
        .map_err(|error| (-32009, error))?;
    serde_json::to_value(reload).map_err(|error| (-32000, error.to_string()))
}

async fn decide_trust(state: &HttpState, params: Option<&serde_json::Value>) -> RpcResult {
    let request: WireExtensionTrustRequest = tool_params(params)?;
    let (response, result) = tokio::sync::oneshot::channel();
    state
        .commands
        .send(WireCommand::DecideExtensionTrust { request, response })
        .map_err(|_| (-32003, "event loop command channel closed".into()))?;
    let trust = result
        .await
        .map_err(|_| (-32003, "extension trust response channel closed".into()))?
        .map_err(|error| (-32009, error))?;
    serde_json::to_value(trust).map_err(|error| (-32000, error.to_string()))
}
