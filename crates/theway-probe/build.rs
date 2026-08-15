//! Build script: compile the four domain proto files
//! (`commands.proto`, `session.proto`, `graph_engine.proto`, `events.proto`)
//! plus `proto/health.proto` for the probe's gRPC clients.

use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proto_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../proto");
    let commands_file = proto_dir.join("commands.proto");
    let session_file = proto_dir.join("session.proto");
    let graph_engine_file = proto_dir.join("graph_engine.proto");
    let events_file = proto_dir.join("events.proto");
    let health_file = proto_dir.join("health.proto");

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
