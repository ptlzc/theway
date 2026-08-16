//! Session helpers (daemon-kernel-layers: moved from the SDK into storage —
//! repo-adjacent resume / list / delete semantics scoped to the current cwd
//! hash, shared by the daemon runtime and the CLI offline session commands).
//! Wraps [`SqliteSessionRepo`] with cwd-scoped semantics.

use std::path::PathBuf;

use crate::sqlite_repo::SqliteSessionRepo;
use anyhow::{Context, Result, bail};
use theway_core::{Session, SessionStorage};

use theway_contract::config::sessions_dir_for_cwd;

pub struct SessionEntry {
    #[allow(dead_code)] // listed via the public API; not read by the CLI itself.
    pub path: PathBuf,
    pub id: String,
    pub created_at: String,
    pub preview: Option<String>,
    pub automation: AutomationCounts,
    /// Parent session id for forks (`parentSessionPath` metadata resolved to the
    /// parent's id). `None` for root sessions.
    pub parent_id: Option<String>,
}

pub async fn open_repo(cwd: &std::path::Path) -> SqliteSessionRepo {
    SqliteSessionRepo::new(sessions_dir_for_cwd(cwd))
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
    repo: &SqliteSessionRepo,
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
    repo: &SqliteSessionRepo,
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
pub async fn create(repo: &SqliteSessionRepo, cwd: &std::path::Path) -> Result<Session> {
    Ok(repo.create(cwd.to_string_lossy().to_string()).await?)
}

/// Resume the most recent session for this cwd, or a specific one by id when supplied.
pub async fn resume(repo: &SqliteSessionRepo, explicit_id: Option<&str>) -> Result<Session> {
    let files = repo.list().await?;
    if files.is_empty() {
        bail!("no sessions to resume in {}", repo.root().display());
    }
    let chosen = if let Some(id) = explicit_id {
        find_session_path(repo, &files, id)
            .await?
            .with_context(|| format!("no session matches id {id}"))?
    } else {
        // SqliteSessionRepo::list() sorts ascending by name (UUIDv7), so the tail is newest.
        files.last().cloned().unwrap()
    };
    Ok(repo.open(&chosen).await?)
}

/// List sessions for this cwd, oldest → newest, with a short preview from the first user
/// message when available. `parent_id` is resolved from each session's `parentSessionPath`
/// metadata (fork lineage).
pub async fn list_entries(repo: &SqliteSessionRepo) -> Result<Vec<SessionEntry>> {
    struct Raw {
        path: PathBuf,
        id: String,
        created_at: String,
        preview: Option<String>,
        automation: AutomationCounts,
        parent_path: Option<String>,
    }
    let mut raw: Vec<Raw> = Vec::new();
    for path in repo.list().await? {
        // The daemon holds an exclusive libsql lock on its live session db, so
        // a cross-process listing (CLI `--list-sessions`, the resume picker
        // while a daemon runs) cannot open it. Degrade that one row instead of
        // failing the whole listing: file stem is the session id, the rest of
        // the fields stay blank until the daemon releases the lock.
        let session = match repo.open(&path).await {
            Ok(s) => s,
            Err(_) => {
                let stem = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("?")
                    .to_string();
                raw.push(Raw {
                    path: path.clone(),
                    id: stem,
                    created_at: "?".into(),
                    preview: None,
                    automation: automation_counts(&path).await,
                    parent_path: None,
                });
                continue;
            }
        };
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
        let parent_path = meta
            .get("parentSessionPath")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let preview = first_user_text(&session).await;
        let automation = automation_counts(&path).await;
        raw.push(Raw {
            path,
            id,
            created_at,
            preview,
            automation,
            parent_path,
        });
    }
    // Resolve parent *paths* to parent *ids* (fork lineage for the tree display).
    let path_to_id: std::collections::HashMap<String, String> = raw
        .iter()
        .map(|r| (r.path.to_string_lossy().into_owned(), r.id.clone()))
        .collect();
    Ok(raw
        .into_iter()
        .map(|r| SessionEntry {
            parent_id: r
                .parent_path
                .as_deref()
                .and_then(|p| path_to_id.get(p))
                .cloned(),
            path: r.path,
            id: r.id,
            created_at: r.created_at,
            preview: r.preview,
            automation: r.automation,
        })
        .collect())
}

/// Fork a session pi-style: create a brand-new session database whose transcript is the
/// given `entries` (a path-to-root chain ending just before the fork point, as produced by
/// `get_entries_to_fork`), and record the parent via `parentSessionPath`.
///
/// Entry ids and their `parentId` chain are preserved verbatim, so the fork replays the
/// parent's history up to the fork point. `current_leaf` in the new database resolves to
/// the last replayed entry, so the next appended message continues from the fork point.
pub async fn fork_session(
    repo: &SqliteSessionRepo,
    cwd: &std::path::Path,
    parent: &Session,
    entries: Vec<theway_core::SessionTreeEntry>,
) -> Result<Session> {
    let parent_meta = parent.storage().get_metadata_json().await?;
    let parent_path = parent_meta
        .get("path")
        .and_then(|v| v.as_str())
        .map(std::path::PathBuf::from)
        .with_context(|| "parent session has no recorded path")?;
    let parent_id = parent_meta
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("?")
        .to_string();

    tokio::fs::create_dir_all(repo.root())
        .await
        .with_context(|| format!("create {}", repo.root().display()))?;
    let file = repo
        .root()
        .join(format!("{}.db", theway_core::create_session_id()));
    let storage = crate::sqlite_storage::SqliteSessionStorage::create(
        &file,
        cwd.to_string_lossy().to_string(),
    )
    .await?;
    for entry in &entries {
        storage.append_entry(entry.clone()).await?;
    }
    storage.set_parent_session_path(&parent_path).await?;
    storage
        .checkpoint()
        .await
        .with_context(|| format!("checkpoint {}", file.display()))?;
    let session = Session::new(
        std::sync::Arc::new(storage) as std::sync::Arc<dyn theway_core::SessionStorage>
    );
    // Validate the replay the same way archive import does.
    session
        .build_context()
        .await
        .with_context(|| format!("validate forked session (forked from {parent_id})"))?;
    Ok(session)
}

// ── tree-shaped history (pi parity) ────────────────────────────────────────────────────

/// One row of the flattened session tree: display fields plus the pi-style prefix
/// (`├─ `/`└─ `/`│ ` continuation) that nests forked children under their parents.
pub struct SessionTreeRow {
    pub path: PathBuf,
    pub id: String,
    pub created_at: String,
    pub preview: Option<String>,
    pub automation: AutomationCounts,
    /// Depth in the fork tree; 0 = root session.
    pub depth: u16,
    /// Tree prefix (empty for roots): continuation bars + `├─ `/`└─ ` connector.
    pub prefix: String,
}

/// Flatten `entries` (chronological, oldest → newest) into tree-display order with pi-style
/// prefixes. Forks appear nested directly under their parent session, so the tree reads
/// top-down as the fork history grows. Entries with a dangling or cyclic parent are kept
/// at root level rather than dropped.
pub fn flatten_session_tree(entries: &[SessionEntry]) -> Vec<SessionTreeRow> {
    // Direct children of each session id, in list order (oldest → newest).
    let mut children: std::collections::HashMap<String, Vec<usize>> =
        std::collections::HashMap::new();
    for (i, e) in entries.iter().enumerate() {
        if let Some(parent) = &e.parent_id {
            children.entry(parent.clone()).or_default().push(i);
        }
    }

    // Ancestor chain (ids, deepest last) for a row, with cycle protection. A
    // self-parent (or any chain that reaches the row itself) is treated as a root.
    let ancestors = |i: usize| -> Vec<String> {
        let mut chain = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut cur = entries[i].parent_id.clone();
        while let Some(id) = cur {
            if id == entries[i].id || !seen.insert(id.clone()) || chain.len() >= 32 {
                break;
            }
            chain.push(id.clone());
            cur = entries
                .iter()
                .find(|e| e.id == id)
                .and_then(|e| e.parent_id.clone());
        }
        chain.reverse(); // root ancestor first
        chain
    };

    entries
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let chain = ancestors(i);
            let depth = chain.len() as u16;
            let mut prefix = String::new();
            // Continuation column per ancestor: "│ " while the ancestor still has
            // children further down the list, "  " afterwards.
            for anc in &chain {
                let has_later = children
                    .get(anc)
                    .is_some_and(|kids| kids.iter().any(|&k| k > i));
                prefix.push_str(if has_later { "│ " } else { "  " });
            }
            // The row's own connector (roots have none). Anchored to the
            // cycle-safe chain, so a self-parent row can't get a connector.
            if let Some(parent) = chain.last() {
                let is_last = children
                    .get(parent)
                    .is_some_and(|kids| kids.last() == Some(&i));
                prefix.push_str(if is_last { "└─ " } else { "├─ " });
            }
            SessionTreeRow {
                path: e.path.clone(),
                id: e.id.clone(),
                created_at: e.created_at.clone(),
                preview: e.preview.clone(),
                automation: e.automation,
                depth,
                prefix,
            }
        })
        .collect()
}

