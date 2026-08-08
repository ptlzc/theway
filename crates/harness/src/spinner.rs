//! Active-prompt spinner. Animates a braille frame to stderr while the agent is between
//! "user submitted" and "LLM is producing content" — then clears its line and exits the
//! moment the first content delta arrives.
//!
//! Design points discovered the hard way:
//!
//! 1. **Render frame 0 synchronously inside `start()`.** `tokio::spawn` only queues the
//!    task; the executor doesn't run it until the current task yields. The agent loop's
//!    early `emit()` calls run listeners inline, and the listener's `stop_sync()` would
//!    beat the spawned task's first draw. We render frame 0 on the caller's thread.
//!
//! 2. **`stop_sync()` flips the flag AND clears on the caller's thread.** The animation
//!    task only checks the flag at frame boundaries (80ms); cleanup there would race
//!    with renderer stdout writes.
//!
//! 3. **Only the original handle auto-stops on drop.** Listener code clones the handle for
//!    each event; those short-lived clones must be able to call `stop_sync()` explicitly
//!    without stopping the spinner merely by going out of scope.
//!
//! 4. **Stop on first content delta, not on `AgentStart`.** The caller wires this via
//!    [`should_stop_spinner_on`] in `main.rs`.
//!
//! Tests inject their own `SpinnerSink` so we can observe the exact byte stream the
//! spinner emits, without touching the process's real stderr.

use std::io::IsTerminal;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use parking_lot::Mutex;

const FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const FRAME_MS: u64 = 80;

/// Where the spinner writes. Production wraps stderr; tests wrap a `Vec<u8>` so the byte
/// stream is observable.
pub trait SpinnerSink: Send + Sync {
    fn write(&self, bytes: &[u8]);
}

struct StderrSink;
impl SpinnerSink for StderrSink {
    fn write(&self, bytes: &[u8]) {
        use std::io::Write;
        let mut err = std::io::stderr();
        let _ = err.write_all(bytes);
        let _ = err.flush();
    }
}

/// Test sink that appends every write to an in-memory buffer. Public-but-test-only so
/// the integration tests under `tests/spinner_e2e.rs` can use it via path-include.
#[derive(Default, Clone)]
#[allow(dead_code)]
pub struct BufferSink {
    pub buf: Arc<Mutex<Vec<u8>>>,
}

#[allow(dead_code)]
impl BufferSink {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn snapshot(&self) -> Vec<u8> {
        self.buf.lock().clone()
    }
    pub fn as_string(&self) -> String {
        String::from_utf8_lossy(&self.snapshot()).into_owned()
    }
}

impl SpinnerSink for BufferSink {
    fn write(&self, bytes: &[u8]) {
        self.buf.lock().extend_from_slice(bytes);
    }
}

pub struct SpinnerHandle {
    stop: Arc<AtomicBool>,
    sink: Arc<dyn SpinnerSink>,
    enabled: bool,
    stop_on_drop: bool,
}

impl Clone for SpinnerHandle {
    fn clone(&self) -> Self {
        Self {
            stop: self.stop.clone(),
            sink: self.sink.clone(),
            enabled: self.enabled,
            stop_on_drop: false,
        }
    }
}

impl SpinnerHandle {
    /// Idempotent stop. Flips the flag, then synchronously emits the line-clear escape so
    /// any subsequent write by the caller lands on a clean line.
    pub fn stop_sync(&self) {
        if self.stop.swap(true, Ordering::SeqCst) {
            return;
        }
        if self.enabled {
            self.sink.write(b"\r\x1b[2K");
        }
    }
}

impl Drop for SpinnerHandle {
    fn drop(&mut self) {
        if self.stop_on_drop {
            self.stop_sync();
        }
    }
}

fn draw_frame(sink: &Arc<dyn SpinnerSink>, idx: usize, label: &str) {
    let icon = FRAMES[idx % FRAMES.len()];
    let line = format!("\r\x1b[2K{icon} {label}");
    sink.write(line.as_bytes());
}

