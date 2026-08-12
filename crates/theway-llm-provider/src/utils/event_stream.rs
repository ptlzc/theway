//! `AssistantMessageEventStream` — the contract between providers and consumers.
//!
//! 1:1 port of `packages/ai/src/utils/event-stream.ts`. The TS class is both async-iterable and
//! awaitable (`stream.result()`). In Rust we get the same two access patterns via:
//!
//! - `impl Stream<Item = AssistantMessageEvent>` for iteration
//! - `.result().await` to consume and return the final `AssistantMessage`
//!
//! The producer side is a separate `AssistantMessageEventSender` — splitting sender/receiver is
//! idiomatic in Rust (mpsc), unlike the TS class that does both. The TS factory
//! `createAssistantMessageEventStream()` is mirrored by [`AssistantMessageEventStream::new`].

use std::pin::Pin;
use std::task::{Context, Poll};

use futures_core::Stream;
use tokio::sync::{mpsc, oneshot};

use crate::types::{AssistantMessage, AssistantMessageEvent};

/// Producer half. Hand this to provider code; keep [`AssistantMessageEventStream`] for the
/// consumer (typically `stream()` returns the stream and spawns a task that owns the sender).
pub struct AssistantMessageEventSender {
    tx: mpsc::UnboundedSender<AssistantMessageEvent>,
    /// Resolves on the *first* terminal event so `.result()` works even if the consumer iterates
    /// past the terminal event. Mirrors the TS `finalResultPromise`.
    final_tx: Option<oneshot::Sender<AssistantMessage>>,
}

impl AssistantMessageEventSender {
    /// Send one event. Terminal events (`Done`/`Error`) also resolve the final-result oneshot.
    pub fn push(&mut self, event: AssistantMessageEvent) {
        if event.is_terminal() {
            if let Some(final_tx) = self.final_tx.take() {
                let msg = match &event {
                    AssistantMessageEvent::Done { message, .. } => message.clone(),
                    AssistantMessageEvent::Error { error, .. } => error.clone(),
                    _ => unreachable!("is_terminal() guarantees Done or Error"),
                };
                let _ = final_tx.send(msg);
            }
        }
        // Receiver may have been dropped (consumer stopped iterating). Swallow that — provider
        // logic must not crash because the consumer lost interest.
        let _ = self.tx.send(event);
    }

    /// True when the consumer has dropped the stream. Providers can short-circuit on this.
    pub fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }

    /// Explicit close. Optional — dropping the sender has the same effect.
    pub fn close(self) {
        drop(self);
    }
}

/// Consumer half. Iterate via the `Stream` impl, or call [`Self::result`] to await the final
/// assembled message. Doing both is supported: `result()` waits on the oneshot resolved by the
/// terminal event and does not require the stream to be drained.
pub struct AssistantMessageEventStream {
    rx: mpsc::UnboundedReceiver<AssistantMessageEvent>,
    /// Holds the final message after a terminal event was pushed. Wrapped in an Option because
    /// `result()` consumes it; subsequent calls would have to re-derive from cached state.
    final_rx: Option<oneshot::Receiver<AssistantMessage>>,
}

impl AssistantMessageEventStream {
    /// Construct a (stream, sender) pair. Hand the sender to the provider task.
    pub fn new() -> (Self, AssistantMessageEventSender) {
        let (tx, rx) = mpsc::unbounded_channel();
        let (final_tx, final_rx) = oneshot::channel();
        let stream = Self {
            rx,
            final_rx: Some(final_rx),
        };
        let sender = AssistantMessageEventSender {
            tx,
            final_tx: Some(final_tx),
        };
        (stream, sender)
    }

    /// Resolve the final `AssistantMessage`. Returns the message stored by the terminal event;
    /// if the sender was dropped without emitting a terminal event, returns `None`.
    pub async fn result(mut self) -> Option<AssistantMessage> {
        let final_rx = self.final_rx.take()?;
        // Make sure the producer is making progress: if the sender side is queueing events
        // faster than they are read, the unbounded channel buffers them — that is fine, the
        // oneshot will resolve as soon as `push` sees a terminal event.
        final_rx.await.ok()
    }
}

impl Stream for AssistantMessageEventStream {
    type Item = AssistantMessageEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.rx.poll_recv(cx)
    }
}

/// Factory function. Mirrors `createAssistantMessageEventStream()` on the TS side.
pub fn create_assistant_message_event_stream()
-> (AssistantMessageEventStream, AssistantMessageEventSender) {
    AssistantMessageEventStream::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::*;
    use futures::StreamExt;

    fn mk_msg() -> AssistantMessage {
        AssistantMessage {
            role: AssistantRole::Assistant,
            content: vec![ContentBlock::text("hello")],
            api: Api::known(KnownApi::AnthropicMessages),
            provider: Provider::from("anthropic"),
            model: "claude-test".into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: StopReason::Stop,
            error_message: None,
            timestamp: 0,
        }
    }

    #[tokio::test]
    async fn iterates_to_done() {
        let (mut stream, mut sender) = AssistantMessageEventStream::new();
        let msg = mk_msg();
        sender.push(AssistantMessageEvent::Start {
            partial: msg.clone(),
        });
        sender.push(AssistantMessageEvent::Done {
            reason: DoneReason::Stop,
            message: msg,
        });
        drop(sender);
        let mut count = 0;
        while let Some(_ev) = stream.next().await {
            count += 1;
        }
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn result_resolves_before_drain() {
        let (stream, mut sender) = AssistantMessageEventStream::new();
        let msg = mk_msg();
        sender.push(AssistantMessageEvent::Done {
            reason: DoneReason::Stop,
            message: msg,
        });
        drop(sender);
        let final_msg = stream.result().await;
        assert!(final_msg.is_some());
    }
}
