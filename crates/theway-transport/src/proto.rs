//! `WireStatus` ↔ `SessionState` conversion (transport layer).
//!
//! `WireStatus` (serde, `crate::wire`) is the internal model shared by the
//! `--http` JSON surface and the UI event loop; `SessionState` (prost, generated
//! from the seven domain proto files — `commands.proto`, `session.proto`,
//! `graph_engine.proto`, `events.proto`, `settings.proto`, `tools.proto`,
//! `state.proto` — plus `health.proto` by this crate's build.rs) is the
//! structured wire model for gRPC. The gRPC server serializes `SessionState`
//! as binary protobuf; JSON channels keep using `WireStatus` until the
//! protojson migration (see docs/PROTOCOL.md). The tool-operation
//! (`tools.proto`) codecs live in [`crate::tools`]; the runtime-state
//! (`state.proto`) codecs live in [`crate::state`].

/// Generated protobuf code for package `theway.grpc.v1`, produced by this
/// crate's `build.rs` into its own OUT_DIR. Each domain proto file carries its
/// messages, enums, and service in the same package: `commands.proto` /
/// `session.proto` / `graph_engine.proto` / `events.proto` / `settings.proto`
/// / `tools.proto` / `state.proto`.
pub mod theway_grpc {
    tonic::include_proto!("theway.grpc.v1");
}

/// Generated code for `crates/theway-transport/proto/health.proto` (standard
/// `grpc.health.v1` health checking protocol), produced by this crate's build.rs.
pub mod health {
    tonic::include_proto!("grpc.health.v1");
}

use crate::feed::{self, WireFeedBlock};
use crate::wire::{
    WireAgentEvent, WireCollapseSessionRequest, WireCollapseSessionResponse,
    WireCollapsedSessionNode, WireDagEvent, WireExtensionCatalogEntry,
    WireExtensionCommandDescriptor, WireExtensionContribution, WireExtensionDiagnostic,
    WireExtensionSnapshot, WireSessionGraphNode, WireSessionGraphNodeStreamFrame,
    WireSessionGraphNodeType, WireSessionInfo, WireSessionRuntime, WireSessionSnapshot, WireStatus,
};
use crate::wire::{WireFeedDelta, WirePathContext};
use theway_grpc as wire;

include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/proto/status.rs"));
include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/proto/extensions.rs"
));
include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/proto/session.rs"));
include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/proto/graph.rs"));
include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/proto/sidebar.rs"));
include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/proto/feed.rs"));
include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/proto/dag.rs"));
include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/proto/resources.rs"
));

#[cfg(test)]
// Test files live in `tests/transport/proto/` (mirror of src), pulled in by
// path so they keep unit-test semantics (private access). See docs/rust-test-files.md.
tests_bridge_macro::tests_bridge!("proto");
