//! Host abstraction: the server-side app that transports drive.

use async_trait::async_trait;

use crate::transport::{TransportEndpoints, TransportMode};

/// The server-side app a transport drives: hands out its channel surface and
/// runs the serialized event loop. Implemented by the daemon kernel's `TurnHost`
/// (`theway-daemon`).
#[async_trait(?Send)]
pub trait TransportHost: Send {
    fn transport_endpoints(&mut self) -> TransportEndpoints;
    async fn run_transport_loop(
        self: Box<Self>,
        mode: TransportMode,
        endpoints: TransportEndpoints,
        server_task: tokio::task::JoinHandle<anyhow::Result<()>>,
    ) -> anyhow::Result<()>;
}
