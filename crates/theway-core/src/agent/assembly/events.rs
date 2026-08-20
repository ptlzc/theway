use std::sync::Arc;

use tokio::sync::broadcast;

use crate::types::AgentMessage;

use super::AgentHarness;

impl AgentHarness {
    /// Register a harness-level lifecycle listener. Returns an unsubscriber closure.
    ///
    /// Listener panics are caught — see [`SessionEvent`] for the isolation contract. The
    /// returned closure removes the listener; calling it twice is a no-op.
    pub fn subscribe_harness(&self, listener: SessionListener) -> Box<dyn FnOnce() + Send> {
        self.harness_listeners.lock().push(listener.clone());
        let target = Arc::as_ptr(&listener) as *const () as usize;
        let listeners = Arc::clone(&self.harness_listeners);
        Box::new(move || {
            let mut guard = listeners.lock();
            if let Some(index) = guard
                .iter()
                .position(|item| (Arc::as_ptr(item) as *const () as usize) == target)
            {
                guard.remove(index);
            }
        })
    }

    /// Obtain a new [`tokio::sync::broadcast::Receiver`] for the [`SessionEvent`] broadcast
    /// channel. The receiver sees all events emitted after subscription.
    pub fn subscribe_session_broadcast(&self) -> broadcast::Receiver<SessionEvent> {
        self.session_broadcast_tx.subscribe()
    }

    /// Dispatch to isolated synchronous callbacks, then publish to the broadcast channel.
    pub(crate) fn emit_harness_event(&self, event: SessionEvent) {
        let listeners = self.harness_listeners.lock().clone();
        for listener in listeners {
            let event = event.clone();
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || listener(event)));
        }
        let _ = self.session_broadcast_tx.send(event);
    }

    pub(super) async fn ensure_session_start_emitted(&self) {
        let should_emit = {
            let mut emitted = self.session_start_emitted.lock();
            if *emitted {
                false
            } else {
                *emitted = true;
                true
            }
        };
        if !should_emit {
            return;
        }
        let messages_replayed = self.agent.state().messages.len();
        self.runtime_extensions.ensure_session_start().await;
        self.emit_harness_event(SessionEvent::Started { messages_replayed });
    }

    /// Reconstruct the session-scoped extension runtime before it begins
    /// serving prompts. Idempotent with the first prompt path.
    pub async fn start_runtime_extensions(&self) {
        self.ensure_session_start_emitted().await;
    }

    /// Cancel any active run, wait for its awaited cleanup, then publish the
    /// extension session-shutdown lifecycle exactly once.
    pub async fn shutdown_runtime_extensions(&self) {
        self.abort();
        self.agent.wait_until_idle().await;
        self.runtime_extensions.shutdown().await;
    }
}

/// Harness-level lifecycle events emitted in addition to the inner agent's per-turn events.
#[derive(Clone, Debug)]
pub enum SessionEvent {
    /// First prompt entry after construction.
    Started { messages_replayed: usize },
    /// Auto- or manual compaction completed.
    Compaction {
        from_hook: bool,
        summary: String,
        tokens_before: u64,
    },
    /// The active session branch changed.
    Branch {
        from_entry_id: Option<String>,
        to_entry_id: Option<String>,
        summary_entry_id: Option<String>,
    },
    /// A best-effort persistence operation failed.
    PersistenceError { context: String, message: String },
    /// A turn-completion hook made a lifecycle decision.
    TurnDecision {
        decision: &'static str,
        continuation_count: u32,
        reason: Option<String>,
        next_prompt_preview: Option<String>,
    },
    /// The skill catalog was hot-reloaded.
    SkillsReloaded { total: usize },
    /// An extension handled input without starting an LLM run.
    ExtensionCommandOutcome {
        outcome: theway_contract::extension::ExtensionCommandOutcome,
    },
}

pub type SessionListener = Arc<dyn Fn(SessionEvent) + Send + Sync>;

/// Snapshot passed into [`OnTurnEndHook`] after a prompt cycle reaches a natural stop.
#[derive(Clone)]
pub struct OnTurnEndContext {
    pub transcript: Vec<AgentMessage>,
    pub continuation_count: u32,
    pub last_user_prompt: Option<String>,
}

/// What the runtime should do after [`OnTurnEndHook`] inspects a completed prompt cycle.
#[derive(Clone, Debug)]
pub enum TurnEndAction {
    Noop,
    Stop,
    Pause { reason: String },
    Continue { prompt: String },
}

impl TurnEndAction {
    /// Stable value persisted in turn-end audit entries; `Noop` deliberately has no audit.
    pub fn as_audit_str(&self) -> Option<&'static str> {
        match self {
            Self::Noop => None,
            Self::Stop => Some("stop"),
            Self::Pause { .. } => Some("pause"),
            Self::Continue { .. } => Some("continue"),
        }
    }
}

/// Decision envelope returned from [`OnTurnEndHook`].
#[derive(Clone, Debug)]
pub struct TurnEndDecision {
    pub action: TurnEndAction,
    pub payload: Option<serde_json::Value>,
}

impl From<TurnEndAction> for TurnEndDecision {
    fn from(action: TurnEndAction) -> Self {
        Self {
            action,
            payload: None,
        }
    }
}

pub type OnTurnEndHook = Arc<
    dyn Fn(
            OnTurnEndContext,
            tokio_util::sync::CancellationToken,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = TurnEndDecision> + Send>>
        + Send
        + Sync,
>;

/// Default maximum number of continuation iterations per prompt cycle.
pub const DEFAULT_TURN_CONTINUATION_CAP: u32 = 25;