/// Text of a user message entry (text or text blocks joined), truncated and
/// newline-flattened for listing use. Public so the daemon's `/fork` listing can
/// reuse the same preview shape as `first_user_text`.
pub fn user_message_text(entry: &theway_core::SessionTreeEntry) -> Option<String> {
    let theway_core::SessionTreeEntry::Message {
        message: theway_core::AgentMessage::Llm(theway_llm_provider::Message::User(u)),
        ..
    } = entry
    else {
        return None;
    };
    let text = match &u.content {
        theway_llm_provider::UserContent::Text(s) => s.clone(),
        theway_llm_provider::UserContent::Blocks(blocks) => blocks
            .iter()
            .filter_map(|b| match b {
                theway_llm_provider::UserContentBlock::Text(t) => Some(t.text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" "),
    };
    let text = text.replace('\n', " ");
    let preview = if text.chars().count() > 80 {
        let mut p: String = text.chars().take(80).collect();
        p.push('…');
        p
    } else {
        text
    };
    Some(preview)
}

/// First user message text, truncated to a short preview. Public so the daemon's
/// session-ops layer (which no longer owns this module) can build listings from it.
pub async fn first_user_text(session: &Session) -> Option<String> {
    let entries = session.entries().await.ok()?;
    for e in entries {
        if let Some(preview) = user_message_text(&e) {
            return Some(preview);
        }
    }
    None
}

/// Delete a session by id (full UUIDv7) or a unique prefix.
pub async fn delete_by_id(repo: &SqliteSessionRepo, id: &str) -> Result<PathBuf> {
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
    repo: &SqliteSessionRepo,
    files: &[PathBuf],
    id: &str,
) -> Result<Option<PathBuf>> {
    // Fast path: the db file stem IS the session id for both created and
    // imported sessions (uuidv7, see SqliteSessionStorage::create /
    // create_with_id). Never opens a db, so a live daemon holding the WAL
    // lock on the newest session can't block resolving an older one.
    for path in files {
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        if stem == id || stem.starts_with(id) {
            return Ok(Some(path.clone()));
        }
    }
    // Slow path: files whose stem was renamed away from the metadata id. Each
    // open may collide with a live daemon's lock; only reachable when no stem
    // matched.
    for path in files {
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
pub async fn find_path_by_id(repo: &SqliteSessionRepo, id: &str) -> Result<Option<PathBuf>> {
    let files = repo.list().await?;
    find_session_path(repo, &files, id).await
}

/// Return the newest session transcript path for this cwd-scoped repo.
pub async fn newest_path(repo: &SqliteSessionRepo) -> Result<Option<PathBuf>> {
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
    repo: &SqliteSessionRepo,
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
tests_bridge_macro::tests_bridge!("session");
