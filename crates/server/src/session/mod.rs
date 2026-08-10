//! Session helpers — wraps `theway_core::JsonlSessionRepo` with resume / list / delete
//! semantics scoped to the current cwd hash.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use theway_core::{JsonlSessionRepo, Session};

use crate::config::sessions_dir_for_cwd;

pub struct SessionEntry {
    #[allow(dead_code)] // listed via the public API; not read by the CLI itself.
    pub path: PathBuf,
    pub id: String,
    pub created_at: String,
    pub preview: Option<String>,
    pub automation: AutomationCounts,
}

pub async fn open_repo(cwd: &std::path::Path) -> JsonlSessionRepo {
    JsonlSessionRepo::new(sessions_dir_for_cwd(cwd))
}

/// Dynamic trigger rules are session-scoped sidecars next to the jsonl transcript.
pub fn trigger_sidecar_path(session_path: &std::path::Path) -> PathBuf {
    session_path.with_extension("triggers.json")
}

/// Cron jobs are session-scoped by default, parallel to dynamic trigger sidecars.
pub fn cron_sidecar_path(session_path: &std::path::Path) -> PathBuf {
    session_path.with_extension("cron.toml")
}

/// Return the dynamic-trigger sidecar for a live session.
///
/// Jsonl sessions record their absolute transcript path in metadata. Older or synthetic
/// sessions may not have that field, so keep a deterministic fallback under the repo root.
pub async fn trigger_sidecar_path_for_session(
    session: &Session,
    repo: &JsonlSessionRepo,
) -> Result<PathBuf> {
    let metadata = session.storage().get_metadata_json().await?;
    if let Some(path) = metadata.get("path").and_then(|v| v.as_str()) {
        return Ok(trigger_sidecar_path(std::path::Path::new(path)));
    }

    let session_id = metadata
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown-session");
    Ok(repo.root().join(format!("{session_id}.triggers.json")))
}

/// Public endpoint bindings are session-scoped sidecars, parallel to trigger sidecars.
pub fn endpoint_sidecar_path(session_path: &std::path::Path) -> PathBuf {
    session_path.with_extension("endpoints.json")
}

/// Return the cron sidecar for a live session.
pub async fn cron_sidecar_path_for_session(
    session: &Session,
    repo: &JsonlSessionRepo,
) -> Result<PathBuf> {
    let metadata = session.storage().get_metadata_json().await?;
    if let Some(path) = metadata.get("path").and_then(|v| v.as_str()) {
        return Ok(cron_sidecar_path(std::path::Path::new(path)));
    }

    let session_id = metadata
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown-session");
    Ok(repo.root().join(format!("{session_id}.cron.toml")))
}

/// Create a brand-new session under the current cwd's sessions dir.
pub async fn create(repo: &JsonlSessionRepo, cwd: &std::path::Path) -> Result<Session> {
    Ok(repo.create(cwd.to_string_lossy().to_string()).await?)
}

/// Resume the most recent session for this cwd, or a specific one by id when supplied.
pub async fn resume(repo: &JsonlSessionRepo, explicit_id: Option<&str>) -> Result<Session> {
    let files = repo.list().await?;
    if files.is_empty() {
        bail!("no sessions to resume in {}", repo.root().display());
    }
    let chosen = if let Some(id) = explicit_id {
        find_session_path(repo, &files, id)
            .await?
            .with_context(|| format!("no session matches id {id}"))?
    } else {
        // JsonlSessionRepo::list() sorts ascending by name (UUIDv7), so the tail is newest.
        files.last().cloned().unwrap()
    };
    Ok(repo.open(&chosen).await?)
}

