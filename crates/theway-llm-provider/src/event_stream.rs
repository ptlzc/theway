//! AWS `application/vnd.amazon.eventstream` binary framing parser. Used by Bedrock's
//! `:invoke-with-response-stream` endpoint. Each on-wire message is laid out as:
//!
//! ```text
//! +----------------+----------------+----------------+
//! | total_length   | headers_length | prelude_crc    |  (12 bytes, big-endian u32 each)
//! +----------------+----------------+----------------+
//! | headers (variable)                              |
//! +----------------+--------------------------------+
//! | payload (variable)                              |
//! +-------------------------------------------------+
//! | message_crc    | (4 bytes, big-endian u32)      |
//! +----------------+--------------------------------+
//! ```
//!
//! Each header is `name_len:u8 | name | value_type:u8 | (type-specific)`. v1 supports only
//! the value types that AWS event-stream uses in practice (string + byte buffer); other
//! types deserialize as raw bytes so the caller can decide.
//!
//! CRC32 follows the IEEE polynomial that AWS specifies. Hand-rolled to avoid pulling a CRC
//! crate just for this.

#![allow(dead_code)]

use std::collections::HashMap;

#[derive(Debug, thiserror::Error)]
pub enum EventStreamError {
    #[error("frame too short: need {need} bytes, have {have}")]
    Short { need: usize, have: usize },
    #[error("prelude CRC mismatch (expected {expected:#x}, got {got:#x})")]
    PreludeCrc { expected: u32, got: u32 },
    #[error("message CRC mismatch (expected {expected:#x}, got {got:#x})")]
    MessageCrc { expected: u32, got: u32 },
    #[error("invalid header length (total {total}, headers {headers})")]
    HeaderLen { total: u32, headers: u32 },
    #[error("unsupported header value type {0}")]
    HeaderValue(u8),
    #[error("malformed header: {0}")]
    Header(&'static str),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventMessage {
    pub headers: HashMap<String, HeaderValue>,
    pub payload: Vec<u8>,
}

impl EventMessage {
    pub fn message_type(&self) -> Option<&str> {
        self.header_str(":message-type")
    }
    pub fn event_type(&self) -> Option<&str> {
        self.header_str(":event-type")
    }
    pub fn content_type(&self) -> Option<&str> {
        self.header_str(":content-type")
    }
    fn header_str(&self, key: &str) -> Option<&str> {
        match self.headers.get(key)? {
            HeaderValue::String(s) => Some(s),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeaderValue {
    String(String),
    Bytes(Vec<u8>),
    Bool(bool),
    Int(i64),
    Other { value_type: u8, raw: Vec<u8> },
}

/// Parse one event-stream message from the head of `buf`. Returns the message + the number
/// of bytes consumed. The caller is responsible for stream-buffering when partial frames
/// arrive.
pub fn parse_message(buf: &[u8]) -> Result<(EventMessage, usize), EventStreamError> {
    if buf.len() < 12 {
        return Err(EventStreamError::Short {
            need: 12,
            have: buf.len(),
        });
    }
    let total = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]);
    let header_len = u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]);
    let prelude_crc = u32::from_be_bytes([buf[8], buf[9], buf[10], buf[11]]);
    let calc_prelude = crc32(&buf[..8]);
    if calc_prelude != prelude_crc {
        return Err(EventStreamError::PreludeCrc {
            expected: prelude_crc,
            got: calc_prelude,
        });
    }
    let total_usz = total as usize;
    if buf.len() < total_usz {
        return Err(EventStreamError::Short {
            need: total_usz,
            have: buf.len(),
        });
    }
    // Headers are at bytes [12 .. 12 + header_len], payload at [12+header_len .. total-4],
    // then 4 bytes of message CRC.
    let headers_start = 12usize;
    let headers_end = headers_start + header_len as usize;
    if headers_end + 4 > total_usz {
        return Err(EventStreamError::HeaderLen {
            total,
            headers: header_len,
        });
    }
    let payload_start = headers_end;
    let payload_end = total_usz - 4;
    let msg_crc = u32::from_be_bytes([
        buf[total_usz - 4],
        buf[total_usz - 3],
        buf[total_usz - 2],
        buf[total_usz - 1],
    ]);
    let calc_msg = crc32(&buf[..total_usz - 4]);
    if calc_msg != msg_crc {
        return Err(EventStreamError::MessageCrc {
            expected: msg_crc,
            got: calc_msg,
        });
    }
    let headers = parse_headers(&buf[headers_start..headers_end])?;
    let payload = buf[payload_start..payload_end].to_vec();
    Ok((EventMessage { headers, payload }, total_usz))
}

