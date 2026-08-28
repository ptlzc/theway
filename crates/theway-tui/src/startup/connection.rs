//! Controller services plus daemon discovery, spawn, and recovery.

use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result};
use theway_storage::sqlite_repo::SqliteSessionRepo;
use theway_transport::client::{
    GrpcClient, discover, probe_storage_service, spawn_daemon, spawn_daemon_detached,
    spawn_daemon_quiet, wait_ready,
};
use theway_transport::grpc::{
    StorageServiceState, ToolServiceState, serve_storage_service, serve_tool_service,
};
use theway_transport::proto::wire_status;
use theway_transport::transport::{SessionOps, StorageOps, ToolOps};
use theway_transport::wire::{WireDaemonConfig, WireStatus};
use tokio::net::TcpListener;

use super::{daemon_launch_args, daemon_runtime_args};
use crate::cli::Cli;
use crate::config_payload::{assemble_config, provision_config};
use crate::controller_storage::{ControllerSessionOps, ControllerStorageOps};
use crate::local_tool_ops::LocalToolOps;

const DISCOVERY_TIMEOUT: Duration = Duration::from_millis(800);
const STORAGE_PROBE_TIMEOUT: Duration = Duration::from_millis(800);
const SPAWN_TIMEOUT: Duration = Duration::from_secs(20);
const SESSION_RESTORE_TIMEOUT: Duration = Duration::from_secs(10);

/// Which daemon spawn variant the connector should use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DaemonSpawnKind {
    Inherit,
    Quiet,
    Detached,
}

fn daemon_spawn_kind(daemon_mode: bool, inherit_stdio: bool) -> DaemonSpawnKind {
    if daemon_mode {
        DaemonSpawnKind::Detached
    } else if inherit_stdio {
        DaemonSpawnKind::Inherit
    } else {
        DaemonSpawnKind::Quiet
    }
}

fn apply_controller_endpoints(
    desired: &mut WireDaemonConfig,
    daemon_mode: bool,
    tool_addr: String,
    storage_addr: String,
) {
    if !daemon_mode {
        desired.tool_service_addr = Some(tool_addr);
        desired.storage_service_addr = Some(storage_addr);
    }
}

/// One fully checked daemon connection. `reused` is true only when an
/// already-running daemon had a live runtime-storage path.
pub(crate) struct DaemonConnection {
    pub(crate) client: GrpcClient,
    pub(crate) status: WireStatus,
    pub(crate) reused: bool,
    pub(crate) notes: Vec<String>,
}

/// Process-local controller resources needed to reconnect or replace a
/// daemon. The service tasks intentionally live for the whole `App` lifetime.
pub(crate) struct DaemonConnector {
    cwd: PathBuf,
    desired_config: WireDaemonConfig,
    initial_args: Vec<String>,
    runtime_args: Vec<String>,
    storage_addr: String,
    children: Vec<Child>,
    daemon_mode: bool,
    tool_server: tokio::task::JoinHandle<anyhow::Result<()>>,
    storage_server: tokio::task::JoinHandle<anyhow::Result<()>>,
}

impl DaemonConnector {
    /// Start controller tool/storage services and establish the initial
    /// daemon connection.
    pub(crate) async fn start(
        cli: &Cli,
        cwd: &Path,
        repo: Arc<SqliteSessionRepo>,
    ) -> Result<(Self, DaemonConnection)> {
        let tool_listener = TcpListener::bind("127.0.0.1:0").await?;
        let tool_addr = tool_listener.local_addr()?.to_string();
        let tool_ops: Arc<dyn ToolOps> = Arc::new(LocalToolOps::new(cwd.to_path_buf()));
        let tool_server = serve_tool_service(tool_listener, ToolServiceState::new(tool_ops));

        let storage_listener = TcpListener::bind("127.0.0.1:0").await?;
        let storage_addr = storage_listener.local_addr()?.to_string();
        let session_ops: Arc<dyn SessionOps> =
            Arc::new(ControllerSessionOps::new(repo.clone(), cwd.to_path_buf()));
        let storage_ops: Arc<dyn StorageOps> = Arc::new(ControllerStorageOps::new(repo));
        let storage_server = serve_storage_service(
            storage_listener,
            StorageServiceState::new(session_ops, storage_ops),
        );

        let (mut desired_config, notes) = assemble_config(cli).await;
        apply_controller_endpoints(
            &mut desired_config,
            cli.daemon,
            tool_addr,
            storage_addr.clone(),
        );

        let mut connector = Self {
            cwd: cwd.to_path_buf(),
            initial_args: daemon_launch_args(cli, &desired_config),
            runtime_args: daemon_runtime_args(cli, &desired_config),
            desired_config,
            storage_addr,
            children: Vec::new(),
            daemon_mode: cli.daemon,
            tool_server,
            storage_server,
        };
        let mut connection = connector
            .connect(connector.initial_args.clone(), true)
            .await?;
        connection.notes.splice(0..0, notes);
        Ok((connector, connection))
    }

