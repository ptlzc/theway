//! `.theway-session` export/import support.
//!
//! The archive is intentionally small and inspectable: a tar file with a manifest, one
//! session transcript (rendered as JSONL lines), and optional session-scoped automation
//! sidecars. It preserves transcript/tool history, so callers must render a sensitivity
//! warning.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Cursor, Read};
use std::path::{Component, Path, PathBuf};

use crate::sqlite_repo::SqliteSessionRepo;
use crate::sqlite_storage::SqliteSessionStorage;
use anyhow::{Context, Result, anyhow, bail};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use theway_contract::session::{
    JsonlSessionMetadata, SessionImportOrigin, SessionReader, SessionStore, StoredSessionEntry,
    validate_session_entries,
};

use theway_contract::triggers::CronJob;
use theway_contract::triggers::DynamicTriggerRule;

const SCHEMA: &str = "theway.session_export.v1";
const MANIFEST_PATH: &str = "manifest.json";
const SESSION_PATH: &str = "session.jsonl";
const TRIGGERS_PATH: &str = "sidecars/triggers.json";
const CRON_PATH: &str = "sidecars/cron.toml";
const MAX_MANIFEST_BYTES: usize = 128 * 1024;
const MAX_SESSION_BYTES: usize = 50 * 1024 * 1024;
const MAX_SIDECAR_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivateTriggers {
    Off,
    Ask,
    On,
}

#[derive(Debug)]
pub struct ExportSummary {
    pub output_path: PathBuf,
    pub session_id: String,
    pub entry_count: usize,
    pub has_triggers: bool,
    pub has_cron: bool,
}

