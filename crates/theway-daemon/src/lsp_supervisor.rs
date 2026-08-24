//! LSP supervisor — owns multiple `LspClient` instances keyed by language id, lazily spawns
//! a server the first time a file of that language is touched, and exposes an
//! `after_tool_call` hook that attaches diagnostics to write/edit tool results.
//!
//! Closes the after-edit half of c4pt0r/theway#12.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use parking_lot::Mutex;
use serde::Deserialize;
use theway_core::{AfterToolCallContext, AfterToolCallHook, AfterToolCallResult};
use theway_llm_provider::UserContentBlock;
use tokio::sync::OnceCell;
use tokio_util::sync::CancellationToken;

use crate::lsp::{Diagnostic, LspClient};

const DIAG_WAIT_MS: u64 = 800;

#[derive(Debug, Default, Deserialize)]
pub struct LspConfig {
    #[serde(default)]
    pub language: Vec<LanguageConfig>,
}

#[derive(Debug, Deserialize)]
pub struct LanguageConfig {
    /// Language id (matches LSP "languageId", e.g. "rust", "typescript").
    pub id: String,
    /// File extensions this server handles (without the leading dot).
    pub extensions: Vec<String>,
    /// Command to spawn (e.g. "rust-analyzer").
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
}

pub struct LspSupervisor {
    cwd_uri: String,
    by_ext: HashMap<String, LanguageConfig>,
    clients: Mutex<HashMap<String, Arc<OnceCell<Arc<LspClient>>>>>,
    open_files: Mutex<HashMap<String, ()>>,
}

impl LspSupervisor {
    pub fn from_config(cwd: &Path, cfg: LspConfig) -> Self {
        let cwd_uri = format!("file://{}", cwd.display());
        let mut by_ext = HashMap::new();
        for lang in cfg.language {
            for ext in &lang.extensions {
                by_ext.insert(
                    ext.to_string(),
                    LanguageConfig {
                        id: lang.id.clone(),
                        extensions: lang.extensions.clone(),
                        command: lang.command.clone(),
                        args: lang.args.clone(),
                    },
                );
            }
        }
        Self {
            cwd_uri,
            by_ext,
            clients: Mutex::new(HashMap::new()),
            open_files: Mutex::new(HashMap::new()),
        }
    }

    /// Load `paths.work_dir/.theway/lsp.toml` and `paths.base/lsp.toml`;
    /// project entries overlay the user entries by language id.
    pub async fn load(paths: &crate::DaemonPaths) -> Self {
        let mut combined = LspConfig::default();
        for path in [
            paths.base.join("lsp.toml"),
            paths.work_dir.join(".theway").join("lsp.toml"),
        ] {
            if !path.exists() {
                continue;
            }
            let text = match tokio::fs::read_to_string(&path).await {
                Ok(t) => t,
                Err(_) => continue,
            };
            let cfg: LspConfig = match toml::from_str(&text) {
                Ok(c) => c,
                Err(_) => continue,
            };
            for lang in cfg.language {
                if let Some(idx) = combined.language.iter().position(|l| l.id == lang.id) {
                    combined.language[idx] = lang;
                } else {
                    combined.language.push(lang);
                }
            }
        }
        Self::from_config(&paths.work_dir, combined)
    }

    pub fn is_empty(&self) -> bool {
        self.by_ext.is_empty()
    }

    pub fn language_count(&self) -> usize {
        let unique: std::collections::HashSet<&str> =
            self.by_ext.values().map(|l| l.id.as_str()).collect();
        unique.len()
    }

    /// Lazily get or spawn the LSP client for `ext`. Caches per language id.
    async fn client_for_ext(&self, ext: &str) -> Option<Arc<LspClient>> {
        let lang = self.by_ext.get(ext)?;
        let cell = {
            let mut g = self.clients.lock();
            g.entry(lang.id.clone())
                .or_insert_with(|| Arc::new(OnceCell::new()))
                .clone()
        };
        let lang_clone = LanguageConfig {
            id: lang.id.clone(),
            extensions: lang.extensions.clone(),
            command: lang.command.clone(),
            args: lang.args.clone(),
        };
        let cwd_uri = self.cwd_uri.clone();
        let result: Result<Arc<LspClient>> = cell
            .get_or_try_init(|| async move {
                let args: Vec<&str> = lang_clone.args.iter().map(|s| s.as_str()).collect();
                let client = LspClient::spawn(&lang_clone.command, &args).await?;
                client.initialize(&cwd_uri).await?;
                Ok::<Arc<LspClient>, anyhow::Error>(Arc::new(client))
            })
            .await
            .cloned();
        result.ok()
    }

