//! Wire protocol model shared by the `--http` (axum) and `--grpc` (tonic) transport
//! servers: the command enum both event loops consume and the status payload both
//! serialize. Decoupled from the terminal UI — the servers live in the `transport`
//! module, and the serialized event loop is driven by the daemon kernel through the
//! [`crate::host::TransportHost`] surface. The proto codecs that map these models onto
//! the generated gRPC types live in `transport::proto` as well.

use std::collections::HashMap;

use chrono::{TimeZone, Utc};
use serde::{Deserialize, Serialize};

include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/wire/commands.rs"));
include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/wire/session.rs"));
include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/wire/graph.rs"));
include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/wire/status.rs"));
include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/wire/snapshots.rs"
));
include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/wire/runtime.rs"));
include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/wire/tools.rs"));
include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/wire/extensions.rs"
));
include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/wire/session_api.rs"
));
