//! `WireStatus` ↔ `SessionSnapshot` conversion (transport layer).
//!
//! `WireStatus` (serde, `crate::wire`) is the internal live projection shared
//! by the daemon event loop; `SessionSnapshot` (prost, generated from the
//! domain proto files) is the single-version structured wire model for gRPC,
//! StreamEvents snapshot frames, and the JSON surface. The gRPC server
//! serializes `SessionSnapshot` as binary protobuf; JSON channels serialize
//! the serde twin `WireSessionSnapshot`. The tool-operation (`tools.proto`)
//! codecs live in [`crate::tools`]; the runtime-state (`state.proto`) codecs
//! live in [`crate::state`].

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
