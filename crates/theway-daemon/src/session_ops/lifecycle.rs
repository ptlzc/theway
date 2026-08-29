//! Session lifecycle query/mutation ops (list / create / rename / delete).

use std::collections::HashMap;

use anyhow::{Context as _, Result, bail};
use theway_core::multiagent::graph::types::DagStatus;
use theway_transport::wire::{SessionSummary, epoch_millis_to_rfc3339};

use super::AppSessionOps;
use super::metadata::{append_session_metadata, read_session_metadata};

pub(super) async fn list(ops: &AppSessionOps) -> Result<Vec<SessionSummary>> {
    let runs = ops.dag_engine.list_runs();

    let mut summaries = Vec::new();
    for record in ops.repo.list().await? {
        let session_runs = runs
            .iter()
            .filter(|run| run.session_id.as_deref() == Some(record.id.as_str()));
        let graph_count = session_runs.clone().count() as u32;
        let active_graph_count = session_runs
            .filter(|run| run.status == DagStatus::Running)
            .count() as u32;

        let (metadata, last_user_text) = match ops.repo.open(&record.id).await? {
            Some(session) => (
                read_session_metadata(session.as_ref()).await?,
                theway_storage::session::last_user_text(session.as_ref()).await,
            ),
            None => (HashMap::new(), None),
        };
        // Plugin-set session name wins; otherwise fall back to the last user
        // input, truncated to 15 chars, as the display title.
        let name = record
            .name
            .clone()
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| {
                last_user_text
                    .as_deref()
                    .map(|text| text.chars().take(15).collect())
                    .unwrap_or_default()
            });
        summaries.push(SessionSummary {
            session_id: record.id,
            name,
            cwd: record.cwd,
            model: record.model,
            created_at: record.created_at,
            last_activity_at: record.last_activity_at,
            last_activity_at_rfc3339: epoch_millis_to_rfc3339(record.last_activity_at),
            graph_count,
            active_graph_count,
            busy: false,
            preview: record.preview,
            tree_prefix: record.tree_prefix,
            metadata,
        });
    }
    Ok(summaries)
}

pub(super) async fn create(
    ops: &AppSessionOps,
    session_id: Option<&str>,
    metadata: &HashMap<String, String>,
) -> Result<String> {
    // New sessions record the daemon work_dir so activation can resolve the
    // matching execution context.
    let cwd = if ops.cwd.is_empty() {
        ".".to_string()
    } else {
        ops.cwd.clone()
    };
    let session = ops
        .repo
        .create_with_id(std::path::Path::new(&cwd), session_id)
        .await?;
    let meta = session.get_metadata_json().await?;
    let id = meta
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    if !metadata.is_empty() {
        append_session_metadata(session.as_ref(), metadata).await?;
    }
    Ok(id)
}

pub(super) async fn update_metadata(
    ops: &AppSessionOps,
    id: &str,
    metadata: &HashMap<String, String>,
) -> Result<()> {
    let session = ops
        .repo
        .open(id)
        .await?
        .with_context(|| format!("no session matches id {id}"))?;
    let mut merged = read_session_metadata(session.as_ref()).await?;
    merged.extend(metadata.iter().map(|(k, v)| (k.clone(), v.clone())));
    append_session_metadata(session.as_ref(), &merged).await?;
    Ok(())
}

pub(super) async fn rename(ops: &AppSessionOps, id: &str, name: &str) -> Result<()> {
    let name = name.trim();
    if name.is_empty() {
        bail!("session name must not be empty");
    }
    let session = ops
        .repo
        .open(id)
        .await?
        .with_context(|| format!("no session matches id {id}"))?;
    theway_storage::session::append_session_name(session.as_ref(), name).await?;
    Ok(())
}

pub(super) async fn delete(ops: &AppSessionOps, id: &str) -> Result<Vec<String>> {
    let session = ops
        .repo
        .open(id)
        .await?
        .with_context(|| format!("no session matches id {id}"))?;
    let meta = session.get_metadata_json().await?;
    let session_id = meta
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    // Delete protection: refuse while any of this session's DAG runs is still active.
    let active: Vec<String> = ops
        .dag_engine
        .list_runs()
        .iter()
        .filter(|run| {
            run.session_id.as_deref() == Some(session_id.as_str())
                && run.status == DagStatus::Running
        })
        .map(|run| run.id.clone())
        .collect();
    if !active.is_empty() {
        return Ok(active);
    }

    ops.repo.delete(id).await?;
    // Credentials are memory-only and session-scoped; deleting the session
    // must also drop its zeroizing secrets and execution context.
    ops.session_execution.remove(&session_id);
    Ok(Vec::new())
}
