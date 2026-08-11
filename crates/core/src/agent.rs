//! The single-agent runtime. The bare `Agent` state machine (this file) is always on —
//! `prompt()` / `continue()` / `subscribe()` / `abort()`, no harness dependency. The
//! harness layer (skills, sessions, compaction, permission, …) lives in the submodules
//! below, behind `#[cfg(feature = "harness")]` (opt-out for embedders that only want the
//! bare Agent). Orchestration builds on top in `crate::multiagent`.
//!
//! Implemented:
//! - State container + getters/setters (Mutex-protected)
//! - Listener subscription with unsubscribe fn
//! - `prompt(...)` / `continue_()` driving the agent loop
//! - `abort()` via `tokio_util::sync::CancellationToken`
//! - Steering / follow-up queues (`enqueue_steering` / `enqueue_follow_up`)
//!
//! TODO:
//! - `onPayload` / `onResponse` SimpleStreamOptions surface
//! - `transformContext` & `getApiKey` hooks (declared, wired up later)
//! - `prepareNextTurn` model/thinking-level rewrite mid-run

// Harness layer (feature-gated): the bare Agent stays always-on.
#[cfg(feature = "harness")]
pub mod assembly;
#[cfg(feature = "harness")]
pub mod compaction;
#[cfg(feature = "harness")]
pub mod cost;
#[cfg(all(feature = "harness", feature = "native-env"))]
pub mod env;
#[cfg(feature = "harness")]
pub mod hooks;
#[cfg(feature = "harness")]
pub mod messages;
#[cfg(feature = "harness")]
pub mod permission;
// The loop engine is part of the bare Agent (prompt()/continue_() call it) — always on.
pub mod run_loop;
#[cfg(feature = "harness")]
pub mod session;
#[cfg(feature = "harness")]
pub mod skills;
#[cfg(feature = "harness")]
pub mod system_prompt;
#[cfg(feature = "harness")]
pub mod types;
#[cfg(feature = "harness")]
pub mod utils;

use std::sync::Arc;

use parking_lot::Mutex;
use tokio::sync::{Notify, broadcast};
use tokio_util::sync::CancellationToken;

use crate::agent::run_loop::{run_agent_loop, run_agent_loop_continue};
use crate::types::*;

use theway_llm_provider::Message;