#[derive(Debug)]
pub struct ImportSummary {
    pub session_id: String,
    pub session_path: PathBuf,
    pub entry_count: usize,
    pub triggers_imported: usize,
    pub cron_imported: usize,
    pub automation_enabled: bool,
    /// Ids that were enabled in the source archive. A disabled-by-default import keeps
    /// these so an interactive "activate now?" answer can restore exactly the source
    /// state (same semantics as `--activate-triggers=on`).
    pub originally_enabled_triggers: Vec<String>,
    pub originally_enabled_cron: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Manifest {
    schema: String,
    created_at: String,
    theway_version: String,
    source: ManifestSource,
    content: ManifestContent,
    sensitivity: ManifestSensitivity,
}

#[derive(Debug, Serialize, Deserialize)]
struct ManifestSource {
    session_id: String,
    cwd: String,
    session_path: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct ManifestContent {
    session_jsonl_sha256: String,
    entry_count: usize,
    active_leaf_id: Option<String>,
    has_triggers: bool,
    has_cron: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct ManifestSensitivity {
    session_transcript_preserved: bool,
    separate_auth_stores_included: bool,
    provider_credentials_included: bool,
    mcp_config_included: bool,
}

#[derive(Debug)]
struct ParsedSession {
    entries: Vec<StoredSessionEntry>,
    active_leaf_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct DynamicTriggerFile {
    version: u32,
    rules: Vec<DynamicTriggerRule>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct CronJobsFile {
    #[serde(default)]
    jobs: Vec<CronJob>,
}

pub async fn export_session<R: SessionReader + ?Sized>(
    session: &R,
    output_path: &Path,
    exclude_triggers: bool,
) -> Result<ExportSummary> {
    let metadata = session
        .get_metadata_json()
        .await
        .context("read session metadata")?;
    let session_path = metadata
        .get("path")
        .and_then(|v| v.as_str())
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("session metadata is missing transcript path"))?;
    let (session_jsonl, entry_count) = render_session_jsonl(&metadata, session).await?;
    if session_jsonl.len() > MAX_SESSION_BYTES {
        bail!("session transcript is too large to export");
    }
    let session_id = metadata
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let session_cwd = metadata
        .get("cwd")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let session_hash = sha256_hex(session_jsonl.as_bytes());
    let active_leaf_id = session.get_leaf_id().await.context("read active leaf")?;

    let trigger_path = crate::session::trigger_sidecar_path(&session_path);
    let cron_path = crate::session::cron_sidecar_path(&session_path);
    let trigger_bytes = if !exclude_triggers {
        read_optional_sidecar(&trigger_path).await?
    } else {
        None
    };
    let cron_bytes = if !exclude_triggers {
        read_optional_sidecar(&cron_path).await?
    } else {
        None
    };

    let manifest = Manifest {
        schema: SCHEMA.into(),
        created_at: Utc::now().to_rfc3339(),
        theway_version: env!("CARGO_PKG_VERSION").into(),
        source: ManifestSource {
            session_id: session_id.clone(),
            cwd: session_cwd,
            session_path: session_path.to_string_lossy().to_string(),
        },
        content: ManifestContent {
            session_jsonl_sha256: session_hash,
            entry_count,
            active_leaf_id: active_leaf_id.clone(),
            has_triggers: trigger_bytes.is_some(),
            has_cron: cron_bytes.is_some(),
        },
        sensitivity: ManifestSensitivity {
            session_transcript_preserved: true,
            separate_auth_stores_included: false,
            provider_credentials_included: false,
            mcp_config_included: false,
        },
    };

    if let Some(parent) = output_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("create {}", parent.display()))?;
    }
    let output = output_path.to_path_buf();
    let manifest_bytes = serde_json::to_vec_pretty(&manifest)?;
    let session_bytes = session_jsonl.into_bytes();
    let trigger_for_tar = trigger_bytes.clone();
    let cron_for_tar = cron_bytes.clone();
    tokio::task::spawn_blocking(move || -> Result<()> {
        let file = create_archive_file(&output)?;
        let mut tar = tar::Builder::new(file);
        append_bytes(&mut tar, MANIFEST_PATH, &manifest_bytes)?;
        append_bytes(&mut tar, SESSION_PATH, &session_bytes)?;
        if let Some(bytes) = trigger_for_tar.as_deref() {
            append_bytes(&mut tar, TRIGGERS_PATH, bytes)?;
        }
        if let Some(bytes) = cron_for_tar.as_deref() {
            append_bytes(&mut tar, CRON_PATH, bytes)?;
        }
        tar.finish().context("finish session archive")?;
        Ok(())
    })
    .await??;

    Ok(ExportSummary {
        output_path: output_path.to_path_buf(),
        session_id,
        entry_count,
        has_triggers: trigger_bytes.is_some(),
        has_cron: cron_bytes.is_some(),
    })
}

/// Render the canonical `session.jsonl` text for an archive: the header line
/// (metadata) followed by one line per tree entry, plus the entry count.
/// Backend-agnostic — renders from any `SessionStorage`; the archive transcript
/// stays JSONL so existing archives and tooling keep working.
async fn render_session_jsonl<R: SessionReader + ?Sized>(
    metadata: &serde_json::Value,
    session: &R,
) -> Result<(String, usize)> {
    let header: JsonlSessionMetadata = serde_json::from_value(metadata.clone())
        .context("session metadata is not a valid header")?;
    let mut out = serde_json::to_string(&header)?;
    out.push('\n');
    let entries = session
        .get_entries()
        .await
        .context("read session entries")?;
    validate_session_entries(&entries).context("validate session transcript")?;
    for entry in &entries {
        out.push_str(&serde_json::to_string(&entry.payload)?);
        out.push('\n');
    }
    Ok((out, entries.len()))
}

pub async fn import_session(
    repo: &SqliteSessionRepo,
    archive_path: &Path,
    cwd: &Path,
    activate_triggers: ActivateTriggers,
) -> Result<ImportSummary> {
    if activate_triggers == ActivateTriggers::Ask {
        bail!(
            "activate-triggers=ask requires interactive confirmation and is not implemented yet; use off or on"
        );
    }
    let archive_path = archive_path.to_path_buf();
    let files = tokio::task::spawn_blocking(move || read_archive(&archive_path)).await??;
    let manifest_bytes = files
        .get(MANIFEST_PATH)
        .ok_or_else(|| anyhow!("session archive is missing manifest.json"))?;
    let session_bytes = files
        .get(SESSION_PATH)
        .ok_or_else(|| anyhow!("session archive is missing session.jsonl"))?;

    let manifest: Manifest =
        serde_json::from_slice(manifest_bytes).context("parse session archive manifest")?;
    if manifest.schema != SCHEMA {
        bail!("unsupported session archive schema");
    }
    let actual_hash = sha256_hex(session_bytes);
    if actual_hash != manifest.content.session_jsonl_sha256 {
        bail!("session archive checksum mismatch");
    }
    let session_text = std::str::from_utf8(session_bytes).context("session.jsonl is not UTF-8")?;
    let parsed = parse_session_jsonl(session_text)?;
    if parsed.entries.len() != manifest.content.entry_count {
        bail!("session archive entry count mismatch");
    }
    if parsed.active_leaf_id != manifest.content.active_leaf_id {
        bail!("session archive active leaf mismatch");
    }
    let automation_enabled = activate_triggers == ActivateTriggers::On;
    let trigger_sidecar = files
        .get(TRIGGERS_PATH)
        .map(|bytes| rewrite_trigger_sidecar(bytes, automation_enabled))
        .transpose()?;
    let cron_sidecar = files
        .get(CRON_PATH)
        .map(|bytes| rewrite_cron_sidecar(bytes, automation_enabled))
        .transpose()?;
    let originally_enabled_triggers = files
        .get(TRIGGERS_PATH)
        .and_then(|bytes| serde_json::from_slice::<DynamicTriggerFile>(bytes).ok())
        .map(|file| {
            file.rules
                .iter()
                .filter(|rule| rule.enabled)
                .map(|rule| rule.id.clone())
                .collect()
        })
        .unwrap_or_default();
    let originally_enabled_cron = files
        .get(CRON_PATH)
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
        .and_then(|text| toml::from_str::<CronJobsFile>(text).ok())
        .map(|file| {
            file.jobs
                .iter()
                .filter(|job| job.enabled)
                .map(|job| job.id.clone())
                .collect()
        })
        .unwrap_or_default();

    tokio::fs::create_dir_all(repo.root())
        .await
        .with_context(|| format!("create {}", repo.root().display()))?;
    let new_id = uuid::Uuid::now_v7().to_string();
    let session_path = repo.root().join(format!("{new_id}.db"));
    if tokio::fs::try_exists(&session_path).await? {
        bail!("import destination already exists");
    }
    let temp_path = repo.root().join(format!("{new_id}.db.tmp"));
    let origin = Some(SessionImportOrigin {
        session_id: manifest.source.session_id.clone(),
        cwd: manifest.source.cwd.clone(),
        exported_at: manifest.created_at.clone(),
        theway_version: manifest.theway_version.clone(),
    });

    let mut sidecars: Vec<(PathBuf, String)> = Vec::new();
    let triggers_imported = match &trigger_sidecar {
        Some(rules) => {
            sidecars.push((
                crate::session::trigger_sidecar_path(&session_path),
                serde_json::to_string_pretty(rules)?,
            ));
            rules.rules.len()
        }
        None => 0,
    };
    let cron_imported = match &cron_sidecar {
        Some(jobs) => {
            sidecars.push((
                crate::session::cron_sidecar_path(&session_path),
                toml::to_string_pretty(jobs)?,
            ));
            jobs.jobs.len()
        }
        None => 0,
    };
    commit_import(
        &session_path,
        &temp_path,
        &parsed.entries,
        cwd,
        origin,
        &sidecars,
    )
    .await?;

    Ok(ImportSummary {
        session_id: new_id,
        session_path,
        entry_count: parsed.entries.len(),
        triggers_imported,
        cron_imported,
        automation_enabled,
        originally_enabled_triggers,
        originally_enabled_cron,
    })
}

/// Re-enable the given trigger/cron ids on an imported session's sidecars — the second
/// half of the interactive "activate imported automation now?" flow. Sync IO: callers run
/// from UI resolution paths; the sidecars are small.
pub fn activate_imported(
    session_path: &Path,
    trigger_ids: &[String],
    cron_ids: &[String],
) -> Result<(usize, usize)> {
    let mut triggers_enabled = 0usize;
    if !trigger_ids.is_empty() {
        let path = crate::session::trigger_sidecar_path(session_path);
        let text =
            std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        let mut file: DynamicTriggerFile =
            serde_json::from_str(&text).context("parse trigger sidecar")?;
        for rule in &mut file.rules {
            if trigger_ids.contains(&rule.id) && !rule.enabled {
                rule.enabled = true;
                triggers_enabled += 1;
            }
        }
        std::fs::write(&path, serde_json::to_string_pretty(&file)?)
            .with_context(|| format!("write {}", path.display()))?;
    }
    let mut cron_enabled = 0usize;
    if !cron_ids.is_empty() {
        let path = crate::session::cron_sidecar_path(session_path);
        let text =
            std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        let mut file: CronJobsFile = toml::from_str(&text).context("parse cron sidecar")?;
        for job in &mut file.jobs {
            if cron_ids.contains(&job.id) && !job.enabled {
                job.enabled = true;
                cron_enabled += 1;
            }
        }
        std::fs::write(&path, toml::to_string_pretty(&file)?)
            .with_context(|| format!("write {}", path.display()))?;
    }
    Ok((triggers_enabled, cron_enabled))
}

pub fn default_export_path(cwd: &Path, session_id: &str) -> PathBuf {
    let short: String = session_id.chars().take(16).collect();
    cwd.join(format!("theway-session-{short}.theway-session"))
}

fn parse_session_jsonl(text: &str) -> Result<ParsedSession> {
    let mut lines = text.lines();
    let header = lines
        .next()
        .ok_or_else(|| anyhow!("session transcript is empty"))?;
    let metadata: JsonlSessionMetadata =
        serde_json::from_str(header).context("parse session metadata")?;
    if metadata.base.id.trim().is_empty() {
        bail!("session metadata is missing id");
    }
    let mut entries = Vec::new();
    for (idx, line) in lines.enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let payload = serde_json::from_str(line)
            .with_context(|| format!("parse session entry line {}", idx + 2))?;
        let entry = StoredSessionEntry::from_payload(payload)
            .with_context(|| format!("parse session entry line {}", idx + 2))?;
        entries.push(entry);
    }
    let active_leaf_id =
        validate_session_entries(&entries).context("validate session transcript")?;
    Ok(ParsedSession {
        entries,
        active_leaf_id,
    })
}

/// Write all imported files with the session rename as the commit point. The
/// SQLite database is staged at `temp_path` (a non-`.db` name, invisible to repo
/// listings), populated, replay-validated there, and only renamed into place
/// after every sidecar landed. Any failure removes everything written so a
/// failed import leaves no orphan or partial session behind.
async fn commit_import(
    session_path: &Path,
    temp_path: &Path,
    entries: &[StoredSessionEntry],
    cwd: &Path,
    origin: Option<SessionImportOrigin>,
    sidecars: &[(PathBuf, String)],
) -> Result<()> {
    let result = async {
        let storage = SqliteSessionStorage::create_with_id(
            temp_path,
            cwd.to_string_lossy().to_string(),
            session_path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned()),
        )
        .await
        .map_err(|e| anyhow!("create session db: {e}"))?;
        for entry in entries {
            storage
                .append_entry(entry.clone())
                .await
                .map_err(|e| anyhow!("write session db: {e}"))?;
        }
        if let Some(o) = &origin {
            storage
                .set_import_origin(Some(o.clone()))
                .await
                .map_err(|e| anyhow!("write session db: {e}"))?;
        }
        storage
            .set_session_path(session_path)
            .await
            .map_err(|e| anyhow!("write session db: {e}"))?;
        // WAL mode keeps pages in `-wal` until checkpoint; without this the
        // rename below would drop everything not yet flushed into the main file.
        storage
            .checkpoint()
            .await
            .map_err(|e| anyhow!("checkpoint session db: {e}"))?;
        for (path, content) in sidecars {
            tokio::fs::write(path, content)
                .await
                .with_context(|| format!("write {}", path.display()))?;
        }
        tokio::fs::rename(temp_path, session_path)
            .await
            .with_context(|| format!("rename into {}", session_path.display()))?;
        // The WAL companion stays behind (now empty after TRUNCATE); sweep it
        // so the sessions dir doesn't accumulate `-wal` litter.
        let _ = tokio::fs::remove_file(format!("{}-wal", temp_path.display())).await;
        let _ = tokio::fs::remove_file(format!("{}-shm", temp_path.display())).await;
        Ok(())
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(temp_path).await;
        for (path, _) in sidecars {
            let _ = tokio::fs::remove_file(path).await;
        }
    }
    result
}

