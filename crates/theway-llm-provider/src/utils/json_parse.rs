//! Partial-JSON tolerant parser. 1:1 port of `packages/ai/src/utils/json-parse.ts`.
//!
//! Streaming tool calls arrive as JSON fragments — `serde_json::from_str` would reject most of
//! them mid-stream. This module exposes a forgiving parser that closes any open braces/brackets
//! before delegating to `serde_json`. Same idea as the TS `partial-json` package.

use serde_json::Value;

/// Parse a (potentially incomplete) JSON document, closing any open structures and trailing
/// strings so the result is still valid JSON. Returns `Value::Null` for empty input.
pub fn parse_partial_json(input: &str) -> Result<Value, serde_json::Error> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Ok(Value::Null);
    }

    // Fast path — well-formed input.
    if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
        return Ok(v);
    }

    let closed = close_partial(trimmed);
    serde_json::from_str(&closed)
}

/// Walk the input and append closing tokens needed to balance braces/brackets/strings. Mirrors
/// the closing logic in the `partial-json` package; minimal but enough for tool-call args, which
/// are the only consumers.
fn close_partial(input: &str) -> String {
    let mut stack: Vec<char> = Vec::new();
    let mut in_string = false;
    let mut escape = false;
    let mut out = String::with_capacity(input.len() + 4);

    for ch in input.chars() {
        out.push(ch);
        if escape {
            escape = false;
            continue;
        }
        if in_string {
            match ch {
                '\\' => escape = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' => stack.push('}'),
            '[' => stack.push(']'),
            '}' | ']' => {
                stack.pop();
            }
            _ => {}
        }
    }

    if in_string {
        out.push('"');
    }
    // Trim trailing comma before closing — `{"a":1,` would otherwise become `{"a":1,}`.
    let trimmed = out.trim_end_matches([' ', '\n', '\r', '\t']);
    let trimmed = trimmed.trim_end_matches(',');
    let mut out = trimmed.to_owned();
    while let Some(c) = stack.pop() {
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_object() {
        let v = parse_partial_json(r#"{"a": 1, "b": "two"}"#).unwrap();
        assert_eq!(v["a"], 1);
        assert_eq!(v["b"], "two");
    }

    #[test]
    fn unclosed_object() {
        let v = parse_partial_json(r#"{"a": 1"#).unwrap();
        assert_eq!(v["a"], 1);
    }

    #[test]
    fn unclosed_string_in_value() {
        let v = parse_partial_json(r#"{"a": "hello"#).unwrap();
        assert_eq!(v["a"], "hello");
    }

    #[test]
    fn trailing_comma() {
        let v = parse_partial_json(r#"{"a": 1,"#).unwrap();
        assert_eq!(v["a"], 1);
    }

    #[test]
    fn empty() {
        let v = parse_partial_json("").unwrap();
        assert!(v.is_null());
    }
}