/// List sessions for this cwd, oldest → newest, with a short preview from the first user
/// message when available.
pub async fn list_entries(repo: &JsonlSessionRepo) -> Result<Vec<SessionEntry>> {
    let mut out = Vec::new();
    for path in repo.list().await? {
        let session = repo.open(&path).await?;
        let meta = session.storage().get_metadata_json().await?;
        let id = meta
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("?")
            .to_string();
        let created_at = meta
            .get("createdAt")
            .and_then(|v| v.as_str())
            .unwrap_or("?")
            .to_string();
        let preview = first_user_text(&session).await;
        let automation = automation_counts(&path).await;
        out.push(SessionEntry {
            path,
            id,
            created_at,
            preview,
            automation,
        });
    }
    Ok(out)
}

pub(crate) async fn first_user_text(session: &Session) -> Option<String> {
    let entries = session.entries().await.ok()?;
    for e in entries {
        if let theway_core::SessionTreeEntry::Message {
            message: theway_core::AgentMessage::Llm(theway_llm_provider::Message::User(u)),
            ..
        } = e
        {
            let text = match u.content {
                theway_llm_provider::UserContent::Text(s) => s,
                theway_llm_provider::UserContent::Blocks(blocks) => blocks
                    .into_iter()
                    .filter_map(|b| match b {
                        theway_llm_provider::UserContentBlock::Text(t) => Some(t.text),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join(" "),
            };
            // Char-bounded truncation: `String::truncate` works in bytes and panics if
            // the cutoff lands inside a multi-byte UTF-8 character (CJK / emoji).
            let text = text.replace('\n', " ");
            let preview = if text.chars().count() > 80 {
                let mut p: String = text.chars().take(80).collect();
                p.push('…');
                p
            } else {
                text
            };
            return Some(preview);
        }
    }
    None
}

/// Delete a session by id (full UUIDv7) or a unique prefix.
pub async fn delete_by_id(repo: &JsonlSessionRepo, id: &str) -> Result<PathBuf> {
    let files = repo.list().await?;
    let path = find_session_path(repo, &files, id)
        .await?
        .with_context(|| format!("no session matches id {id}"))?;
    repo.delete(&path).await?;
    let trigger_sidecar = trigger_sidecar_path(&path);
    match tokio::fs::remove_file(&trigger_sidecar).await {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).with_context(|| format!("delete {}", trigger_sidecar.display())),
    }
    let cron_sidecar = cron_sidecar_path(&path);
    match tokio::fs::remove_file(&cron_sidecar).await {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).with_context(|| format!("delete {}", cron_sidecar.display())),
    }
    let endpoint_sidecar = endpoint_sidecar_path(&path);
    match tokio::fs::remove_file(&endpoint_sidecar).await {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).with_context(|| format!("delete {}", endpoint_sidecar.display())),
    }
    Ok(path)
}

async fn find_session_path(
    repo: &JsonlSessionRepo,
    files: &[PathBuf],
    id: &str,
) -> Result<Option<PathBuf>> {
    for path in files {
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        if stem == id || stem.starts_with(id) {
            return Ok(Some(path.clone()));
        }

        let session = repo.open(path).await?;
        let metadata_id = session
            .storage()
            .get_metadata_json()
            .await?
            .get("id")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        if metadata_id
            .as_deref()
            .map(|s| s == id || s.starts_with(id))
            .unwrap_or(false)
        {
            return Ok(Some(path.clone()));
        }
    }
    Ok(None)
}

/// Resolve a session id (full UUIDv7 or unique prefix) to its transcript path.
pub async fn find_path_by_id(repo: &JsonlSessionRepo, id: &str) -> Result<Option<PathBuf>> {
    let files = repo.list().await?;
    find_session_path(repo, &files, id).await
}

/// Return the newest session transcript path for this cwd-scoped repo.
pub async fn newest_path(repo: &JsonlSessionRepo) -> Result<Option<PathBuf>> {
    let files = repo.list().await?;
    Ok(files.last().cloned())
}

/// Enabled/total counts of a session's automation sidecars (cron jobs + dynamic trigger
/// rules). Cron jobs and triggers are session-scoped, so after exiting a session this is
/// the only record that automation exists at all.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AutomationCounts {
    pub cron_enabled: usize,
    pub cron_total: usize,
    pub trigger_enabled: usize,
    pub trigger_total: usize,
}