/// Production entry point — writes to stderr when stderr is a TTY, no-ops on pipes/CI.
pub fn start(label: impl Into<String>) -> SpinnerHandle {
    let enabled = std::io::stderr().is_terminal();
    start_with(label, Arc::new(StderrSink) as Arc<dyn SpinnerSink>, enabled)
}

/// Test entry point — inject a custom sink + force-enable.
#[allow(dead_code)]
pub fn start_with(
    label: impl Into<String>,
    sink: Arc<dyn SpinnerSink>,
    enabled: bool,
) -> SpinnerHandle {
    let stop = Arc::new(AtomicBool::new(false));
    if !enabled {
        return SpinnerHandle {
            stop,
            sink,
            enabled,
            stop_on_drop: true,
        };
    }
    let label = label.into();
    // Render frame 0 right now — no waiting for the executor to pick up the spawned task.
    draw_frame(&sink, 0, &label);

    let stop_clone = stop.clone();
    let sink_for_task = sink.clone();
    let frame_counter = Arc::new(AtomicUsize::new(1));
    let label_for_task = label;
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(FRAME_MS)).await;
            if stop_clone.load(Ordering::SeqCst) {
                break;
            }
            let idx = frame_counter.fetch_add(1, Ordering::SeqCst);
            draw_frame(&sink_for_task, idx, &label_for_task);
        }
    });
    SpinnerHandle {
        stop,
        sink,
        enabled,
        stop_on_drop: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn first_frame_renders_synchronously_in_start() {
        let sink = BufferSink::new();
        let _h = start_with("thinking", Arc::new(sink.clone()), true);
        // No await between start() and snapshot — frame 0 MUST already be there.
        let body = sink.as_string();
        assert!(
            body.contains("⠋"),
            "frame 0 missing from synchronous render: {body:?}"
        );
        assert!(body.contains("thinking"), "label missing: {body:?}");
    }

    #[tokio::test]
    async fn spinner_animates_multiple_frames_over_time() {
        let sink = BufferSink::new();
        let h = start_with("thinking", Arc::new(sink.clone()), true);
        // Let the animation task drive 3-4 frames.
        tokio::time::sleep(Duration::from_millis(300)).await;
        h.stop_sync();
        let body = sink.as_string();
        // At least two distinct frame glyphs should have been emitted.
        let frame_chars = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        let distinct: usize = frame_chars.iter().filter(|f| body.contains(*f)).count();
        assert!(
            distinct >= 2,
            "expected ≥2 distinct frames, got {distinct} in {body:?}"
        );
    }

    #[tokio::test]
    async fn stop_sync_emits_line_clear_immediately() {
        let sink = BufferSink::new();
        let h = start_with("thinking", Arc::new(sink.clone()), true);
        // Snapshot before stop.
        let before = sink.as_string();
        h.stop_sync();
        let after = sink.as_string();
        let cleared = &after[before.len()..];
        assert_eq!(
            cleared, "\r\x1b[2K",
            "stop_sync must emit clear escape immediately, got {cleared:?}"
        );
    }

    #[tokio::test]
    async fn stop_sync_is_idempotent() {
        let sink = BufferSink::new();
        let h = start_with("thinking", Arc::new(sink.clone()), true);
        h.stop_sync();
        let after_first = sink.as_string().len();
        h.stop_sync();
        let after_second = sink.as_string().len();
        assert_eq!(
            after_first, after_second,
            "second stop_sync must be a no-op"
        );
    }

    #[tokio::test]
    async fn dropping_clone_does_not_stop_owner() {
        let sink = BufferSink::new();
        let h = start_with("thinking", Arc::new(sink.clone()), true);
        drop(h.clone());
        let before = sink.as_string().len();
        tokio::time::sleep(Duration::from_millis(120)).await;
        let after = sink.as_string().len();
        h.stop_sync();
        assert!(
            after > before,
            "dropping a temporary clone must not stop the spinner"
        );
    }

    #[tokio::test]
    async fn disabled_spinner_writes_nothing() {
        let sink = BufferSink::new();
        let h = start_with("thinking", Arc::new(sink.clone()), false);
        tokio::time::sleep(Duration::from_millis(200)).await;
        h.stop_sync();
        assert_eq!(sink.snapshot().len(), 0, "disabled spinner must be silent");
    }
}
