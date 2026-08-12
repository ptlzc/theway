//! Compiles `proto/theway_grpc.proto` (workspace root, shared with the TS
//! protoc-gen-es side) and `proto/health.proto` (standard grpc.health.v1)
//! into Rust via protox (pure-Rust, no system protoc) + tonic-prost-build.
//! The protos use only local messages, so no well-known-type includes are
//! needed.

use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proto_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../proto");
    let proto_file = proto_dir.join("theway_grpc.proto");
    let health_file = proto_dir.join("health.proto");
    println!("cargo:rerun-if-changed={}", proto_file.display());
    println!("cargo:rerun-if-changed={}", health_file.display());

    let fds = protox::compile([&proto_file, &health_file], [&proto_dir])?;
    tonic_prost_build::configure().compile_fds(fds)?;
    Ok(())
}
