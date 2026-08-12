//! Build script: compile proto/health.proto for the probe's gRPC client.
//! Only health.proto is needed; the full theway_grpc.proto is optional for the
//! probe (we test via health check + raw TCP signal tests).

use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proto_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../proto");
    let health_file = proto_dir.join("health.proto");
    let domain_file = proto_dir.join("theway_grpc.proto");
    println!("cargo:rerun-if-changed={}", health_file.display());
    println!("cargo:rerun-if-changed={}", domain_file.display());

    let fds = protox::compile([&health_file, &domain_file], [&proto_dir])?;
    tonic_prost_build::configure().compile_fds(fds)?;
    Ok(())
}
