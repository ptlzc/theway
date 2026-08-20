use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeExtensionScopeKind {
    Run,
    Turn,
    Request,
    Message,
    ToolCall,
}

impl RuntimeExtensionScopeKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Run => "run",
            Self::Turn => "turn",
            Self::Request => "request",
            Self::Message => "message",
            Self::ToolCall => "tool-call",
        }
    }
}

#[derive(Clone, Debug)]
pub struct RuntimeExtensionScopeAllocator {
    session_id: Arc<str>,
    next_scope: Arc<AtomicU64>,
    next_sequence: Arc<AtomicU64>,
}

impl RuntimeExtensionScopeAllocator {
    pub fn new(session_id: impl Into<String>) -> Result<Self, ScopeAllocationError> {
        let session_id = session_id.into();
        if session_id.trim().is_empty() {
            return Err(ScopeAllocationError::EmptySessionId);
        }
        Ok(Self {
            session_id: Arc::from(session_id),
            next_scope: Arc::new(AtomicU64::new(1)),
            next_sequence: Arc::new(AtomicU64::new(1)),
        })
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn allocate(
        &self,
        kind: RuntimeExtensionScopeKind,
    ) -> Result<String, ScopeAllocationError> {
        let ordinal = take_next(&self.next_scope)?;
        Ok(format!("{}:{}:{ordinal}", self.session_id, kind.as_str()))
    }

    pub fn next_sequence(&self) -> Result<u64, ScopeAllocationError> {
        take_next(&self.next_sequence)
    }
}

fn take_next(counter: &AtomicU64) -> Result<u64, ScopeAllocationError> {
    counter
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            current.checked_add(1)
        })
        .map_err(|_| ScopeAllocationError::Exhausted)
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum ScopeAllocationError {
    #[error("runtime extension session id must not be empty")]
    EmptySessionId,
    #[error("runtime extension scope or lifecycle sequence space is exhausted")]
    Exhausted,
}