/// The archive carries the full transcript, so it is created owner-only and never
/// truncates an existing file.
fn create_archive_file(path: &Path) -> Result<File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    options.open(path).map_err(|err| {
        if err.kind() == std::io::ErrorKind::AlreadyExists {
            anyhow!(
                "output already exists: {} (remove it or pass a different path)",
                path.display()
            )
        } else {
            anyhow::Error::new(err).context(format!("create {}", path.display()))
        }
    })
}

async fn read_optional_sidecar(path: &Path) -> Result<Option<Vec<u8>>> {
    match tokio::fs::read(path).await {
        Ok(bytes) => {
            if bytes.len() > MAX_SIDECAR_BYTES {
                bail!("session sidecar is too large to export");
            }
            Ok(Some(bytes))
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err).with_context(|| format!("read {}", path.display())),
    }
}

fn append_bytes<W: std::io::Write>(
    tar: &mut tar::Builder<W>,
    path: &str,
    bytes: &[u8],
) -> Result<()> {
    let mut header = tar::Header::new_gnu();
    header.set_size(bytes.len() as u64);
    header.set_mode(0o600);
    header.set_cksum();
    tar.append_data(&mut header, path, Cursor::new(bytes))
        .with_context(|| format!("append {path}"))?;
    Ok(())
}