    /// Re-establish a complete runtime connection, replacing a daemon whose
    /// protocol endpoint is alive but whose controller storage is not. The
    /// requested session is authoritative after this method returns.
    pub(crate) async fn recover(&mut self, session_id: &str) -> Result<DaemonConnection> {
        self.reap_children();
        let mut args = vec!["--resume-id".to_string(), session_id.to_string()];
        args.extend(self.runtime_args.clone());
        let mut connection = self.connect(args, false).await?;
        connection.status =
            restore_session(&mut connection.client, connection.status, session_id).await?;
        Ok(connection)
    }

    async fn connect(
        &mut self,
        spawn_args: Vec<String>,
        inherit_daemon_stdio: bool,
    ) -> Result<DaemonConnection> {
        let mut notes = Vec::new();
        if let Some(addr) = discover(DISCOVERY_TIMEOUT, &self.cwd).await? {
            match self.connect_existing(&addr).await? {
                Some(connection) => return Ok(connection),
                None => notes.push(format!(
                    "daemon at {addr} has unavailable controller storage; starting a replacement"
                )),
            }
        }
        self.spawn(spawn_args, notes, inherit_daemon_stdio).await
    }

    async fn connect_existing(&self, addr: &str) -> Result<Option<DaemonConnection>> {
        let mut client = GrpcClient::connect(addr).await?;
        let current = client
            .get_config()
            .await
            .with_context(|| format!("query daemon config at {addr}"))?;

        if let Some(storage_addr) = current.storage_service_addr.as_deref()
            && let Err(error) = probe_storage_service(storage_addr, STORAGE_PROBE_TIMEOUT).await
        {
            tracing::warn!(
                "daemon at {addr} has unavailable controller storage {storage_addr}: {error}"
            );
            return Ok(None);
        }

        // A second TUI is a protocol client, not the owner of an existing
        // healthy daemon. Preserve the tool/storage endpoint pair belonging
        // to that daemon's controller instead of stealing tool forwarding.
        let desired =
            config_for_existing_controller(&self.desired_config, &current, &self.storage_addr);
        let outcome = provision_config(&mut client, &desired, true)
            .await
            .with_context(|| format!("provision daemon config at {addr}"))?;
        if outcome.pushed {
            tracing::info!("provisioned daemon config at {addr} via settings RPC");
        }
        let state = client.get_state().await?;
        Ok(Some(DaemonConnection {
            client,
            status: wire_status(&state),
            reused: true,
            notes: outcome.notes,
        }))
    }

    async fn spawn(
        &mut self,
        args: Vec<String>,
        mut notes: Vec<String>,
        inherit_stdio: bool,
    ) -> Result<DaemonConnection> {
        let mut child = match daemon_spawn_kind(self.daemon_mode, inherit_stdio) {
            DaemonSpawnKind::Inherit => spawn_daemon(&self.cwd, &args),
            DaemonSpawnKind::Quiet => spawn_daemon_quiet(&self.cwd, &args),
            DaemonSpawnKind::Detached => spawn_daemon_detached(&self.cwd, &args),
        }
        .with_context(|| format!("spawn thewayd in {}", self.cwd.display()))?;
        let pid = child.id();
        let addr = match wait_ready(SPAWN_TIMEOUT, &self.cwd, pid).await {
            Ok(addr) => addr,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        };
        tracing::info!("spawned daemon at {addr}");
        self.children.push(child);

        let mut client = GrpcClient::connect(&addr).await?;
        let outcome = provision_config(&mut client, &self.desired_config, false)
            .await
            .with_context(|| format!("provision daemon config at {addr}"))?;
        if outcome.pushed {
            tracing::info!("provisioned daemon config at {addr} via settings RPC");
        }
        notes.extend(outcome.notes);
        let state = client.get_state().await?;
        Ok(DaemonConnection {
            client,
            status: wire_status(&state),
            reused: false,
            notes,
        })
    }

    fn reap_children(&mut self) {
        self.children.retain_mut(|child| match child.try_wait() {
            Ok(Some(_)) => false,
            Ok(None) => true,
            Err(error) => {
                tracing::warn!("failed to reap daemon child {}: {error}", child.id());
                false
            }
        });
    }
}

