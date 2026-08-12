//! Server-Sent-Events parser. No 1:1 TS counterpart — the TS package depends on the third-party
//! `eventsource-parser`. We inline a minimal parser here so providers don't have to.
//!
//! Spec: <https://html.spec.whatwg.org/multipage/server-sent-events.html#event-stream-interpretation>
//! We deliberately implement only what providers use (`event:` and `data:` fields; comments
//! starting with `:` are dropped).

use bytes::{Bytes, BytesMut};
use futures::Stream;
use std::pin::Pin;
use std::task::{Context, Poll};

#[derive(Clone, Debug, Default)]
pub struct SseEvent {
    pub event: Option<String>,
    pub data: String,
}

/// Stream adapter: turns an HTTP body byte stream into a stream of parsed `SseEvent`s.
pub struct SseStream<S> {
    inner: S,
    buf: BytesMut,
    current: SseEvent,
    pending: Vec<SseEvent>,
    upstream_done: bool,
}

impl<S> SseStream<S> {
    pub fn new(inner: S) -> Self {
        Self {
            inner,
            buf: BytesMut::new(),
            current: SseEvent::default(),
            pending: Vec::new(),
            upstream_done: false,
        }
    }

    fn flush_current(&mut self) {
        if !self.current.data.is_empty() || self.current.event.is_some() {
            self.pending.push(std::mem::take(&mut self.current));
        }
    }

    fn drain_lines(&mut self) {
        // Find each complete line in `self.buf`.
        loop {
            let nl = match self.buf.iter().position(|&b| b == b'\n') {
                Some(i) => i,
                None => return,
            };
            let line = self.buf.split_to(nl + 1);
            let line = std::str::from_utf8(&line[..line.len().saturating_sub(1)])
                .unwrap_or("")
                .trim_end_matches('\r');
            if line.is_empty() {
                self.flush_current();
                continue;
            }
            if line.starts_with(':') {
                // Comment — ignore.
                continue;
            }
            let (field, value) = match line.find(':') {
                Some(i) => (&line[..i], line[i + 1..].trim_start_matches(' ')),
                None => (line, ""),
            };
            match field {
                "event" => self.current.event = Some(value.to_string()),
                "data" => {
                    if !self.current.data.is_empty() {
                        self.current.data.push('\n');
                    }
                    self.current.data.push_str(value);
                }
                _ => {} // ignore `id`, `retry`, etc.
            }
        }
    }
}

impl<S> Stream for SseStream<S>
where
    S: Stream<Item = Result<Bytes, reqwest::Error>> + Unpin,
{
    type Item = Result<SseEvent, reqwest::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            if let Some(ev) = self.pending.pop() {
                return Poll::Ready(Some(Ok(ev)));
            }
            if self.upstream_done {
                self.flush_current();
                if let Some(ev) = self.pending.pop() {
                    return Poll::Ready(Some(Ok(ev)));
                }
                return Poll::Ready(None);
            }
            match Pin::new(&mut self.inner).poll_next(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Some(Err(e))) => return Poll::Ready(Some(Err(e))),
                Poll::Ready(None) => {
                    self.upstream_done = true;
                }
                Poll::Ready(Some(Ok(chunk))) => {
                    self.buf.extend_from_slice(&chunk);
                    self.drain_lines();
                    // Order: FIFO. drain_lines pushes to back; we pop from back. Reverse so the
                    // first event we parsed comes out first.
                    self.pending.reverse();
                }
            }
        }
    }
}
