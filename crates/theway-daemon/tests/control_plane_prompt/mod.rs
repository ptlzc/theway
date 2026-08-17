//! Tests for `control_plane_prompt` — split out of src (see docs/rust-test-files.md).

use super::*;

fn request() -> ControlPlanePromptRequest {
    ControlPlanePromptRequest {
        tool_call_id: "call-1".into(),
        tool_name: "InstallSkill".into(),
        args_hash: "hash-1".into(),
        label: "install a skill".into(),
        payload: serde_json::json!({ "name": "demo" }),
        reason: "escalating write".into(),
    }
}

#[tokio::test]
async fn interactive_hook_resolve_sends_decision_back() {
    // Arrange
    let (hook, mut rx) = interactive_hook();
    let cancel = CancellationToken::new();

    // Act: run the hook and resolve the prompt through the UI channel.
    let hook_task = tokio::spawn({
        let hook = hook.clone();
        let req = request();
        async move { hook(req, cancel).await }
    });
    let prompt = rx.recv().await.expect("prompt reaches the UI");
    assert_eq!(prompt.request.tool_name, "InstallSkill");
    assert_eq!(prompt.request.tool_call_id, "call-1");
    prompt.resolve(ControlPlanePromptDecision::Allow);
    let decision = hook_task.await.unwrap();

    // Assert
    assert!(matches!(decision, ControlPlanePromptDecision::Allow));
}

#[tokio::test]
async fn interactive_hook_denies_when_ui_unavailable() {
    // Arrange
    let (hook, rx) = interactive_hook();
    drop(rx);
    let cancel = CancellationToken::new();

    // Act
    let decision = hook(request(), cancel).await;

    // Assert
    match decision {
        ControlPlanePromptDecision::Deny {
            reason: Some(reason),
        } => {
            assert!(reason.contains("unavailable"), "{reason}");
        }
        other => panic!("expected deny with unavailable reason, got {other:?}"),
    }
}

#[tokio::test]
async fn interactive_hook_denies_when_cancelled() {
    // Arrange
    let (hook, _rx) = interactive_hook();
    let cancel = CancellationToken::new();
    cancel.cancel();

    // Act
    let decision = hook(request(), cancel).await;

    // Assert
    match decision {
        ControlPlanePromptDecision::Deny {
            reason: Some(reason),
        } => {
            assert!(reason.contains("cancelled"), "{reason}");
        }
        other => panic!("expected deny with cancelled reason, got {other:?}"),
    }
}

#[tokio::test]
async fn deny_hook_returns_configured_reason() {
    // Arrange
    let hook = deny_hook("never");

    // Act
    let decision = hook(request(), CancellationToken::new()).await;

    // Assert
    assert!(matches!(
        decision,
        ControlPlanePromptDecision::Deny { reason: Some(reason) } if reason == "never"
    ));
}

#[tokio::test]
async fn allow_hook_returns_allow() {
    // Arrange
    let hook = allow_hook();

    // Act
    let decision = hook(request(), CancellationToken::new()).await;

    // Assert
    assert!(matches!(decision, ControlPlanePromptDecision::Allow));
}
