//! In-memory collection of runtime sessions for tests and embedders that do not
//! need durable persistence.

use std::sync::{Arc, Mutex, MutexGuard};

use super::super::types::SessionError;
use super::memory_storage::MemorySessionStorage;
use super::session::Session;

pub struct MemorySessionRepo {
    sessions: Mutex<Vec<Session>>,
}

impl MemorySessionRepo {
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(Vec::new()),
        }
    }

    /// Lock the session list, recovering the guard when another thread panicked while holding
    /// it. The list is only ever rewritten wholesale (`push`, `*g = keep`), so a panic cannot
    /// leave it half-updated; recovering keeps `create` / `count` / `list` infallible, which is
    /// their declared contract — they have no error channel to report a poisoned lock through.
    fn lock(&self) -> MutexGuard<'_, Vec<Session>> {
        self.sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Create a new in-memory session and return it.
    pub fn create(&self) -> Session {
        let storage = Arc::new(MemorySessionStorage::new());
        let session = Session::new(storage as Arc<dyn super::session::SessionStorage>);
        self.lock().push(session.clone());
        session
    }

    pub fn count(&self) -> usize {
        self.lock().len()
    }

    pub fn list(&self) -> Vec<Session> {
        self.lock().clone()
    }

    pub async fn delete_by_id(&self, id: &str) -> Result<bool, SessionError> {
        let mut g = self.lock();
        let start_len = g.len();
        let mut keep: Vec<Session> = Vec::with_capacity(start_len);
        for s in g.drain(..) {
            let meta = s.storage().get_metadata_json().await?;
            let matches = meta.get("id").and_then(|v| v.as_str()) == Some(id);
            if !matches {
                keep.push(s);
            }
        }
        *g = keep;
        Ok(start_len != g.len())
    }
}

impl Default for MemorySessionRepo {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
tests_bridge_macro::tests_bridge!("agent/session/memory_repo");
