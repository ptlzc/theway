//! Tracing subscriber bootstrap for the CLI.
//!
//! Writes structured logs to `~/.theway/logs/<session-id>.log` via tracing-appender's
//! non-blocking writer. Default filter is `info` with `RUST_LOG` override (env_filter syntax,
//! same as ripgrep/tokio). The returned `WorkerGuard` MUST be kept alive for the lifetime of
//! the process — dropping it stops the background writer and queued events get lost.
//!
//! The init function is intentionally tolerant: any IO failure here logs to stderr and
//! returns `None` rather than blowing up the CLI. Logging is observability, not load-bearing.

use std::path::{Path, PathBuf};

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

use theway_transport::client::base_dir;

pub struct LoggingHandle {
    /// Background-writer guard. Drops on process exit.
    _guard: WorkerGuard,
    /// Path written for this session — surfaced by `/diag`.
    pub log_path: PathBuf,
}

/// Install a tracing subscriber tied to the supplied session id. Idempotent: if a subscriber
/// is already set (e.g. by a test harness), the function returns `None`.
pub fn init(session_id: &str) -> Option<LoggingHandle> {
    let dir = base_dir().join("logs");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("(logging disabled: cannot create {}: {e})", dir.display());
        return None;
    }
    let filename = format!("{}.log", short(session_id));
    let log_path = dir.join(&filename);

    // Non-blocking appender keeps tracing off the hot path. The worker thread streams to disk
    // in batches; pending events are flushed when the guard drops.
    let file = match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
    {
        Ok(f) => f,
        Err(e) => {
            eprintln!(
                "(logging disabled: cannot open {}: {e})",
                log_path.display()
            );
            return None;
        }
    };
    let (writer, guard) = tracing_appender::non_blocking(file);

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let layer = fmt::layer()
        .with_ansi(false)
        .with_writer(writer)
        .with_target(true)
        .with_thread_ids(true)
        .with_span_events(fmt::format::FmtSpan::CLOSE);

    // Optional OTLP layer (issue #15). Activates only when OTEL_EXPORTER_OTLP_ENDPOINT is set
    // — silent no-op otherwise.
    let otlp = crate::otlp::try_layer();
    let registry = tracing_subscriber::registry().with(filter).with(layer);
    let res = if let Some(otlp_layer) = otlp {
        registry.with(otlp_layer).try_init()
    } else {
        registry.try_init()
    };
    if res.is_err() {
        // Another subscriber is already installed (tests usually). Bail silently.
        return None;
    }

    Some(LoggingHandle {
        _guard: guard,
        log_path,
    })
}

/// Used in the log filename: keep just enough of the UUIDv7 to disambiguate within a day.
fn short(session_id: &str) -> &str {
    let cap = session_id.len().min(16);
    &session_id[..cap]
}

#[allow(dead_code)]
fn _path_check(_p: &Path) {}

#[cfg(test)]
tests_bridge_macro::tests_bridge!("logging");
