//! In-memory `SessionRepo`. 1:1 port of
//! `packages/agent/src/harness/session/memory-repo.ts` (~50 lines). Holds a `Vec<Session>`
//! created from `MemorySessionStorage`. Used by tests and any embedder that doesn't want to
//! touch disk.

use std::sync::{Arc, Mutex};

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

    /// Create a new in-memory session and return it.
    pub fn create(&self) -> Session {
        let storage = Arc::new(MemorySessionStorage::new());
        let session = Session::new(storage as Arc<dyn super::session::SessionStorage>);
        self.sessions.lock().unwrap().push(session.clone());
        session
    }

    pub fn count(&self) -> usize {
        self.sessions.lock().unwrap().len()
    }

    pub fn list(&self) -> Vec<Session> {
        self.sessions.lock().unwrap().clone()
    }

    pub async fn delete_by_id(&self, id: &str) -> Result<bool, SessionError> {
        let mut g = self.sessions.lock().unwrap();
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
