//! `tgrep serve` process registry for the built-in grep tool (issue #121).
//!
//! The daemon lazily spawns one `tgrep serve <root>` per canonical project
//! root (binary discovered next to the daemon's own exe, then on PATH) and
//! answers the grep tool's readiness question. Queries only take the
//! index-accelerated client path once the server reports its initial index
//! build as complete (`status` JSON-RPC, `indexing == false`) — before that,
//! and whenever the binary is missing or the server dies, the grep tool
//! answers with its in-process walker so results are always complete.
//!
//! Servers are shared across sessions (tgrep serve is multi-client), LRU-
//! evicted beyond [`MAX_SERVERS`], and killed on daemon exit (registry drop).

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const MAX_SERVERS: usize = 3;
/// tgrep's on-disk index directory name (upstream constant, see vendored
/// `tgrep-core/src/builder.rs::INDEX_DIR_NAME`).
const INDEX_DIR_NAME: &str = ".tgrep";
/// Connect/read budget for one readiness poll. A poll that cannot determine
/// readiness leaves the entry in the indexing state.
const POLL_TIMEOUT: Duration = Duration::from_millis(500);

/// Readiness of the `tgrep serve` instance for a project root, from the
/// grep tool's point of view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TgrepReadiness {
    /// No usable binary or the root cannot be served; use the walker.
    Missing,
    /// A server exists but its initial index build is still running; use the
    /// walker (a serving server would answer partial-index results).
    Indexing,
    /// Index complete; queries may take the client path.
    Ready,
}

struct ServerEntry {
    child: Option<Child>,
    ready: bool,
    last_used: Instant,
}

struct RegistryInner {
    entries: HashMap<PathBuf, ServerEntry>,
}

/// Process-scoped registry of `tgrep serve` children.
///
/// Clones share the same state. All methods are synchronous and cheap; call
/// them from a blocking context (the grep tool's `spawn_blocking`).
#[derive(Clone)]
pub struct TgrepServerRegistry {
    inner: Arc<Mutex<RegistryInner>>,
    /// Explicit binary override (tests). `None` = resolve at spawn time.
    binary: Option<PathBuf>,
}

