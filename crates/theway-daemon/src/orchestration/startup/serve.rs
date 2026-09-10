//! Transport serving for the selected mode and the per-cwd discovery entry.

use std::sync::Arc;

use anyhow::Result;

use super::DaemonOptions;
use super::DaemonTransport;
use super::controller_storage::supervise_controller_storage;
use crate::turn::daemon::TurnHost;

/// Serve the protocol surface selected on the command line. gRPC and HTTP
/// share the assembled host; MCP stdio serves the same shared endpoints.
pub(super) async fn serve_transport(
    mode: DaemonTransport,
    options: &DaemonOptions,
    mut host: TurnHost,
    cwd: &std::path::Path,
    port_file: &std::path::Path,
    on_listen: Arc<dyn Fn(std::net::SocketAddr) + Send + Sync>,
) -> Result<()> {
    match mode {
        DaemonTransport::Mcp => {
            // Clear a leftover discovery entry only when its daemon is gone;
            // a live daemon keeps its entry (MCP mode serves no gRPC surface).
            if let Ok(Some(entry)) = theway_transport::client::read_port_file(cwd) {
                if entry
                    .pid
                    .map(|p| !theway_transport::client::pid_alive(p))
                    .unwrap_or(true)
                {
                    let _ = std::fs::remove_file(port_file);
                }
            }
            // Build the same shared service the gRPC/HTTP servers use, then
            // serve it through the MCP stdio protocol.
            let endpoints = host.transport_endpoints();
            supervise_controller_storage(options.storage_service_addr.as_deref(), async {
                crate::mcp_server::run_mcp_server(
                    endpoints.external_ops.clone(),
                    endpoints.job_ops.clone(),
                )
                .await
                .map_err(|e| anyhow::anyhow!("mcp server: {e}"))
            })
            .await
        }
        DaemonTransport::Grpc => {
            supervise_controller_storage(
                options.storage_service_addr.as_deref(),
                theway_transport::grpc::run_grpc(
                    Box::new(host),
                    theway_transport::grpc::GrpcOptions {
                        host: options.host.clone(),
                        port: options.port,
                        on_listen: Some(on_listen.clone()),
                    },
                ),
            )
            .await
        }
        DaemonTransport::Http => {
            supervise_controller_storage(
                options.storage_service_addr.as_deref(),
                theway_transport::http::run_web(
                    Box::new(host),
                    theway_transport::wire::WebOptions {
                        host: options.host.clone(),
                        port: options.port,
                        on_listen: Some(on_listen.clone()),
                    },
                ),
            )
            .await
        }
    }
}

/// Build the bind callback that publishes the bound port and our pid to the
/// per-cwd discovery file.
pub(super) fn port_file_listener(
    port_file: std::path::PathBuf,
    daemon_pid: u32,
) -> Arc<dyn Fn(std::net::SocketAddr) + Send + Sync> {
    Arc::new(move |addr| {
        let entry = format!("{} {}", addr.port(), daemon_pid);
        if let Err(e) = std::fs::write(&port_file, entry) {
            tracing::warn!("write daemon port file {}: {e}", port_file.display());
        }
    })
}
