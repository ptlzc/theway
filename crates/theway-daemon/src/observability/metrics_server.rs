use std::net::SocketAddr;
use std::time::Duration;

use prometheus::Encoder as _;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

pub(super) struct MetricsServer {
    #[cfg(test)]
    pub(super) addr: SocketAddr,
    stop: tokio::sync::oneshot::Sender<()>,
    task: tokio::task::JoinHandle<()>,
}

impl MetricsServer {
    pub(super) async fn spawn(addr: SocketAddr, registry: prometheus::Registry) -> Option<Self> {
        let listener = match tokio::net::TcpListener::bind(addr).await {
            Ok(listener) => listener,
            Err(error) => {
                tracing::warn!(
                    target: "theway::observability",
                    %addr,
                    %error,
                    "Prometheus listener is disabled"
                );
                return None;
            }
        };
        let addr = listener.local_addr().ok()?;
        let (stop, mut stopped) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut stopped => break,
                    accepted = listener.accept() => {
                        let Ok((stream, _)) = accepted else { continue };
                        let registry = registry.clone();
                        tokio::spawn(async move { serve_metrics(stream, registry).await; });
                    }
                }
            }
        });
        tracing::info!(target: "theway::observability", %addr, "Prometheus listener started");
        Some(Self {
            #[cfg(test)]
            addr,
            stop,
            task,
        })
    }

    pub(super) async fn shutdown(self) {
        let _ = self.stop.send(());
        let _ = tokio::time::timeout(Duration::from_secs(2), self.task).await;
    }
}

async fn serve_metrics(mut stream: tokio::net::TcpStream, registry: prometheus::Registry) {
    let mut request = [0_u8; 2_048];
    let Ok(read) = stream.read(&mut request).await else {
        return;
    };
    let metrics_path = request[..read].starts_with(b"GET /metrics ");
    let (status, content_type, body) = if metrics_path {
        let encoder = prometheus::TextEncoder::new();
        let mut body = Vec::new();
        if encoder.encode(&registry.gather(), &mut body).is_err() {
            return;
        }
        ("200 OK", encoder.format_type().to_string(), body)
    } else {
        (
            "404 Not Found",
            "text/plain; charset=utf-8".to_string(),
            b"not found\n".to_vec(),
        )
    };
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes()).await;
    let _ = stream.write_all(&body).await;
    let _ = stream.shutdown().await;
}