    /// Open the file in the relevant LSP if not already open. Returns the matching language
    /// id (for did_open's required `languageId`).
    async fn ensure_open(&self, path: &Path) -> Option<(Arc<LspClient>, String)> {
        let ext = path.extension().and_then(|e| e.to_str())?.to_string();
        let lang_id = self.by_ext.get(&ext)?.id.clone();
        let client = self.client_for_ext(&ext).await?;
        let uri = format!("file://{}", path.display());
        let already_open = self.open_files.lock().contains_key(&uri);
        if !already_open {
            let text = tokio::fs::read_to_string(path).await.ok()?;
            client.did_open(&uri, &lang_id, &text).await.ok()?;
            self.open_files.lock().insert(uri.clone(), ());
        }
        Some((client, lang_id))
    }
}

/// Build an `AfterToolCallHook` that attaches LSP diagnostics to write/edit tool results.
/// On non-edit tools, returns the result unmodified.
pub fn as_after_tool_call(supervisor: Arc<LspSupervisor>) -> AfterToolCallHook {
    let closure = move |ctx: AfterToolCallContext, cancel: CancellationToken| {
        let supervisor = supervisor.clone();
        let fut = attach_diagnostics(supervisor, ctx, cancel);
        Box::pin(fut) as Pin<Box<dyn std::future::Future<Output = AfterToolCallResult> + Send>>
    };
    Arc::new(closure)
}

#[allow(clippy::needless_pass_by_value)]
async fn attach_diagnostics(
    supervisor: Arc<LspSupervisor>,
    ctx: AfterToolCallContext,
    _cancel: CancellationToken,
) -> AfterToolCallResult {
    if supervisor.is_empty() {
        return AfterToolCallResult::default();
    }
    let tool_name = ctx.tool_call.name.as_str();
    if tool_name != "write" && tool_name != "edit" {
        return AfterToolCallResult::default();
    }
    let path = match ctx.args.get("path").and_then(|v| v.as_str()) {
        Some(p) => p.to_string(),
        None => return AfterToolCallResult::default(),
    };
    let path_buf = PathBuf::from(path);
    let Some((client, _lang)) = supervisor.ensure_open(&path_buf).await else {
        return AfterToolCallResult::default();
    };
    // Wait briefly for diagnostics to arrive after the edit. If the LSP doesn't push within
    // the timeout, fall back to whatever cached diagnostics it already has for the URI.
    let uri = format!("file://{}", path_buf.display());
    let _ = client
        .await_diagnostics(Duration::from_millis(DIAG_WAIT_MS))
        .await;
    let diags = client.diagnostics_for(&uri);
    if diags.is_empty() {
        return AfterToolCallResult::default();
    }
    let summary = render_diagnostics(&path_buf, &diags);
    let mut content = ctx.result.content.clone();
    content.push(UserContentBlock::text(summary));
    AfterToolCallResult {
        content: Some(content),
        details: None,
        is_error: None,
        terminate: None,
    }
}

fn render_diagnostics(path: &Path, diags: &[Diagnostic]) -> String {
    let mut out = format!("\n\nLSP diagnostics for {}:\n", path.display());
    for d in diags.iter().take(20) {
        let sev = match d.severity {
            Some(1) => "error",
            Some(2) => "warning",
            Some(3) => "info",
            Some(4) => "hint",
            _ => "diag",
        };
        out.push_str(&format!(
            "  [{sev}] {}:{}: {}\n",
            d.range.start.line + 1,
            d.range.start.character + 1,
            d.message
        ));
    }
    if diags.len() > 20 {
        out.push_str(&format!("  ({} more)\n", diags.len() - 20));
    }
    out
}

#[cfg(test)]
tests_bridge_macro::tests_bridge!("lsp_supervisor");