impl Drop for DaemonConnector {
    fn drop(&mut self) {
        // Ending the controller services makes a controller-backed daemon's
        // storage watchdog perform a bounded, graceful shutdown. A standalone
        // daemon spawned with `theway --daemon` has no controller storage, so
        // it keeps running after this client exits.
        self.tool_server.abort();
        self.storage_server.abort();
        self.reap_children();
    }
}

fn config_for_existing_controller(
    desired: &WireDaemonConfig,
    current: &WireDaemonConfig,
    own_storage_addr: &str,
) -> WireDaemonConfig {
    if current.storage_service_addr.as_deref() == Some(own_storage_addr) {
        return desired.clone();
    }
    let mut attached = desired.clone();
    attached.tool_service_addr = current.tool_service_addr.clone();
    attached.storage_service_addr = current.storage_service_addr.clone();
    attached
}

async fn restore_session(
    client: &mut GrpcClient,
    current: WireStatus,
    session_id: &str,
) -> Result<WireStatus> {
    if current.session_id == session_id {
        return Ok(current);
    }
    // No session switch: read the requested session's state directly.
    let state = client.get_state_for_session(session_id).await?;
    let status = wire_status(&state);
    if status.session_id != session_id {
        anyhow::bail!(
            "daemon did not restore session {session_id} within {SESSION_RESTORE_TIMEOUT:?}"
        );
    }
    Ok(status)
}

#[cfg(test)]
mod tests {
    use super::{
        DaemonSpawnKind, apply_controller_endpoints, config_for_existing_controller,
        daemon_spawn_kind,
    };
    use theway_transport::wire::WireDaemonConfig;

    #[test]
    fn daemon_spawn_kind_covers_all_modes() {
        assert_eq!(daemon_spawn_kind(false, true), DaemonSpawnKind::Inherit);
        assert_eq!(daemon_spawn_kind(false, false), DaemonSpawnKind::Quiet);
        assert_eq!(daemon_spawn_kind(true, true), DaemonSpawnKind::Detached);
        assert_eq!(daemon_spawn_kind(true, false), DaemonSpawnKind::Detached);
    }

    #[test]
    fn apply_controller_endpoints_attaches_in_default_mode() {
        let mut desired = WireDaemonConfig::default();
        apply_controller_endpoints(
            &mut desired,
            false,
            "127.0.0.1:1001".into(),
            "127.0.0.1:1002".into(),
        );
        assert_eq!(desired.tool_service_addr.as_deref(), Some("127.0.0.1:1001"));
        assert_eq!(
            desired.storage_service_addr.as_deref(),
            Some("127.0.0.1:1002")
        );
    }

    #[test]
    fn apply_controller_endpoints_skips_controller_services_in_daemon_mode() {
        let mut desired = WireDaemonConfig::default();
        apply_controller_endpoints(
            &mut desired,
            true,
            "127.0.0.1:1001".into(),
            "127.0.0.1:1002".into(),
        );
        assert_eq!(desired.tool_service_addr, None);
        assert_eq!(desired.storage_service_addr, None);
    }

    #[test]
    fn foreign_controller_endpoints_are_preserved_on_attach() {
        let desired = WireDaemonConfig {
            tool_service_addr: Some("127.0.0.1:1001".into()),
            storage_service_addr: Some("127.0.0.1:1002".into()),
            model: Some("new-model".into()),
            ..WireDaemonConfig::default()
        };
        let current = WireDaemonConfig {
            tool_service_addr: Some("127.0.0.1:2001".into()),
            storage_service_addr: Some("127.0.0.1:2002".into()),
            ..WireDaemonConfig::default()
        };

        let attached = config_for_existing_controller(&desired, &current, "127.0.0.1:1002");
        assert_eq!(attached.tool_service_addr, current.tool_service_addr);
        assert_eq!(attached.storage_service_addr, current.storage_service_addr);
        assert_eq!(attached.model.as_deref(), Some("new-model"));
    }

    #[test]
    fn owning_controller_can_reprovision_its_tool_endpoint() {
        let desired = WireDaemonConfig {
            tool_service_addr: Some("127.0.0.1:1001".into()),
            storage_service_addr: Some("127.0.0.1:1002".into()),
            ..WireDaemonConfig::default()
        };
        let current = WireDaemonConfig {
            tool_service_addr: Some("127.0.0.1:9999".into()),
            storage_service_addr: Some("127.0.0.1:1002".into()),
            ..WireDaemonConfig::default()
        };

        assert_eq!(
            config_for_existing_controller(&desired, &current, "127.0.0.1:1002"),
            desired
        );
    }
}
