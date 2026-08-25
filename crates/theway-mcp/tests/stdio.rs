//! Stdio transport spawn tests. `pwd` is Unix-only; the whole module is gated.

#[cfg(unix)]
mod tests {
    use std::path::PathBuf;

    use theway_mcp::{StdioTransport, Transport};

    #[tokio::test]
    async fn spawn_in_uses_explicit_cwd() {
        let cwd = std::env::temp_dir().join(format!("theway-mcp-stdio-{}", std::process::id()));
        std::fs::create_dir_all(&cwd).unwrap();
        let cwd = cwd.canonicalize().unwrap();
        let transport = StdioTransport::spawn_in("pwd", &[], &cwd).await.unwrap();
        let line = transport.recv_line().await.unwrap().unwrap();
        assert_eq!(PathBuf::from(line), cwd);
        transport.close().await;
    }

    #[tokio::test]
    async fn spawn_keeps_inherited_cwd() {
        let transport = StdioTransport::spawn("pwd", &[]).await.unwrap();
        let line = transport.recv_line().await.unwrap().unwrap();
        assert_eq!(PathBuf::from(line), std::env::current_dir().unwrap());
        transport.close().await;
    }
}
