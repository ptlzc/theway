//! shared client contract (not protocol) — zone per the crate-level "Module zones" doc.
//! Bug-report support shared with clients: the secret redactor
//! (daemon-kernel-layers: moved from the SDK into transport — the TUI applies
//! it before rendering, the daemon before writing dumps).
//!
//! Bug reports are the canonical "give me something to attach to an issue"
//! artifact, so we trade detail for safety: the redactor is conservative —
//! every well-known secret pattern is replaced with a fixed placeholder that
//! names which class of secret was caught, so the user can verify which rules
//! fired without leaking detail.
//!
//! The full `/bug-report` builder (diagnostic snapshot + log tail + transcript
//! render) lives in the daemon crate (`theway_daemon::bug_report`); it applies
//! this redactor before writing the dump.

use std::sync::LazyLock;

use regex::Regex;

/// Apply every secret-pattern regex to `input`. Each match is replaced with a fixed
/// placeholder that names which class of secret was caught so the user can verify which
/// rules fired without leaking detail.
pub fn redact(input: &str) -> String {
    let mut out = input.to_string();
    for (label, re) in REDACTORS.iter() {
        out = re
            .replace_all(&out, format!("[REDACTED:{label}]"))
            .into_owned();
    }
    out
}

static REDACTORS: LazyLock<Vec<(&'static str, Regex)>> = LazyLock::new(|| {
    let raw: Vec<(&'static str, &'static str)> = vec![
        // OpenAI / Anthropic / Stripe-style keys ("sk-..." prefix, 20+ alnum after).
        ("openai_anthropic_key", r"sk-[A-Za-z0-9_-]{20,}"),
        // AWS access key id (always 20 chars, AKIA or ASIA prefix).
        ("aws_access_key", r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b"),
        // GitHub PATs (40 chars after `gho_` / `ghp_` / `ghu_` / `ghs_`).
        ("github_token", r"\bgh[ousp]_[A-Za-z0-9]{30,}\b"),
        // Slack tokens.
        ("slack_token", r"\bxox[abprs]-[A-Za-z0-9-]{10,}\b"),
        // Google API keys (39 chars after AIza).
        ("google_api_key", r"\bAIza[0-9A-Za-z_-]{35}\b"),
        // Generic Bearer tokens in HTTP-style strings.
        ("bearer_token", r"Bearer\s+[A-Za-z0-9._\-]{16,}"),
        // Hub browser login and loopback callback URLs can carry auth state or one-time codes.
        ("theway_hub_login_url", r"https?://[^\s]+/login\?[^\s]+"),
        (
            "theway_hub_callback_url",
            r"http://127\.0\.0\.1:[0-9]+/callback(?:\?[^\s]+)?",
        ),
        // theway hub session / agent credentials can appear as bare values in transport errors.
        (
            "theway_hub_token",
            r"\bhub_(?:agent|hs)_[A-Za-z0-9._\-]{8,}\b",
        ),
        // Hub/user-visible diagnostics should not expose raw immutable IDs.
        (
            "uuid",
            r"\b[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}\b",
        ),
    ];
    raw.into_iter()
        .map(|(label, src)| (label, Regex::new(src).expect("regex must compile")))
        .collect()
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_known_patterns() {
        let s = "key=sk-abcdefghij1234567890abcd , aws=AKIAEXAMPLEEXAMPLE1A, gh=gho_abcdefghijklmnopqrstuvwxyz0123456789, slack=xoxb-1234567890-abcdef, header=Authorization: Bearer eyJabc.defghijklmnopqr, login=https://pie.0xfefe.me/login?req=018fe23a-1111-4a22-8b33-123456789abc&state=state_secret, callback=http://127.0.0.1:49152/callback?code=hub_code_secret&state=state_secret, hub=hub_agent_abcdefghijklmnopqrstuvwxyz, session=hub_hs_abcdefghijklmnopqrstuvwxyz, id=018fe23a-1111-4a22-8b33-123456789abc";
        let r = redact(s);
        assert!(!r.contains("sk-abcdefghij"), "openai key leaked: {r}");
        assert!(!r.contains("AKIAEXAMPLE"), "aws key leaked: {r}");
        assert!(!r.contains("gho_"), "github token leaked: {r}");
        assert!(!r.contains("xoxb-"), "slack token leaked: {r}");
        assert!(!r.contains("eyJabc.defghijklmnopqr"), "bearer leaked: {r}");
        assert!(
            !r.contains("pie.0xfefe.me/login"),
            "hub login URL leaked: {r}"
        );
        assert!(
            !r.contains("127.0.0.1:49152/callback"),
            "hub callback URL leaked: {r}"
        );
        assert!(!r.contains("hub_agent_"), "hub agent token leaked: {r}");
        assert!(!r.contains("hub_hs_"), "hub session token leaked: {r}");
        assert!(!r.contains("018fe23a-1111"), "uuid leaked: {r}");
        assert!(r.contains("[REDACTED:openai_anthropic_key]"));
        assert!(r.contains("[REDACTED:aws_access_key]"));
        assert!(r.contains("[REDACTED:theway_hub_login_url]"));
        assert!(r.contains("[REDACTED:theway_hub_callback_url]"));
        assert!(r.contains("[REDACTED:theway_hub_token]"));
        assert!(redact("id=018fe23a-1111-4a22-8b33-123456789abc").contains("[REDACTED:uuid]"));
    }

    #[test]
    fn redact_leaves_normal_text_alone() {
        let s = "hello world, no secrets here";
        assert_eq!(redact(s), s);
    }
}