fn read_archive(path: &Path) -> Result<BTreeMap<String, Vec<u8>>> {
    let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut archive = tar::Archive::new(file);
    let mut files = BTreeMap::new();
    for entry in archive.entries().context("read session archive")? {
        let entry = entry.context("read archive entry")?;
        if !entry.header().entry_type().is_file() {
            bail!("session archive contains a non-file entry");
        }
        let path = entry.path().context("read archive entry path")?;
        validate_archive_path(&path)?;
        let rel = path
            .to_str()
            .ok_or_else(|| anyhow!("session archive contains non-UTF-8 path"))?
            .to_string();
        let limit = match rel.as_str() {
            MANIFEST_PATH => MAX_MANIFEST_BYTES,
            SESSION_PATH => MAX_SESSION_BYTES,
            TRIGGERS_PATH | CRON_PATH => MAX_SIDECAR_BYTES,
            _ => bail!("session archive contains an unexpected file"),
        };
        let mut bytes = Vec::new();
        entry
            .take((limit + 1) as u64)
            .read_to_end(&mut bytes)
            .context("read archive file")?;
        if bytes.len() > limit {
            bail!("session archive file is too large");
        }
        if files.insert(rel, bytes).is_some() {
            bail!("session archive contains duplicate file paths");
        }
    }
    Ok(files)
}