/// Async listener for lifecycle events. Receives an event and the active cancellation token
/// for the run. Used for subscribers that need to perform I/O (e.g. session persistence).
/// For memory-only, sub-microsecond operations prefer [`LoopSyncCallback`].
pub type LoopListener = Arc<
    dyn Fn(
            LoopEvent,
            CancellationToken,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        + Send
        + Sync,
>;

/// Lightweight synchronous callback for lifecycle events. MUST complete in <50µs — no I/O,
/// no blocking, no allocation beyond simple atomic/counter updates. Each callback is wrapped
/// in `catch_unwind` during emission so a panic in one does not affect others.
pub type LoopSyncCallback = Arc<dyn Fn(&LoopEvent) + Send + Sync>;

/// Capacity of the [`LoopEvent`] broadcast channel.
pub const LOOP_EVENT_BROADCAST_CAPACITY: usize = 256;

/// Options accepted by [`Agent::new`].
#[derive(Default)]
pub struct AgentOptions {
    pub initial_state: Option<AgentState>,
    pub convert_to_llm: Option<ConvertToLlm>,
    pub transform_context: Option<TransformContext>,
    pub stream_fn: Option<StreamFn>,
    pub get_api_key: Option<GetApiKey>,
    pub before_tool_call: Option<BeforeToolCallHook>,
    pub after_tool_call: Option<AfterToolCallHook>,
    pub on_control_plane_prompt: Option<OnControlPlanePromptHook>,
    pub should_stop_after_turn: Option<ShouldStopHook>,
    pub prepare_next_turn: Option<PrepareNextTurnHook>,
    pub steering_mode: QueueMode,
    pub follow_up_mode: QueueMode,
    pub session_id: Option<String>,
    pub tool_execution: ToolExecutionMode,
}

/// Stateful wrapper around the low-level agent loop.
pub struct Agent {
    inner: Arc<AgentInner>,
}

pub(crate) struct AgentInner {
    pub state: Mutex<AgentState>,
    /// Segment 1: synchronous callbacks (memory-only, <50µs). Each wrapped in `catch_unwind`.
    pub sync_callbacks: Mutex<Vec<LoopSyncCallback>>,
    /// Segment 2: async await-listeners (persistence, I/O). Emitted sequentially.
    pub await_listeners: Mutex<Vec<LoopListener>>,
    /// Segment 3: broadcast channel for external subscribers (UI, gRPC, hooks). Non-blocking send.
    pub broadcast_tx: broadcast::Sender<LoopEvent>,
    pub steering: Mutex<PendingMessageQueue>,
    pub follow_up: Mutex<PendingMessageQueue>,
    pub options: AgentOptions,
    pub active_cancel: Mutex<Option<CancellationToken>>,
    /// Per-turn cancel token: `interrupt()` cancels the in-flight LLM call only;
    /// the run survives if a steering message is queued, otherwise it ends.
    pub turn_cancel: Mutex<Option<CancellationToken>>,
    pub idle: Notify,
}

pub(crate) struct PendingMessageQueue {
    mode: QueueMode,
    items: Vec<AgentMessage>,
}

impl PendingMessageQueue {
    fn new(mode: QueueMode) -> Self {
        Self {
            mode,
            items: Vec::new(),
        }
    }

    pub fn enqueue(&mut self, m: AgentMessage) {
        self.items.push(m);
    }

    pub fn drain(&mut self) -> Vec<AgentMessage> {
        match self.mode {
            QueueMode::All => std::mem::take(&mut self.items),
            QueueMode::OneAtATime => {
                if self.items.is_empty() {
                    Vec::new()
                } else {
                    vec![self.items.remove(0)]
                }
            }
        }
    }
}

impl Agent {
    pub fn new(mut options: AgentOptions) -> Self {
        let state = options.initial_state.take().unwrap_or_default();
        if options.convert_to_llm.is_none() {
            options.convert_to_llm = Some(default_convert_to_llm());
        }
        let (broadcast_tx, _) = broadcast::channel(LOOP_EVENT_BROADCAST_CAPACITY);
        let inner = AgentInner {
            state: Mutex::new(state),
            sync_callbacks: Mutex::new(Vec::new()),
            await_listeners: Mutex::new(Vec::new()),
            broadcast_tx,
            steering: Mutex::new(PendingMessageQueue::new(options.steering_mode)),
            follow_up: Mutex::new(PendingMessageQueue::new(options.follow_up_mode)),
            options,
            active_cancel: Mutex::new(None),
            turn_cancel: Mutex::new(None),
            idle: Notify::new(),
        };
        Self {
            inner: Arc::new(inner),
        }
    }

    /// Subscribe an async listener (segment 2 — await path). For persistence/I/O subscribers
    /// that need the cancellation token. Returns an unsubscribe closure.
    ///
    /// For memory-only callbacks (<50µs), use [`Self::subscribe_sync`]. For external
    /// subscribers that want a broadcast [`tokio::sync::broadcast::Receiver`], use
    /// [`Self::subscribe_broadcast`].
    pub fn subscribe(&self, listener: LoopListener) -> impl FnOnce() {
        let inner = self.inner.clone();
        inner.await_listeners.lock().push(listener.clone());
        move || {
            let mut listeners = inner.await_listeners.lock();
            if let Some(pos) = listeners.iter().position(|l| Arc::ptr_eq(l, &listener)) {
                listeners.remove(pos);
            }
        }
    }

    /// Register a synchronous callback (segment 1 — catch_unwind path). The callback MUST
    /// complete in <50µs — no I/O, no blocking. Returns an unsubscribe closure.
    pub fn subscribe_sync(&self, callback: LoopSyncCallback) -> impl FnOnce() {
        let inner = self.inner.clone();
        inner.sync_callbacks.lock().push(callback.clone());
        move || {
            let mut cbs = inner.sync_callbacks.lock();
            if let Some(pos) = cbs.iter().position(|c| Arc::ptr_eq(c, &callback)) {
                cbs.remove(pos);
            }
        }
    }

    /// Obtain a new [`tokio::sync::broadcast::Receiver`] for the LoopEvent broadcast
    /// channel (segment 3). The receiver sees all events emitted after subscription.
    pub fn subscribe_broadcast(&self) -> broadcast::Receiver<LoopEvent> {
        self.inner.broadcast_tx.subscribe()
    }

    /// Inspect the current agent state. The lock guards against concurrent loop mutations.
    pub fn state(&self) -> parking_lot::MutexGuard<'_, AgentState> {
        self.inner.state.lock()
    }

    pub fn is_streaming(&self) -> bool {
        self.inner.state.lock().is_streaming
    }

    pub fn enqueue_steering(&self, message: AgentMessage) {
        self.inner.steering.lock().enqueue(message);
    }

    pub fn enqueue_follow_up(&self, message: AgentMessage) {
        self.inner.follow_up.lock().enqueue(message);
    }

    /// Abort the active run, if any. Subsequent calls are no-ops.
    pub fn abort(&self) {
        if let Some(token) = self.inner.active_cancel.lock().as_ref() {
            token.cancel();
        }
    }

    /// Interrupt the current turn: cancels the in-flight LLM call. The run ends
    /// unless a steering message is queued (then the next turn carries it).
    pub fn interrupt(&self) {
        if let Some(token) = self.inner.turn_cancel.lock().as_ref() {
            token.cancel();
        }
    }

    /// Active cancellation token while a run is in flight, otherwise `None`.
    pub fn active_token(&self) -> Option<CancellationToken> {
        self.inner.active_cancel.lock().clone()
    }

    /// Start a new prompt. Appends a user `AgentMessage`, runs the loop, awaits completion.
    pub async fn prompt(&self, message: AgentMessage) -> Result<(), AgentRunError> {
        self.prompt_many(vec![message]).await
    }

    /// Start a new prompt with a batch of messages.
    pub async fn prompt_many(&self, messages: Vec<AgentMessage>) -> Result<(), AgentRunError> {
        self.guard_not_streaming()?;
        run_agent_loop(self.inner.clone(), messages).await
    }

    /// Continue from the current transcript without appending new user messages.
    pub async fn continue_(&self) -> Result<(), AgentRunError> {
        self.guard_not_streaming()?;
        run_agent_loop_continue(self.inner.clone()).await
    }

    fn guard_not_streaming(&self) -> Result<(), AgentRunError> {
        if self.is_streaming() {
            return Err(AgentRunError::AlreadyStreaming);
        }
        Ok(())
    }
}

/// Errors that can short-circuit `prompt` / `continue_`.
#[derive(Debug, thiserror::Error)]
pub enum AgentRunError {
    #[error(
        "Agent is already processing a prompt. Use enqueue_steering/enqueue_follow_up or wait for completion."
    )]
    AlreadyStreaming,
    /// The current turn was interrupted via [`Agent::interrupt`] and no steering
    /// message was queued, so the run ended at the turn boundary.
    #[error("turn interrupted")]
    TurnInterrupted,
    #[error("{0}")]
    Other(String),
}

impl AgentInner {
    pub fn convert_to_llm(&self, msgs: &[AgentMessage]) -> Vec<Message> {
        self.options
            .convert_to_llm
            .as_ref()
            .expect("convert_to_llm is always set in Agent::new")(msgs)
    }
}
