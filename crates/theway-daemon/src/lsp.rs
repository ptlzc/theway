//! Minimal LSP client. v1 scope: subprocess transport with Content-Length framing,
//! `initialize` handshake, `textDocument/didOpen`, async collection of
//! `textDocument/publishDiagnostics` notifications. Wires into the agent's after-edit hook
//! in a follow-up.
//!
//! Per-language server resolution is config-driven (`~/.theway/lsp.toml`). v1 supports stdio
//! servers only; SSE/socket transports defer. Public API is `#[allow(dead_code)]` at the
//! item level so the binary compiles before the after-edit-hook wiring lands.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::Mutex as AsyncMutex;
use tokio::sync::mpsc;
use tokio::sync::oneshot;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub range: DiagnosticRange,
    #[serde(default)]
    pub severity: Option<u8>,
    pub message: String,
    #[serde(default)]
    pub source: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct DiagnosticRange {
    pub start: Position,
    pub end: Position,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

#[derive(Debug, Clone, Deserialize)]
struct PublishDiagnosticsParams {
    uri: String,
    diagnostics: Vec<Diagnostic>,
}

#[allow(dead_code)]
pub struct LspClient {
    stdin: AsyncMutex<ChildStdin>,
    next_id: AtomicU64,
    inflight: Arc<Mutex<HashMap<u64, oneshot::Sender<serde_json::Value>>>>,
    diagnostics: Arc<Mutex<HashMap<String, Vec<Diagnostic>>>>,
    diag_rx: AsyncMutex<mpsc::UnboundedReceiver<(String, Vec<Diagnostic>)>>,
    child: AsyncMutex<Option<Child>>,
    request_timeout: Duration,
}

#[allow(dead_code)]
impl LspClient {
    /// Spawn the server, wire stdio, and start the read pump. Caller must then call
    /// [`Self::initialize`] before any other method.
    pub async fn spawn(cmd: &str, args: &[&str]) -> Result<Self> {
        let mut child = Command::new(cmd)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("spawn LSP server {cmd}"))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("LSP child has no stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("LSP child has no stdout"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow!("LSP child has no stderr"))?;

        let inflight: Arc<Mutex<HashMap<u64, oneshot::Sender<serde_json::Value>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let diagnostics: Arc<Mutex<HashMap<String, Vec<Diagnostic>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let (diagnostics_tx, diag_rx) = mpsc::unbounded_channel::<(String, Vec<Diagnostic>)>();

        // Read pump.
        let pump_inflight = inflight.clone();
        let pump_diagnostics = diagnostics.clone();
        tokio::spawn(async move {
            let mut reader = tokio::io::BufReader::new(stdout);
            loop {
                match read_framed(&mut reader).await {
                    Ok(Some(value)) => {
                        let id = value.get("id").and_then(|v| v.as_u64());
                        let method = value
                            .get("method")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());
                        if let Some(id) = id {
                            // Either a response or a server-initiated request (we don't
                            // handle the latter in v1 — ignore).
                            if value.get("method").is_none() {
                                if let Some(tx) = pump_inflight.lock().remove(&id) {
                                    let _ = tx.send(value);
                                }
                            }
                        } else if let Some(method) = method {
                            if method == "textDocument/publishDiagnostics" {
                                if let Some(params) = value.get("params") {
                                    if let Ok(p) = serde_json::from_value::<PublishDiagnosticsParams>(
                                        params.clone(),
                                    ) {
                                        pump_diagnostics
                                            .lock()
                                            .insert(p.uri.clone(), p.diagnostics.clone());
                                        let _ = diagnostics_tx.send((p.uri, p.diagnostics));
                                    }
                                }
                            }
                        }
                    }
                    Ok(None) => break,
                    Err(_) => break,
                }
            }
        });

        // Stderr drain.
        tokio::spawn(async move {
            let mut buf = [0u8; 1024];
            let mut stderr = stderr;
            while let Ok(n) = stderr.read(&mut buf).await {
                if n == 0 {
                    break;
                }
            }
        });

        Ok(Self {
            stdin: AsyncMutex::new(stdin),
            next_id: AtomicU64::new(1),
            inflight,
            diagnostics,
            diag_rx: AsyncMutex::new(diag_rx),
            child: AsyncMutex::new(Some(child)),
            request_timeout: Duration::from_secs(15),
        })
    }

    /// Send the initialize request and the matching `initialized` notification.
    pub async fn initialize(&self, root_uri: &str) -> Result<serde_json::Value> {
        let params = serde_json::json!({
            "processId": std::process::id(),
            "rootUri": root_uri,
            "capabilities": {
                "textDocument": {
                    "synchronization": { "didSave": true },
                    "publishDiagnostics": {}
                }
            },
            "clientInfo": { "name": "theway", "version": env!("CARGO_PKG_VERSION") }
        });
        let result = self.request("initialize", Some(params)).await?;
        // Notify the server we're ready.
        self.notify("initialized", Some(serde_json::json!({})))
            .await?;
        Ok(result)
    }

    /// Send a textDocument/didOpen notification.
    pub async fn did_open(&self, uri: &str, language_id: &str, text: &str) -> Result<()> {
        let params = serde_json::json!({
            "textDocument": {
                "uri": uri,
                "languageId": language_id,
                "version": 1,
                "text": text
            }
        });
        self.notify("textDocument/didOpen", Some(params)).await
    }

    /// Return the most recent diagnostics for `uri` (empty if none received yet).
    pub fn diagnostics_for(&self, uri: &str) -> Vec<Diagnostic> {
        self.diagnostics
            .lock()
            .get(uri)
            .cloned()
            .unwrap_or_default()
    }

    /// Wait up to `timeout` for the next diagnostics push (regardless of URI). Returns the
    /// uri + diagnostics, or None on timeout.
    pub async fn await_diagnostics(&self, timeout: Duration) -> Option<(String, Vec<Diagnostic>)> {
        let mut rx = self.diag_rx.lock().await;
        match tokio::time::timeout(timeout, rx.recv()).await {
            Ok(Some(pair)) => Some(pair),
            _ => None,
        }
    }

    pub async fn shutdown(&self) {
        // Best-effort shutdown — let the server clean up; if it doesn't respond, kill.
        let _ = self
            .request("shutdown", Some(serde_json::Value::Null))
            .await;
        let _ = self.notify("exit", None).await;
        if let Some(mut child) = self.child.lock().await.take() {
            let _ = child.start_kill();
        }
    }

    async fn request(
        &self,
        method: &'static str,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let line = serde_json::to_string(&req)?;
        let (tx, rx) = oneshot::channel();
        self.inflight.lock().insert(id, tx);
        self.write_framed(&line).await?;
        let resp = tokio::time::timeout(self.request_timeout, rx).await;
        match resp {
            Ok(Ok(value)) => {
                if let Some(err) = value.get("error") {
                    return Err(anyhow!("LSP server error: {err}"));
                }
                Ok(value
                    .get("result")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null))
            }
            Ok(Err(_)) => Err(anyhow!("LSP response channel closed")),
            Err(_) => {
                self.inflight.lock().remove(&id);
                Err(anyhow!(
                    "LSP request {method} timed out after {:?}",
                    self.request_timeout
                ))
            }
        }
    }

    async fn notify(&self, method: &'static str, params: Option<serde_json::Value>) -> Result<()> {
        let req = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        let line = serde_json::to_string(&req)?;
        self.write_framed(&line).await
    }

    async fn write_framed(&self, payload: &str) -> Result<()> {
        let mut s = self.stdin.lock().await;
        let header = format!("Content-Length: {}\r\n\r\n", payload.len());
        s.write_all(header.as_bytes()).await?;
        s.write_all(payload.as_bytes()).await?;
        s.flush().await?;
        Ok(())
    }
}

async fn read_framed<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut R,
) -> Result<Option<serde_json::Value>> {
    // Read headers until empty line.
    let mut content_length: Option<usize> = None;
    let mut header_line = String::new();
    loop {
        header_line.clear();
        let n = tokio::io::AsyncBufReadExt::read_line(reader, &mut header_line).await?;
        if n == 0 {
            return Ok(None);
        }
        let trimmed = header_line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some(rest) = trimmed.strip_prefix("Content-Length: ") {
            content_length = rest.parse().ok();
        }
    }
    let len = content_length.ok_or_else(|| anyhow!("LSP frame missing Content-Length"))?;
    let mut buf = vec![0u8; len];
    tokio::io::AsyncReadExt::read_exact(reader, &mut buf).await?;
    let value = serde_json::from_slice(&buf)?;
    Ok(Some(value))
}