fn validate_archive_path(path: &Path) -> Result<()> {
    for component in path.components() {
        match component {
            Component::Normal(_) => {}
            _ => bail!("session archive contains an unsafe path"),
        }
    }
    Ok(())
}

/// Activation never widens what the source had: `activate` ANDs with each rule's own
/// `enabled` flag, and `fired_at` history is preserved so fire-once rules don't re-fire.
fn rewrite_trigger_sidecar(bytes: &[u8], activate: bool) -> Result<DynamicTriggerFile> {
    let mut file: DynamicTriggerFile =
        serde_json::from_slice(bytes).context("parse trigger sidecar")?;
    for rule in &mut file.rules {
        rule.enabled = rule.enabled && activate;
    }
    Ok(file)
}

fn rewrite_cron_sidecar(bytes: &[u8], activate: bool) -> Result<CronJobsFile> {
    let text = std::str::from_utf8(bytes).context("cron sidecar is not UTF-8")?;
    let mut file: CronJobsFile = toml::from_str(text).context("parse cron sidecar")?;
    for job in &mut file.jobs {
        job.enabled = job.enabled && activate;
        job.running_trace_id = None;
        job.last_due_at = None;
        job.last_error = None;
        job.skipped_overlap_count = 0;
    }
    Ok(file)
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

#[cfg(test)]
// Test files live in `tests/session_archive/` (mirror of src), pulled in by
// path so they keep unit-test semantics (private access). See docs/rust-test-files.md.
tests_bridge_macro::tests_bridge!("session_archive");
