//! `/bug-report` builder. Writes a single text dump to
//! `~/.theway/bug-reports/<utc-stamp>.txt` containing:
//!
//! 1. Diagnostic snapshot (model / thinking / tools / cost).
//! 2. Tail of the active session log file (up to 200 lines).
//! 3. The session transcript (rendered via `crate::export::render`).
//!
//! Everything goes through a redactor that strips well-known secret patterns. Bug reports are
//! the canonical "give me something to attach to an issue" artifact, so we trade detail for
//! safety: the redactor is conservative.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Utc;
use theway_core::Session;

// Re-exported: daemon modules reach the redactor through `crate::bug_report::redact`.
pub use theway::bug_report::redact;

use theway::config::base_dir;

const MAX_LOG_LINES: usize = 200;

pub fn default_dest() -> PathBuf {
    let stamp = Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    base_dir().join("bug-reports").join(format!("{stamp}.txt"))
}

/// Snapshot of harness state that lives outside the harness itself. The caller fills it in
/// from CommandCtx so this module stays decoupled from the slash-command layer.
pub struct DiagInputs {
    pub session_id: String,
    pub model: Option<String>,
    pub thinking: String,
    pub tool_count: usize,
    pub skill_count: usize,
    pub cost_summary: String,
    pub log_path: Option<PathBuf>,
}

pub async fn build(diag: DiagInputs, session: &Session, dest: &Path) -> Result<PathBuf> {
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("create bug-reports dir {}", parent.display()))?;
    }

    let mut body = String::new();
    body.push_str("theway bug report\n");
    body.push_str(&format!("generated_at: {}\n", Utc::now().to_rfc3339()));
    body.push_str(&format!("theway_version: {}\n", env!("CARGO_PKG_VERSION")));
    body.push('\n');

    body.push_str("---- diagnostic ----\n");
    body.push_str(&format!("session_id    {}\n", diag.session_id));
    body.push_str(&format!(
        "model         {}\n",
        diag.model.as_deref().unwrap_or("(none)")
    ));
    body.push_str(&format!("thinking      {}\n", diag.thinking));
    body.push_str(&format!("tools         {}\n", diag.tool_count));
    body.push_str(&format!("skills        {}\n", diag.skill_count));
    body.push_str(&format!("cost          {}\n", diag.cost_summary));
    body.push_str(&format!(
        "log_path      {}\n",
        diag.log_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(disabled)".into())
    ));
    body.push('\n');

    if let Some(log) = diag.log_path.as_ref() {
        body.push_str(&format!(
            "---- log tail ({} lines from {}) ----\n",
            MAX_LOG_LINES,
            log.display()
        ));
        match tokio::fs::read_to_string(log).await {
            Ok(text) => {
                let lines: Vec<&str> = text.lines().collect();
                let tail = if lines.len() > MAX_LOG_LINES {
                    &lines[lines.len() - MAX_LOG_LINES..]
                } else {
                    &lines[..]
                };
                for line in tail {
                    body.push_str(line);
                    body.push('\n');
                }
            }
            Err(e) => {
                body.push_str(&format!("(cannot read log: {e})\n"));
            }
        }
        body.push('\n');
    }

    body.push_str("---- transcript ----\n");
    match crate::export::render(session).await {
        Ok(transcript) => body.push_str(&transcript),
        Err(e) => body.push_str(&format!("(cannot render transcript: {e})\n")),
    }

    let redacted = redact(&body);
    tokio::fs::write(dest, redacted)
        .await
        .with_context(|| format!("write {}", dest.display()))?;
    Ok(dest.to_path_buf())
}

// The secret redactor (`redact`) lives in the `theway` SDK
// (`theway::bug_report::redact`) so both the daemon and client crates share one
// conservative implementation; imported above.
