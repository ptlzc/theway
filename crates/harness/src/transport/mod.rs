//! Wire transports for the coding agent: the `--web` (axum HTTP/SSE) and `--grpc`
//! (tonic) servers plus the protocol model they share, decoupled from the terminal
//! UI (`crate::ui`). Behavior is identical to the former `ui::web` / `ui::grpc`
//! modules — same endpoints, same wire shapes.

pub mod grpc;
pub mod http;
pub mod proto;
pub mod types;
pub mod ws;
