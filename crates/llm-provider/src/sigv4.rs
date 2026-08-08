//! AWS Signature Version 4 (SigV4) signer. Hand-rolled per
//! <https://docs.aws.amazon.com/general/latest/gr/sigv4_signing.html>; no aws-sdk dep.
//!
//! Used by the Bedrock provider (c4pt0r/theway#14) to sign POSTs to bedrock-runtime. The shape
//! is provider-agnostic: any AWS service that uses standard SigV4 over HTTPS works.

use std::collections::BTreeMap;

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

/// Inputs for one signing pass. All strings are caller-owned to keep the signer pure.
pub struct SigningRequest<'a> {
    pub method: &'a str,
    pub url: &'a url::Url,
    /// Caller-provided headers. The signer adds `host`, `x-amz-date`, and (if needed)
    /// `x-amz-content-sha256`. Header names are lower-cased internally.
    pub headers: &'a [(&'a str, &'a str)],
    pub payload: &'a [u8],
    pub region: &'a str,
    pub service: &'a str,
    pub access_key: &'a str,
    pub secret_key: &'a str,
    /// Optional STS session token (for temporary creds). When set, included as
    /// `x-amz-security-token` and added to SignedHeaders.
    pub session_token: Option<&'a str>,
    /// `YYYYMMDDThhmmssZ` — caller supplies so tests can pin to fixed timestamps.
    pub amz_date: &'a str,
}

/// Output: the `Authorization` header value plus any synthesized headers the caller must
/// attach to the outgoing request.
#[derive(Debug, Clone)]
pub struct SignedRequest {
    pub authorization: String,
    pub headers: Vec<(String, String)>,
}

pub fn sign(req: &SigningRequest<'_>) -> SignedRequest {
    let date = &req.amz_date[..8]; // YYYYMMDD
    let payload_hash = hex::encode(Sha256::digest(req.payload));

    // Compose all headers we'll sign. Caller headers + host + x-amz-date + content-hash +
    // optional security-token, lower-cased, sorted, trimmed.
    let mut header_map: BTreeMap<String, String> = BTreeMap::new();
    let host = host_with_port(req.url);
    header_map.insert("host".into(), host);
    header_map.insert("x-amz-date".into(), req.amz_date.to_string());
    header_map.insert("x-amz-content-sha256".into(), payload_hash.clone());
    if let Some(tok) = req.session_token {
        header_map.insert("x-amz-security-token".into(), tok.to_string());
    }
    for (k, v) in req.headers {
        header_map.insert(k.to_ascii_lowercase(), trim_collapse(v));
    }

    let canonical_headers: String = header_map
        .iter()
        .map(|(k, v)| format!("{k}:{v}\n"))
        .collect();
    let signed_headers = header_map.keys().cloned().collect::<Vec<_>>().join(";");

    let canonical_uri = canonical_uri(req.url);
    let canonical_query = canonical_query(req.url);
    let canonical_request = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        req.method, canonical_uri, canonical_query, canonical_headers, signed_headers, payload_hash
    );

    let scope = format!("{date}/{}/{}/aws4_request", req.region, req.service);
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{}\n{}\n{}",
        req.amz_date,
        scope,
        hex::encode(Sha256::digest(canonical_request.as_bytes()))
    );

    let k_date = hmac(
        format!("AWS4{}", req.secret_key).as_bytes(),
        date.as_bytes(),
    );
    let k_region = hmac(&k_date, req.region.as_bytes());
    let k_service = hmac(&k_region, req.service.as_bytes());
    let k_signing = hmac(&k_service, b"aws4_request");
    let signature = hex::encode(hmac(&k_signing, string_to_sign.as_bytes()));

    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={signature}",
        req.access_key
    );

    // Hand back every header we generated so the caller doesn't have to recompute them.
    let mut synthesized = vec![
        ("x-amz-date".to_string(), req.amz_date.to_string()),
        ("x-amz-content-sha256".to_string(), payload_hash),
    ];
    if let Some(tok) = req.session_token {
        synthesized.push(("x-amz-security-token".to_string(), tok.to_string()));
    }
    SignedRequest {
        authorization,
        headers: synthesized,
    }
}

fn host_with_port(url: &url::Url) -> String {
    let host = url.host_str().unwrap_or("").to_string();
    match (url.port(), url.scheme()) {
        (Some(p), "http") if p == 80 => host,
        (Some(p), "https") if p == 443 => host,
        (Some(p), _) => format!("{host}:{p}"),
        (None, _) => host,
    }
}

