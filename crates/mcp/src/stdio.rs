//! Stdio transport. Spawns a subprocess, talks JSON-RPC over its stdin/stdout, captures
//! stderr to a buffered log accessor for diagnostics.

use std::process::Stdio;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::Mutex as AsyncMutex;

use crate::errors::McpError;
use crate::transport::Transport;

/// Builder for spawning an MCP server subprocess.
pub struct StdioTransport {
    stdin: AsyncMutex<ChildStdin>,
    rx: AsyncMutex<tokio::sync::mpsc::Receiver<Result<String, McpError>>>,
    child: AsyncMutex<Option<Child>>,
    #[allow(dead_code)]
    stderr_tail: Arc<Mutex<Vec<String>>>,
}

impl StdioTransport {
    /// Spawn `cmd` with `args` and connect stdio. Returns once the child is launched (the
    /// initialize handshake is the caller's responsibility).
    pub async fn spawn(cmd: &str, args: &[&str]) -> Result<Self, McpError> {
        let mut child = Command::new(cmd)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| McpError::Transport(format!("spawn {cmd}: {e}")))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| McpError::Transport("child has no stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| McpError::Transport("child has no stdout".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| McpError::Transport("child has no stderr".into()))?;

        let (tx, rx) = tokio::sync::mpsc::channel::<Result<String, McpError>>(64);
        // stdout reader.
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) => {
                        if tx.send(Ok(line)).await.is_err() {
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(e) => {
                        let _ = tx.send(Err(McpError::Transport(e.to_string()))).await;
                        break;
                    }
                }
            }
        });

        // stderr drain (keep tail for diagnostics).
        let stderr_tail = Arc::new(Mutex::new(Vec::<String>::new()));
        let st = stderr_tail.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let mut g = st.lock();
                if g.len() >= 200 {
                    g.remove(0);
                }
                g.push(line);
            }
        });

        Ok(Self {
            stdin: AsyncMutex::new(stdin),
            rx: AsyncMutex::new(rx),
            child: AsyncMutex::new(Some(child)),
            stderr_tail,
        })
    }
}

#[async_trait]
impl Transport for StdioTransport {
    async fn send_line(&self, mut line: String) -> Result<(), McpError> {
        if !line.ends_with('\n') {
            line.push('\n');
        }
        let mut s = self.stdin.lock().await;
        s.write_all(line.as_bytes())
            .await
            .map_err(|e| McpError::Transport(e.to_string()))?;
        s.flush()
            .await
            .map_err(|e| McpError::Transport(e.to_string()))?;
        Ok(())
    }

    async fn recv_line(&self) -> Result<Option<String>, McpError> {
        let mut rx = self.rx.lock().await;
        match rx.recv().await {
            Some(Ok(line)) => Ok(Some(line)),
            Some(Err(e)) => Err(e),
            None => Ok(None),
        }
    }

    async fn close(&self) {
        if let Some(mut child) = self.child.lock().await.take() {
            // Best-effort: send SIGKILL after a tiny grace period; the subprocess sees its
            // stdin close anyway when we drop the half.
            let _ = child.kill().await;
        }
    }
}
