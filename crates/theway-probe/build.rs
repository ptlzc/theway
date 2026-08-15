//! Build script: compile `proto/theway_grpc.proto` (service entrypoint plus
//! domain imports) and `proto/health.proto` for the probe's gRPC client.

use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proto_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../proto");
    let health_file = proto_dir.join("health.proto");
    let domain_file = proto_dir.join("theway_grpc.proto");

    for entry in std::fs::read_dir(&proto_dir)? {
        let path = entry?.path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("proto") {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }

    let fds = protox::compile([&health_file, &domain_file], [&proto_dir])?;
    tonic_prost_build::configure().compile_fds(fds)?;
    Ok(())
}
