//! Compiles the split `proto/theway_grpc.proto` service entrypoint (workspace
//! root, shared with the TS ts-proto side) and its domain imports, plus
//! `proto/health.proto` (standard grpc.health.v1), into Rust via protox
//! (pure-Rust, no system protoc) + tonic-prost-build. The protos use only
//! local messages, so no well-known-type includes are needed.

use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proto_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../proto");
    let proto_file = proto_dir.join("theway_grpc.proto");
    let health_file = proto_dir.join("health.proto");

    // Watch every file in the shared proto directory: theway_grpc.proto
    // imports its domain files, but Cargo only reruns on paths listed here.
    for entry in std::fs::read_dir(&proto_dir)? {
        let path = entry?.path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("proto") {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }

    let fds = protox::compile([&proto_file, &health_file], [&proto_dir])?;
    tonic_prost_build::configure().compile_fds(fds)?;
    Ok(())
}