fn parse_headers(mut buf: &[u8]) -> Result<HashMap<String, HeaderValue>, EventStreamError> {
    let mut out = HashMap::new();
    while !buf.is_empty() {
        let name_len = *buf
            .first()
            .ok_or(EventStreamError::Header("missing name_len"))? as usize;
        buf = &buf[1..];
        if buf.len() < name_len + 1 {
            return Err(EventStreamError::Header("name truncated"));
        }
        let name = std::str::from_utf8(&buf[..name_len])
            .map_err(|_| EventStreamError::Header("name not utf-8"))?
            .to_string();
        buf = &buf[name_len..];
        let value_type = buf[0];
        buf = &buf[1..];
        let value = match value_type {
            7 => {
                // String: u16 length + utf-8 bytes
                if buf.len() < 2 {
                    return Err(EventStreamError::Header("string length missing"));
                }
                let len = u16::from_be_bytes([buf[0], buf[1]]) as usize;
                buf = &buf[2..];
                if buf.len() < len {
                    return Err(EventStreamError::Header("string body truncated"));
                }
                let s = std::str::from_utf8(&buf[..len])
                    .map_err(|_| EventStreamError::Header("string not utf-8"))?
                    .to_string();
                buf = &buf[len..];
                HeaderValue::String(s)
            }
            6 => {
                // Byte buffer: u16 length + raw bytes
                if buf.len() < 2 {
                    return Err(EventStreamError::Header("bytes length missing"));
                }
                let len = u16::from_be_bytes([buf[0], buf[1]]) as usize;
                buf = &buf[2..];
                if buf.len() < len {
                    return Err(EventStreamError::Header("bytes body truncated"));
                }
                let v = buf[..len].to_vec();
                buf = &buf[len..];
                HeaderValue::Bytes(v)
            }
            0 => HeaderValue::Bool(true),
            1 => HeaderValue::Bool(false),
            4 => {
                if buf.len() < 4 {
                    return Err(EventStreamError::Header("int32 truncated"));
                }
                let v = i32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]);
                buf = &buf[4..];
                HeaderValue::Int(v as i64)
            }
            5 => {
                if buf.len() < 8 {
                    return Err(EventStreamError::Header("int64 truncated"));
                }
                let v = i64::from_be_bytes([
                    buf[0], buf[1], buf[2], buf[3], buf[4], buf[5], buf[6], buf[7],
                ]);
                buf = &buf[8..];
                HeaderValue::Int(v)
            }
            other => {
                // Skip unknown type. We can't recover length without knowing the schema,
                // so error here. The caller can treat this as a hard failure.
                return Err(EventStreamError::HeaderValue(other));
            }
        };
        out.insert(name, value);
    }
    Ok(out)
}

/// IEEE 802.3 CRC32 (polynomial 0xEDB88320). Hand-rolled with a 256-entry lookup table built
/// at first use.
pub fn crc32(data: &[u8]) -> u32 {
    use once_cell::sync::Lazy;
    static TABLE: Lazy<[u32; 256]> = Lazy::new(|| {
        let mut t = [0u32; 256];
        for (i, slot) in t.iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xEDB88320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
            *slot = c;
        }
        t
    });
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in data {
        let idx = ((crc ^ b as u32) & 0xFF) as usize;
        crc = TABLE[idx] ^ (crc >> 8);
    }
    crc ^ 0xFFFF_FFFF
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CRC32 of "123456789" is the standard reference value 0xCBF43926.
    #[test]
    fn crc32_matches_reference_vector() {
        assert_eq!(crc32(b"123456789"), 0xCBF43926);
        // Empty input → 0.
        assert_eq!(crc32(b""), 0);
    }

    /// Round-trip: hand-build a minimal event-stream message and parse it back. This
    /// validates the prelude/headers/payload/trailer layout end to end.
    #[test]
    fn round_trip_string_header_and_payload() {
        // Headers: ":event-type" -> "chunk"
        // Header encoding: name_len(1) + name + type(7) + len:u16 + value
        let name = b":event-type";
        let value = b"chunk";
        let mut headers = Vec::new();
        headers.push(name.len() as u8);
        headers.extend_from_slice(name);
        headers.push(7); // string type
        headers.extend_from_slice(&(value.len() as u16).to_be_bytes());
        headers.extend_from_slice(value);

        let payload = b"{\"hello\":\"world\"}";
        let total: u32 = 12 + headers.len() as u32 + payload.len() as u32 + 4;
        let headers_len: u32 = headers.len() as u32;

        let mut buf = Vec::with_capacity(total as usize);
        buf.extend_from_slice(&total.to_be_bytes());
        buf.extend_from_slice(&headers_len.to_be_bytes());
        let prelude_crc = crc32(&buf[..8]);
        buf.extend_from_slice(&prelude_crc.to_be_bytes());
        buf.extend_from_slice(&headers);
        buf.extend_from_slice(payload);
        let msg_crc = crc32(&buf);
        buf.extend_from_slice(&msg_crc.to_be_bytes());

        let (msg, consumed) = parse_message(&buf).unwrap();
        assert_eq!(consumed, buf.len());
        assert_eq!(msg.event_type(), Some("chunk"));
        assert_eq!(msg.payload, payload);
    }

    #[test]
    fn rejects_bad_prelude_crc() {
        // Hand-build a valid frame, then flip one byte of the prelude CRC.
        let payload = b"x";
        let total: u32 = 12 + payload.len() as u32 + 4;
        let mut buf = Vec::new();
        buf.extend_from_slice(&total.to_be_bytes());
        buf.extend_from_slice(&0u32.to_be_bytes());
        let good = crc32(&buf[..8]);
        buf.extend_from_slice(&(good ^ 0xFF).to_be_bytes());
        buf.extend_from_slice(payload);
        let msg_crc = crc32(&buf);
        buf.extend_from_slice(&msg_crc.to_be_bytes());
        assert!(matches!(
            parse_message(&buf),
            Err(EventStreamError::PreludeCrc { .. })
        ));
    }

    #[test]
    fn short_buffer_reports_short_error() {
        assert!(matches!(
            parse_message(&[0u8; 4]),
            Err(EventStreamError::Short { .. })
        ));
    }
}
