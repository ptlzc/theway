use theway_contract::extension::{
    ExtensionActionKind, ExtensionHookClass, ExtensionLifecycleEvent,
};
use tokio_util::sync::CancellationToken;

use crate::agent::model_request::NormalizedModelRequestDraft;
use crate::agent::runtime_extensions::ValidatedRuntimeExtensionResult;

use super::{HarnessRuntimeExtensions, parse_follow_up};

impl HarnessRuntimeExtensions {
    pub(in crate::agent::assembly) async fn before_model_request(
        &self,
        mut request: NormalizedModelRequestDraft,
        model_max_tokens: u32,
        cancel: CancellationToken,
    ) -> NormalizedModelRequestDraft {
        self.model_context.apply_to_request(&mut request);
        if !self.port.has_request_hook(
            ExtensionLifecycleEvent::BeforeModelRequest,
            ExtensionHookClass::Transform,
        ) {
            return request;
        }

        let Ok(invocation) = self.invocation(
            ExtensionLifecycleEvent::BeforeModelRequest,
            ExtensionHookClass::Transform,
            serde_json::json!({"request": request}),
            cancel.is_cancelled(),
        ) else {
            return request;
        };
        let Ok(ValidatedRuntimeExtensionResult::Transform(result)) =
            self.guarded(self.port.dispatch_request(invocation)).await
        else {
            return request;
        };
        if result.actions().iter().any(|action| {
            !matches!(
                action.kind,
                ExtensionActionKind::ReplaceModelRequest
                    | ExtensionActionKind::EnqueueFollowUp
                    | ExtensionActionKind::EmitDiagnostic
            )
        }) {
            return request;
        }

        let replacement = result
            .actions()
            .iter()
            .find(|action| action.kind == ExtensionActionKind::ReplaceModelRequest)
            .map(|action| {
                action
                    .payload
                    .get("request")
                    .cloned()
                    .ok_or(())
                    .and_then(|value| serde_json::from_value(value).map_err(|_| ()))
            })
            .transpose();
        let Ok(replacement) = replacement else {
            return request;
        };
        let accepted = replacement.unwrap_or_else(|| request.clone());
        if accepted
            .validate_replacement(&request, model_max_tokens)
            .is_err()
        {
            return request;
        }

        let follow_ups = result
            .actions()
            .iter()
            .filter(|action| action.kind == ExtensionActionKind::EnqueueFollowUp)
            .map(|action| parse_follow_up(&action.payload))
            .collect::<Option<Vec<_>>>();
        let Some(follow_ups) = follow_ups else {
            return request;
        };
        if self.enqueue_follow_ups(follow_ups).is_err() {
            return request;
        }
        accepted
    }
}