fn canonical_uri(url: &url::Url) -> String {
    let path = url.path();
    if path.is_empty() {
        "/".into()
    } else {
        // Each path segment is URL-encoded once. AWS S3 wants double-encoding but Bedrock
        // (and most services) use single-encoding.
        let mut out = String::with_capacity(path.len());
        for (i, seg) in path.split('/').enumerate() {
            if i > 0 {
                out.push('/');
            }
            for b in seg.bytes() {
                if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
                    out.push(b as char);
                } else {
                    out.push_str(&format!("%{:02X}", b));
                }
            }
        }
        out
    }
}

fn canonical_query(url: &url::Url) -> String {
    let mut pairs: Vec<(String, String)> = url
        .query_pairs()
        .map(|(k, v)| (encode_strict(&k), encode_strict(&v)))
        .collect();
    pairs.sort();
    pairs
        .into_iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&")
}

fn encode_strict(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}

fn trim_collapse(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    let mut last_space = false;
    for c in v.trim().chars() {
        if c == ' ' || c == '\t' {
            if !last_space {
                out.push(' ');
                last_space = true;
            }
        } else {
            out.push(c);
            last_space = false;
        }
    }
    out
}

fn hmac(key: &[u8], message: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("hmac key");
    mac.update(message);
    mac.finalize().into_bytes().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Signing is deterministic: same inputs → same signature. The actual signature is
    /// validated end-to-end when AWS accepts the request from the Bedrock provider; here we
    /// pin the algorithm by asserting determinism + shape of the Authorization header.
    #[test]
    fn signing_is_deterministic_and_well_formed() {
        let url = url::Url::parse("https://example.amazonaws.com/").unwrap();
        let req = SigningRequest {
            method: "GET",
            url: &url,
            headers: &[],
            payload: b"",
            region: "us-east-1",
            service: "service",
            access_key: "AKIDEXAMPLE",
            secret_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
            session_token: None,
            amz_date: "20150830T123600Z",
        };
        let a = sign(&req);
        let b = sign(&req);
        assert_eq!(a.authorization, b.authorization);
        assert!(a.authorization.starts_with(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, "
        ));
        assert!(
            a.authorization
                .contains("SignedHeaders=host;x-amz-content-sha256;x-amz-date,")
        );
        // Signature is 64 hex chars.
        let sig = a.authorization.rsplit("Signature=").next().unwrap();
        assert_eq!(sig.len(), 64);
        assert!(sig.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn signs_post_with_payload_and_session_token() {
        let url = url::Url::parse(
            "https://bedrock-runtime.us-east-1.amazonaws.com/model/anthropic.claude/invoke",
        )
        .unwrap();
        let req = SigningRequest {
            method: "POST",
            url: &url,
            headers: &[("content-type", "application/json")],
            payload: br#"{"messages":[]}"#,
            region: "us-east-1",
            service: "bedrock",
            access_key: "AKIATEST",
            secret_key: "secret",
            session_token: Some("token123"),
            amz_date: "20250101T000000Z",
        };
        let signed = sign(&req);
        assert!(signed.authorization.starts_with(
            "AWS4-HMAC-SHA256 Credential=AKIATEST/20250101/us-east-1/bedrock/aws4_request"
        ));
        assert!(signed.authorization.contains(
            "SignedHeaders=content-type;host;x-amz-content-sha256;x-amz-date;x-amz-security-token"
        ));
        // Generated headers are returned verbatim.
        assert!(
            signed
                .headers
                .iter()
                .any(|(k, v)| k == "x-amz-date" && v == "20250101T000000Z")
        );
        assert!(
            signed
                .headers
                .iter()
                .any(|(k, _v)| k == "x-amz-content-sha256")
        );
        assert!(
            signed
                .headers
                .iter()
                .any(|(k, v)| k == "x-amz-security-token" && v == "token123")
        );
    }

    #[test]
    fn canonical_query_is_sorted_and_encoded() {
        let url = url::Url::parse("https://x.example.com/?b=2&a=hello%20world").unwrap();
        let q = canonical_query(&url);
        assert_eq!(q, "a=hello%20world&b=2");
    }

    #[test]
    fn trim_collapse_normalizes_whitespace() {
        assert_eq!(trim_collapse("  foo   bar  "), "foo bar");
        assert_eq!(trim_collapse("single"), "single");
    }
}
