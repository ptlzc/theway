//! Compiles `proto/theway_grpc.proto` (workspace root, shared with the TS
//! protoc-gen-es side) into Rust via protox (pure-Rust, no system protoc) +
//! tonic-prost-build. The proto uses only a local `Empty` message, so no
//! well-known-type includes are needed.

use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proto_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../proto");
    let proto_file = proto_dir.join("theway_grpc.proto");
    println!("cargo:rerun-if-changed={}", proto_file.display());

    let fds = protox::compile([&proto_file], [&proto_dir])?;
    tonic_prost_build::configure().compile_fds(fds)?;
    Ok(())
}
