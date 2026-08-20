use tonic::{Request, Response, Status};

use super::GrpcState;
use crate::proto::theway_grpc::extension_service_server::ExtensionService;
use crate::proto::theway_grpc::{
    DecideExtensionTrustRequest, DecideExtensionTrustResponse, Empty, ExtensionCommandOutcome,
    ExtensionSnapshot, InvokeExtensionCommandRequest, ReloadExtensionsRequest,
    ReloadExtensionsResponse,
};
use crate::wire::WireCommand;

#[tonic::async_trait]
impl ExtensionService for GrpcState {
    async fn get_extensions(
        &self,
        _request: Request<Empty>,
    ) -> Result<Response<ExtensionSnapshot>, Status> {
        let snapshot = self.latest.lock().extensions.clone();
        Ok(Response::new(crate::proto::extension_snapshot_proto(
            &snapshot,
        )))
    }

    async fn invoke_command(
        &self,
        request: Request<InvokeExtensionCommandRequest>,
    ) -> Result<Response<ExtensionCommandOutcome>, Status> {
        let request = request.into_inner();
        let arguments = serde_json::from_str(&request.arguments_json).map_err(|error| {
            Status::invalid_argument(format!("invalid arguments_json: {error}"))
        })?;
        let (response, result) = tokio::sync::oneshot::channel();
        self.commands
            .send(WireCommand::InvokeExtensionCommand {
                name: request.name,
                arguments,
                has_interactive_client: request.has_interactive_client,
                response,
            })
            .map_err(|_| Status::unavailable("event loop command channel closed"))?;
        let outcome = result
            .await
            .map_err(|_| Status::unavailable("extension command response channel closed"))?
            .map_err(Status::failed_precondition)?;
        Ok(Response::new(ExtensionCommandOutcome {
            status: outcome.status,
            code: outcome.code,
            message: outcome.message,
            data_json: outcome.data.map(|value| value.to_string()),
        }))
    }

    async fn reload(
        &self,
        request: Request<ReloadExtensionsRequest>,
    ) -> Result<Response<ReloadExtensionsResponse>, Status> {
        let (response, result) = tokio::sync::oneshot::channel();
        self.commands
            .send(WireCommand::ReloadExtensions {
                cancel_active: request.into_inner().cancel_active,
                response,
            })
            .map_err(|_| Status::unavailable("event loop command channel closed"))?;
        let reload = result
            .await
            .map_err(|_| Status::unavailable("extension reload response channel closed"))?
            .map_err(Status::failed_precondition)?;
        Ok(Response::new(ReloadExtensionsResponse {
            status: reload.status,
            revision: reload.revision,
        }))
    }

    async fn decide_trust(
        &self,
        request: Request<DecideExtensionTrustRequest>,
    ) -> Result<Response<DecideExtensionTrustResponse>, Status> {
        let request = request.into_inner();
        let (response, result) = tokio::sync::oneshot::channel();
        self.commands
            .send(WireCommand::DecideExtensionTrust {
                request: crate::wire::WireExtensionTrustRequest {
                    subject: request.subject,
                    extension_id: request.extension_id,
                    decision: request.decision,
                    granted_permissions: request.granted_permissions,
                },
                response,
            })
            .map_err(|_| Status::unavailable("event loop command channel closed"))?;
        let trust = result
            .await
            .map_err(|_| Status::unavailable("extension trust response channel closed"))?
            .map_err(Status::failed_precondition)?;
        Ok(Response::new(DecideExtensionTrustResponse {
            accepted: trust.accepted,
            reload: Some(ReloadExtensionsResponse {
                status: trust.reload.status,
                revision: trust.reload.revision,
            }),
        }))
    }
}