impl AutomationCounts {
    pub fn is_empty(&self) -> bool {
        self.cron_total == 0 && self.trigger_total == 0
    }

    pub fn any_enabled(&self) -> bool {
        self.cron_enabled > 0 || self.trigger_enabled > 0
    }

    /// Short badge for session listings: enabled counts ("2 cron, 1 trigger"), or
    /// "automation off" when everything present is disabled. `None` when there is nothing.
    pub fn badge(&self) -> Option<String> {
        if self.is_empty() {
            return None;
        }
        let mut parts = Vec::new();
        if self.cron_enabled > 0 {
            parts.push(format!("{} cron", self.cron_enabled));
        }
        if self.trigger_enabled > 0 {
            parts.push(format!("{} trigger", self.trigger_enabled));
        }
        if parts.is_empty() {
            return Some("automation off".into());
        }
        Some(parts.join(", "))
    }
}

/// Minimal sidecar shapes for counting: only `enabled` matters here, every other field is
/// ignored so format growth in the real types can't break listings.
#[derive(serde::Deserialize)]
struct EnabledOnly {
    #[serde(default)]
    enabled: bool,
}

#[derive(serde::Deserialize)]
struct CronSidecarLite {
    #[serde(default)]
    jobs: Vec<EnabledOnly>,
}

#[derive(serde::Deserialize)]
struct TriggerSidecarLite {
    #[serde(default)]
    rules: Vec<EnabledOnly>,
}

/// Count automation in the sidecars next to `session_path`. Missing or unparsable sidecar
/// files degrade to zero counts — this feeds listings and hints, never hard errors.
pub async fn automation_counts(session_path: &std::path::Path) -> AutomationCounts {
    let mut counts = AutomationCounts::default();
    if let Ok(text) = tokio::fs::read_to_string(cron_sidecar_path(session_path)).await
        && let Ok(file) = toml::from_str::<CronSidecarLite>(&text)
    {
        counts.cron_total = file.jobs.len();
        counts.cron_enabled = file.jobs.iter().filter(|j| j.enabled).count();
    }
    if let Ok(text) = tokio::fs::read_to_string(trigger_sidecar_path(session_path)).await
        && let Ok(file) = serde_json::from_str::<TriggerSidecarLite>(&text)
    {
        counts.trigger_total = file.rules.len();
        counts.trigger_enabled = file.rules.iter().filter(|r| r.enabled).count();
    }
    counts
}

/// When other sessions in this repo hold *enabled* automation, return a one-line hint
/// naming the newest such session so the user can resume it. `current` (the active
/// session's transcript path) is excluded from the scan.
pub async fn automation_elsewhere_hint(
    repo: &JsonlSessionRepo,
    current: Option<&std::path::Path>,
) -> Option<String> {
    let files = repo.list().await.ok()?;
    let current_stem = current.and_then(|p| p.file_stem()).map(|s| s.to_owned());
    let mut holders = Vec::new();
    for path in files {
        if current_stem.is_some() && path.file_stem().map(|s| s.to_owned()) == current_stem {
            continue;
        }
        let counts = automation_counts(&path).await;
        if counts.any_enabled() {
            holders.push((path, counts));
        }
    }
    let extra = holders.len().saturating_sub(1);
    // repo.list() is ascending by UUIDv7, so the last holder is the newest.
    let (path, counts) = holders.pop()?;
    let short_id: String = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
        .chars()
        .take(16)
        .collect();
    let badge = counts.badge().unwrap_or_default();
    let more = if extra > 0 {
        format!(" (+{extra} more session(s))")
    } else {
        String::new()
    };
    Some(format!(
        "automation is session-scoped: session {short_id} has {badge} enabled{more}; resume it with `theway --resume-id {short_id}`"
    ))
}

#[cfg(test)]
// Test files live in `tests/session/` (mirror of src), pulled in by
// path so they keep unit-test semantics (private access). See docs/RUST_TEST_FILES.md.
tests_bridge!("../../tests/session/mod.rs");