impl Default for TgrepServerRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl TgrepServerRegistry {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(RegistryInner {
                entries: HashMap::new(),
            })),
            binary: None,
        }
    }

    /// Test seam: pin the `tgrep` binary path instead of resolving it.
    pub fn with_binary(binary: PathBuf) -> Self {
        Self {
            inner: Arc::new(Mutex::new(RegistryInner {
                entries: HashMap::new(),
            })),
            binary: Some(binary),
        }
    }

    /// Resolve the `tgrep` binary: sibling of the current exe (installed
    /// side by side via install.sh / GitHub Release), then PATH.
    pub fn resolve_binary() -> Option<PathBuf> {
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                let name = if cfg!(windows) { "tgrep.exe" } else { "tgrep" };
                let sibling = dir.join(name);
                if sibling.is_file() {
                    return Some(sibling);
                }
            }
        }
        // Fall back to PATH resolution at spawn time: a bare "tgrep" argv[0]
        // is searched by the OS.
        Some(PathBuf::from(if cfg!(windows) { "tgrep.exe" } else { "tgrep" }))
    }

    fn binary(&self) -> Option<PathBuf> {
        self.binary.clone().or_else(Self::resolve_binary)
    }

    /// The resolved `tgrep` binary path (override or sibling/PATH discovery).
    /// Used by the grep tool to spawn client queries with the same resolution.
    pub fn binary_path(&self) -> Option<PathBuf> {
        self.binary()
    }

    /// Ensure a serve process for `root` and report its readiness. `root`
    /// is canonicalized; failures to canonicalize report [`TgrepReadiness::Missing`].
    pub fn query_root(&self, root: &Path) -> TgrepReadiness {
        let Ok(root) = root.canonicalize() else {
            return TgrepReadiness::Missing;
        };
        let mut inner = self.inner.lock().expect("tgrep registry poisoned");
        let now = Instant::now();
        if let Some(entry) = inner.entries.get_mut(&root) {
            entry.last_used = now;
            if entry.ready {
                return TgrepReadiness::Ready;
            }
            // A server that died before becoming ready gets reaped and
            // re-spawned; a dead Ready server is handled by the client's own
            // fallback plus a reap on the next poll attempt.
            if let Some(child) = entry.child.as_mut()
                && matches!(child.try_wait(), Ok(Some(_)))
            {
                inner.entries.remove(&root);
            } else if poll_status(&root) == Some(true) {
                entry.ready = true;
                return TgrepReadiness::Ready;
            } else {
                return TgrepReadiness::Indexing;
            }
        }

        let Some(binary) = self.binary() else {
            inner.entries.insert(root, missing_entry(now));
            return TgrepReadiness::Missing;
        };
        let spawn = spawn_serve(&binary, &root);
        match spawn {
            Ok(child) => {
                tracing::info!(target: "tgrep", root = %root.display(), pid = child.id(), "spawned tgrep serve");
                insert_and_evict(&mut inner.entries, root, ServerEntry {
                    child: Some(child),
                    ready: false,
                    last_used: now,
                });
                TgrepReadiness::Indexing
            }
            Err(err) => {
                tracing::warn!(target: "tgrep", root = %root.display(), binary = %binary.display(), error = %err, "failed to spawn tgrep serve; grep falls back to the walker");
                insert_and_evict(&mut inner.entries, root, missing_entry(now));
                TgrepReadiness::Missing
            }
        }
    }

    /// Number of tracked servers (diagnostics/tests).
    pub fn len(&self) -> usize {
        self.inner.lock().expect("tgrep registry poisoned").entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Drop for RegistryInner {
    fn drop(&mut self) {
        for (root, entry) in &mut self.entries {
            if let Some(child) = entry.child.as_mut() {
                // Best-effort: tgrep serve has no grandchildren, so a plain
                // kill reaches the whole tree; wait to reap the zombie.
                let _ = child.kill();
                let _ = child.wait();
                tracing::info!(target: "tgrep", root = %root.display(), "reaped tgrep serve at daemon exit");
            }
        }
    }
}

fn missing_entry(now: Instant) -> ServerEntry {
    ServerEntry { child: None, ready: false, last_used: now }
}

fn insert_and_evict(entries: &mut HashMap<PathBuf, ServerEntry>, root: PathBuf, entry: ServerEntry) {
    entries.insert(root, entry);
    if entries.len() > MAX_SERVERS {
        if let Some(lru) = entries
            .iter()
            .min_by_key(|(_, e)| e.last_used)
            .map(|(k, _)| k.clone())
        {
            if let Some(mut evicted) = entries.remove(&lru) {
                if let Some(child) = evicted.child.as_mut() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                tracing::info!(target: "tgrep", root = %lru.display(), "evicted tgrep serve (LRU cap {MAX_SERVERS})");
            }
        }
    }
}

/// Spawn `tgrep serve <root>` with detached stdio; stderr goes to the theway
/// logs dir (`<base>/logs/tgrep-serve-<slug>.log`) for post-mortems.
fn spawn_serve(binary: &Path, root: &Path) -> std::io::Result<Child> {
    let stderr = serve_log_file(root)
        .and_then(|log| {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(log)
                .ok()
        })
        .map(Stdio::from)
        .unwrap_or_else(Stdio::null);
    Command::new(binary)
        .arg("serve")
        .arg(root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(stderr)
        .spawn()
}

/// `<theway base>/logs/tgrep-serve-<slug>.log`; `None` when the base dir
/// cannot be determined or the logs dir cannot be created.
fn serve_log_file(root: &Path) -> Option<PathBuf> {
    let dir = theway_transport::client::base_dir().join("logs");
    std::fs::create_dir_all(&dir).ok()?;
    // Sluggify the root path into a stable, collision-resistant filename.
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    use std::hash::{Hash, Hasher};
    root.display().to_string().hash(&mut hasher);
    let mut slug = format!("{:016x}", hasher.finish());
    slug.truncate(12);
    Some(dir.join(format!("tgrep-serve-{slug}.log")))
}

/// Poll the server's `status` JSON-RPC for index completion.
/// `Some(true)` = index complete; `Some(false)` = server reachable but still
/// indexing; `None` = cannot determine (no serve.json / unreachable / malformed).
fn poll_status(root: &Path) -> Option<bool> {
    let info_path = root.join(INDEX_DIR_NAME).join("serve.json");
    let info: ServeInfo = serde_json::from_str(&std::fs::read_to_string(info_path).ok()?).ok()?;
    let addr = format!("127.0.0.1:{}", info.port);
    let mut stream = TcpStream::connect_timeout(
        &addr.parse().ok()?,
        POLL_TIMEOUT,
    )
    .ok()?;
    stream.set_read_timeout(Some(POLL_TIMEOUT)).ok()?;
    stream.set_write_timeout(Some(POLL_TIMEOUT)).ok()?;
    writeln!(
        stream,
        "{}",
        serde_json::json!({"jsonrpc": "2.0", "method": "status", "id": 1})
    )
    .ok()?;
    stream.flush().ok()?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).ok()?;
    let response: serde_json::Value = serde_json::from_str(&line).ok()?;
    response
        .get("result")?
        .get("indexing")?
        .as_bool()
        .map(|indexing| !indexing)
}

/// Mirrors tgrep's `serve.json` (upstream `tgrep-cli/src/serve.rs::ServerInfo`).
#[derive(serde::Deserialize)]
struct ServeInfo {
    #[allow(dead_code)]
    pid: u32,
    port: u16,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_binary_prefers_sibling() {
        // The test binary's sibling in the deps dir usually does not exist,
        // so this exercises the PATH fallback branch without asserting on the
        // environment.
        let resolved = TgrepServerRegistry::resolve_binary();
        assert!(resolved.is_some());
    }

    #[test]
    fn poll_status_returns_none_without_serve_json() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(poll_status(dir.path()), None);
    }

    #[test]
    fn missing_root_reports_missing() {
        let registry = TgrepServerRegistry::with_binary(PathBuf::from("tgrep"));
        let readiness = registry.query_root(Path::new("/nonexistent/tgrep-root-xyz"));
        assert_eq!(readiness, TgrepReadiness::Missing);
        assert_eq!(registry.len(), 0);
    }

    #[test]
    fn failed_spawn_is_cached_as_missing() {
        // A non-executable path fails to spawn; the registry must cache
        // Missing so it does not retry on every query.
        let registry = TgrepServerRegistry::with_binary(PathBuf::from("/bin/false/definitely-not-tgrep"));
        let dir = tempfile::tempdir().expect("tempdir");
        let readiness = registry.query_root(dir.path());
        assert_eq!(readiness, TgrepReadiness::Missing);
        assert_eq!(registry.len(), 1);
        // Second call must not re-spawn (still Missing, still one entry).
        assert_eq!(registry.query_root(dir.path()), TgrepReadiness::Missing);
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn serve_log_file_slugs_root() {
        let a = serve_log_file(Path::new("/tmp/proj-a")).expect("log file");
        let b = serve_log_file(Path::new("/tmp/proj-b")).expect("log file");
        assert!(a.file_name().expect("name").to_string_lossy().starts_with("tgrep-serve-"));
        assert_ne!(a, b);
    }
}
