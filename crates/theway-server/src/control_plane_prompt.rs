use std::sync::Arc;

use theway_core::{
    ControlPlanePromptDecision, ControlPlanePromptRequest, OnControlPlanePromptHook,
};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

pub struct UiControlPlanePrompt {
    pub(crate) request: ControlPlanePromptRequest,
    pub(crate) responder: oneshot::Sender<ControlPlanePromptDecision>,
}

impl UiControlPlanePrompt {
    pub(crate) fn resolve(self, decision: ControlPlanePromptDecision) {
        let _ = self.responder.send(decision);
    }
}

pub fn interactive_hook() -> (
    OnControlPlanePromptHook,
    mpsc::UnboundedReceiver<UiControlPlanePrompt>,
) {
    let (tx, rx) = mpsc::unbounded_channel::<UiControlPlanePrompt>();
    let hook: OnControlPlanePromptHook = Arc::new(move |request, cancel| {
        let tx = tx.clone();
        Box::pin(async move {
            let (decision_tx, decision_rx) = oneshot::channel();
            if tx
                .send(UiControlPlanePrompt {
                    request,
                    responder: decision_tx,
                })
                .is_err()
            {
                return ControlPlanePromptDecision::Deny {
                    reason: Some("control-plane prompt UI is unavailable".into()),
                };
            }
            tokio::select! {
                decision = decision_rx => decision.unwrap_or(ControlPlanePromptDecision::Deny {
                    reason: Some("control-plane prompt UI closed before a decision".into()),
                }),
                _ = cancel.cancelled() => ControlPlanePromptDecision::Deny {
                    reason: Some("control-plane prompt cancelled".into()),
                },
            }
        })
    });
    (hook, rx)
}

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
