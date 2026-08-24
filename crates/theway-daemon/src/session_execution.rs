//! Session-scoped safe bindings and zeroizing provider credentials.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;
use theway_contract::session::SessionBinding;
use zeroize::Zeroizing;

pub(crate) struct SecretBytes(Zeroizing<Vec<u8>>);

impl SecretBytes {
    pub(crate) fn new(bytes: Vec<u8>) -> Self {
        Self(Zeroizing::new(bytes))
    }

    pub(crate) fn into_zeroizing(self) -> Zeroizing<Vec<u8>> {
        self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum RegistryError {
    #[error("work directory does not exist: {0}")]
    WorkDirMissing(PathBuf),
    #[error("work directory is not a directory: {0}")]
    WorkDirNotDirectory(PathBuf),
    #[error("client key must not be empty")]
    EmptyClientKey,
    #[error(
        "session {session_id} is already bound to client key {client_key:?}; cannot rebind to {requested_client_key:?}"
    )]
    SessionClientKeyConflict {
        session_id: String,
        client_key: String,
        requested_client_key: String,
    },
    #[error(
        "session {session_id} is already bound to work directory {work_dir}; cannot rebind to {requested_work_dir}"
    )]
    SessionWorkDirConflict {
        session_id: String,
        work_dir: String,
        requested_work_dir: String,
    },
    #[error(
        "client key {client_key:?} is already bound to session {existing_session_id} in {work_dir}"
    )]
    ClientKeyCwdConflict {
        client_key: String,
        existing_session_id: String,
        work_dir: String,
    },
    #[error("session {0} is not registered")]
    SessionNotRegistered(String),
}

struct Entry {
    binding: SessionBinding,
    credentials: HashMap<String, SecretBytes>,
}

#[derive(Clone, Default)]
pub(crate) struct SessionExecutionRegistry {
    inner: Arc<Mutex<HashMap<String, Entry>>>,
}

impl SessionExecutionRegistry {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn set(
        &self,
        session_id: impl Into<String>,
        mut binding: SessionBinding,
    ) -> Result<(), RegistryError> {
        if binding.client_key.trim().is_empty() {
            return Err(RegistryError::EmptyClientKey);
        }

        let requested_work_dir = binding.runtime.work_dir.clone();
        let work_dir = Path::new(&requested_work_dir);
        let canonical = std::fs::canonicalize(work_dir)
            .map_err(|_| RegistryError::WorkDirMissing(work_dir.to_path_buf()))?;
        if !canonical.is_dir() {
            return Err(RegistryError::WorkDirNotDirectory(canonical));
        }
        binding.runtime.work_dir = canonical.to_string_lossy().into_owned();

        let session_id = session_id.into();
        let mut inner = self.inner.lock();
        if let Some(existing) = inner.get(&session_id) {
            if existing.binding.client_key != binding.client_key {
                return Err(RegistryError::SessionClientKeyConflict {
                    session_id: session_id.clone(),
                    client_key: existing.binding.client_key.clone(),
                    requested_client_key: binding.client_key,
                });
            }
            if existing.binding.runtime.work_dir != binding.runtime.work_dir {
                return Err(RegistryError::SessionWorkDirConflict {
                    session_id: session_id.clone(),
                    work_dir: existing.binding.runtime.work_dir.clone(),
                    requested_work_dir: binding.runtime.work_dir,
                });
            }
        }

        for (existing_session_id, existing) in inner.iter() {
            if existing_session_id != &session_id
                && existing.binding.client_key == binding.client_key
                && existing.binding.runtime.work_dir == binding.runtime.work_dir
            {
                return Err(RegistryError::ClientKeyCwdConflict {
                    client_key: binding.client_key,
                    existing_session_id: existing_session_id.clone(),
                    work_dir: binding.runtime.work_dir,
                });
            }
        }

        match inner.get_mut(&session_id) {
            Some(entry) => entry.binding = binding,
            None => {
                inner.insert(
                    session_id,
                    Entry {
                        binding,
                        credentials: HashMap::new(),
                    },
                );
            }
        }
        Ok(())
    }

    pub(crate) fn get(&self, session_id: &str) -> Option<SessionBinding> {
        self.inner
            .lock()
            .get(session_id)
            .map(|entry| entry.binding.clone())
    }

    pub(crate) fn remove(&self, session_id: &str) -> bool {
        self.inner.lock().remove(session_id).is_some()
    }

    pub(crate) fn set_credential(
        &self,
        session_id: &str,
        provider: &str,
        bytes: Vec<u8>,
    ) -> Result<(), RegistryError> {
        let mut inner = self.inner.lock();
        let entry = inner
            .get_mut(session_id)
            .ok_or_else(|| RegistryError::SessionNotRegistered(session_id.to_string()))?;
        entry
            .credentials
            .insert(provider.to_string(), SecretBytes::new(bytes));
        Ok(())
    }

    pub(crate) fn get_credential(&self, session_id: &str, provider: &str) -> Option<SecretBytes> {
        let inner = self.inner.lock();
        inner
            .get(session_id)?
            .credentials
            .get(provider)
            .map(|secret| SecretBytes::new(secret.0.to_vec()))
    }

    pub(crate) fn clear_credential(&self, session_id: &str, provider: &str) -> bool {
        self.inner
            .lock()
            .get_mut(session_id)
            .and_then(|entry| entry.credentials.remove(provider))
            .is_some()
    }

    pub(crate) fn clear_credentials(&self, session_id: &str) -> bool {
        let mut inner = self.inner.lock();
        let Some(entry) = inner.get_mut(session_id) else {
            return false;
        };
        if entry.credentials.is_empty() {
            return false;
        }
        entry.credentials.clear();
        true
    }
}

#[cfg(test)]
tests_bridge_macro::tests_bridge!("session_execution");
