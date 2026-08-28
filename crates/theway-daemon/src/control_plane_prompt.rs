use std::sync::Arc;

use theway_core::{
    ControlPlanePromptDecision, ControlPlanePromptRequest, OnControlPlanePromptHook,
};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

pub struct PendingControlPlanePrompt {
    pub session_id: String,
    pub request: ControlPlanePromptRequest,
    pub responder: oneshot::Sender<ControlPlanePromptDecision>,
}

impl PendingControlPlanePrompt {
    pub fn resolve(self, decision: ControlPlanePromptDecision) {
        let _ = self.responder.send(decision);
    }
}

#[allow(dead_code)]
pub fn interactive_hook() -> (
    OnControlPlanePromptHook,
    mpsc::UnboundedReceiver<PendingControlPlanePrompt>,
) {
    let (tx, rx) = mpsc::unbounded_channel::<PendingControlPlanePrompt>();
    let hook = interactive_hook_for_session(String::new(), tx);
    (hook, rx)
}

/// Build a control-plane hook that tags every prompt with `session_id`.
pub fn interactive_hook_for_session(
    session_id: String,
    tx: mpsc::UnboundedSender<PendingControlPlanePrompt>,
) -> OnControlPlanePromptHook {
    Arc::new(move |request, cancel| {
        let tx = tx.clone();
        let session_id = session_id.clone();
        Box::pin(async move {
            let (decision_tx, decision_rx) = oneshot::channel();
            if tx
                .send(PendingControlPlanePrompt {
                    session_id,
                    request,
                    responder: decision_tx,
                })
                .is_err()
            {
                return ControlPlanePromptDecision::Deny {
                    reason: Some("control-plane prompt client is unavailable".into()),
                };
            }
            tokio::select! {
                decision = decision_rx => decision.unwrap_or(ControlPlanePromptDecision::Deny {
                    reason: Some("control-plane prompt client closed before a decision".into()),
                }),
                _ = cancel.cancelled() => ControlPlanePromptDecision::Deny {
                    reason: Some("control-plane prompt cancelled".into()),
                },
            }
        })
    })
}

#[cfg(test)]
pub fn deny_hook(reason: &'static str) -> OnControlPlanePromptHook {
    Arc::new(
        move |_request: ControlPlanePromptRequest, _cancel: CancellationToken| {
            Box::pin(async move {
                ControlPlanePromptDecision::Deny {
                    reason: Some(reason.to_string()),
                }
            })
        },
    )
}

pub fn allow_hook() -> OnControlPlanePromptHook {
    Arc::new(
        move |_request: ControlPlanePromptRequest, _cancel: CancellationToken| {
            Box::pin(async move { ControlPlanePromptDecision::Allow })
        },
    )
}

#[cfg(test)]
tests_bridge_macro::tests_bridge!("control_plane_prompt");
