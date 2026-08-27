//! `thewayd` — headless agent runtime over gRPC, HTTP, or MCP.

use anyhow::Result;
use clap::Parser;
use theway_daemon::{DaemonOptions, DaemonTransport, SessionSelection, run};

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/bin/thewayd/cli.rs"
));

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let transport = if cli.mcp {
        DaemonTransport::Mcp
    } else if cli.http {
        DaemonTransport::Http
    } else {
        DaemonTransport::Grpc
    };
    let paths = theway_daemon::DaemonPaths::from_cli_with_base(
        cli.cwd,
        cli.home,
        cli.skills_dir,
        cli.theway_dir,
    );
    let thinking = cli.thinking.parse().map_err(anyhow::Error::msg)?;
    let session = match (cli.resume_id, cli.continue_) {
        (Some(id), _) => SessionSelection::Id(id),
        (None, true) => SessionSelection::Latest,
        (None, false) => SessionSelection::New,
    };

    run(DaemonOptions {
        paths,
        transport,
        host: cli.host,
        port: cli.port,
        provider: cli.provider,
        model: cli.model,
        base_url: cli.base_url,
        thinking,
        session,
        approve_control_plane: cli.always_allow || cli.yes,
        debug: cli.debug,
        trigger_poll_secs: cli.trigger_poll_secs,
        builtin_skills: cli.builtin_skill,
        storage_service_addr: cli.storage_service_addr,
    })
    .await
}
