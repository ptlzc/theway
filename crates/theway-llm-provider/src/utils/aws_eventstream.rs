//! Minimal decoder for the AWS `vnd.amazon.eventstream` binary framing used by Bedrock's
//! Converse Stream API. No 1:1 TS counterpart — the TS side gets this from the AWS SDK.
//!
//! Frame layout (all integers big-endian):
//! ```text
//! [total_len u32][headers_len u32][prelude_crc u32][headers ...][payload ...][message_crc u32]
//! ```
//! Header entry: `[name_len u8][name][value_type u8][value ...]`. We only need the
//! `:event-type` header (value type 7 = string) and the JSON payload; CRCs are not verified.

use bytes::{Bytes, BytesMut};
use futures::Stream;
use std::pin::Pin;
use std::task::{Context, Poll};

#[derive(Clone, Debug, Default)]
pub struct EventStreamMessage {
    /// `:event-type` header (e.g. "contentBlockDelta", "messageStop").
    pub event_type: Option<String>,
    /// `:exception-type` header, present on error frames.
    pub exception_type: Option<String>,
    /// Raw payload bytes (JSON for Bedrock converse events).
    pub payload: Bytes,
}

pub struct AwsEventStream<S> {
    inner: S,
    buf: BytesMut,
    upstream_done: bool,
}

impl<S> AwsEventStream<S> {
    pub fn new(inner: S) -> Self {
        Self {
            inner,
            buf: BytesMut::new(),
            upstream_done: false,
        }
    }

    /// Try to decode one complete frame from the front of the buffer.
    fn try_decode(&mut self) -> Option<EventStreamMessage> {
        if self.buf.len() < 12 {
            return None;
        }
        let total_len =
            u32::from_be_bytes([self.buf[0], self.buf[1], self.buf[2], self.buf[3]]) as usize;
        if total_len < 16 || self.buf.len() < total_len {
            return None;
        }
        let headers_len =
            u32::from_be_bytes([self.buf[4], self.buf[5], self.buf[6], self.buf[7]]) as usize;

        let frame = self.buf.split_to(total_len);
        let headers_start = 12;
        let headers_end = headers_start + headers_len;
        let payload_end = total_len - 4; // trailing message CRC
        if headers_end > frame.len() || payload_end > frame.len() || headers_end > payload_end {
            return Some(EventStreamMessage::default());
        }

        let mut msg = EventStreamMessage {
            payload: Bytes::copy_from_slice(&frame[headers_end..payload_end]),
            ..Default::default()
        };
        decode_headers(&frame[headers_start..headers_end], &mut msg);
        Some(msg)
    }
}

fn decode_headers(mut h: &[u8], msg: &mut EventStreamMessage) {
    while !h.is_empty() {
        let name_len = h[0] as usize;
        if 1 + name_len + 1 > h.len() {
            return;
        }
        let name = std::str::from_utf8(&h[1..1 + name_len]).unwrap_or("");
        let value_type = h[1 + name_len];
        let mut cursor = 1 + name_len + 1;
        // Value type 7 = string: [len u16][bytes]. Other types we skip by known widths.
        let value: Option<String> = match value_type {
            7 => {
                if cursor + 2 > h.len() {
                    return;
                }
                let vlen = u16::from_be_bytes([h[cursor], h[cursor + 1]]) as usize;
                cursor += 2;
                if cursor + vlen > h.len() {
                    return;
                }
                let v = std::str::from_utf8(&h[cursor..cursor + vlen])
                    .unwrap_or("")
                    .to_string();
                cursor += vlen;
                Some(v)
            }
            0 | 1 => None, // bool true/false, no value bytes
            2 => {
                cursor += 1;
                None
            } // byte
            3 => {
                cursor += 2;
                None
            } // short
            4 => {
                cursor += 4;
                None
            } // int
            5 => {
                cursor += 8;
                None
            } // long
            6 => {
                // byte buffer: [len u16][bytes]
                if cursor + 2 > h.len() {
                    return;
                }
                let vlen = u16::from_be_bytes([h[cursor], h[cursor + 1]]) as usize;
                cursor += 2 + vlen;
                None
            }
            8 => {
                cursor += 8;
                None
            } // timestamp
            9 => {
                cursor += 16;
                None
            } // uuid
            _ => return,
        };
        match name {
            ":event-type" => msg.event_type = value,
            ":exception-type" => msg.exception_type = value,
            _ => {}
        }
        if cursor > h.len() {
            return;
        }
        h = &h[cursor..];
    }
}

impl<S> Stream for AwsEventStream<S>
where
    S: Stream<Item = Result<Bytes, reqwest::Error>> + Unpin,
{
    type Item = Result<EventStreamMessage, reqwest::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            if let Some(msg) = self.try_decode() {
                return Poll::Ready(Some(Ok(msg)));
            }
            if self.upstream_done {
                return Poll::Ready(None);
            }
            match Pin::new(&mut self.inner).poll_next(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Some(Err(e))) => return Poll::Ready(Some(Err(e))),
                Poll::Ready(None) => self.upstream_done = true,
                Poll::Ready(Some(Ok(chunk))) => self.buf.extend_from_slice(&chunk),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a single frame with one `:event-type` string header and a JSON payload.
    fn frame(event_type: &str, payload: &[u8]) -> Vec<u8> {
        // header: [name_len][name][type=7][val_len u16][val]
        let name = ":event-type";
        let mut headers = Vec::new();
        headers.push(name.len() as u8);
        headers.extend_from_slice(name.as_bytes());
        headers.push(7);
        headers.extend_from_slice(&(event_type.len() as u16).to_be_bytes());
        headers.extend_from_slice(event_type.as_bytes());

        let total_len = 12 + headers.len() + payload.len() + 4;
        let mut out = Vec::new();
        out.extend_from_slice(&(total_len as u32).to_be_bytes());
        out.extend_from_slice(&(headers.len() as u32).to_be_bytes());
        out.extend_from_slice(&0u32.to_be_bytes()); // prelude crc (unverified)
        out.extend_from_slice(&headers);
        out.extend_from_slice(payload);
        out.extend_from_slice(&0u32.to_be_bytes()); // message crc (unverified)
        out
    }

    #[test]
    fn decodes_event_type_and_payload() {
        let bytes = frame("contentBlockDelta", br#"{"delta":{"text":"hi"}}"#);
        let mut s = AwsEventStream::new(futures::stream::empty::<Result<Bytes, reqwest::Error>>());
        s.buf.extend_from_slice(&bytes);
        let msg = s.try_decode().expect("one frame");
        assert_eq!(msg.event_type.as_deref(), Some("contentBlockDelta"));
        assert_eq!(&msg.payload[..], br#"{"delta":{"text":"hi"}}"#);
    }

    #[test]
    fn partial_frame_returns_none() {
        let bytes = frame("messageStop", b"{}");
        let mut s = AwsEventStream::new(futures::stream::empty::<Result<Bytes, reqwest::Error>>());
        s.buf.extend_from_slice(&bytes[..8]); // truncated
        assert!(s.try_decode().is_none());
    }
}
