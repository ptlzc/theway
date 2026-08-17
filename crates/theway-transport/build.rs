//! Compiles the four domain proto files (transport crate root, shared with the
//! TS ts-proto side) — `commands.proto`, `session.proto`, `graph_engine.proto`,
//! `events.proto` — plus `health.proto` (standard grpc.health.v1), into Rust
//! via protox (pure-Rust, no system protoc) + tonic-prost-build. The protos
//! use only local messages, so no well-known-type includes are needed.

use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proto_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("proto");
    let commands_file = proto_dir.join("commands.proto");
    let session_file = proto_dir.join("session.proto");
    let graph_engine_file = proto_dir.join("graph_engine.proto");
    let events_file = proto_dir.join("events.proto");
    let health_file = proto_dir.join("health.proto");

    // Watch every file in the shared proto directory: the domain files import
    // each other, but Cargo only reruns on paths listed here.
    for entry in std::fs::read_dir(&proto_dir)? {
        let path = entry?.path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("proto") {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }

    let fds = protox::compile(
        [
            &commands_file,
            &session_file,
            &graph_engine_file,
            &events_file,
            &health_file,
        ],
        [&proto_dir],
    )?;
    tonic_prost_build::configure().compile_fds(fds)?;
    Ok(())
}
