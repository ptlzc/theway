//! `NotificationHook` adapter that turns server-pushed MCP frames into runtime
//! [`Trigger`](theway_core::Trigger) envelopes.
//!
//! Sits between [`theway_mcp::McpClient`] (RFC 1 §4.2.1 read pump, surfaced via
//! [`theway_mcp::client::McpClient::take_notifications`]) and the runtime's `TriggerSink`. One
//! instance per configured MCP server. Constructed by `mcp_loader` once
//! `RFC 1 sub-PR 2` lands a supervisor that owns hook registration; until then the type
//! exists so unit tests pin the per-method dedup / replacement-policy contract from
//! RFC 1 §4.2.3 and the follow-up notes left on PR #35.
//!
//! Mapping rules (RFC 1 §4.2.3 + PR #35 / PR #56 QA notes):
//!
//! | MCP method                            | runtime idempotency key                          | replacement      |
//! |---------------------------------------|--------------------------------------------------|------------------|
//! | `notifications/tools/listChanged`     | `mcp:{server}:tools`                             | `LatestReplaces` |
//! | `notifications/resources/listChanged` | `mcp:{server}:resources`                         | `LatestReplaces` |
//! | `notifications/resources/updated`     | `mcp:{server}:resources:{safe-uri-or-hash}`      | `LatestReplaces` |
//! | `notifications/prompts/listChanged`   | `mcp:{server}:prompts`                           | `LatestReplaces` |
//! | custom `notifications/*`              | `mcp:{server}:custom:{safe-key-or-hash}`         | `Drop`           |
//!
//! Two layers of namespacing:
//!
//! - **`mcp:{server_name}:` prefix** keeps the same intrinsic key from two MCP servers
//!   (e.g. both `tools/listChanged`) from dedup-cancelling each other in the runtime's
//!   global dedup window (PR #56 QA blocker #1).
//! - **`custom:` segment** keeps user-supplied dedup keys in their own slot within a
//!   server, so a custom notification with `_meta.theway_dedup_key = "tools"` cannot collide
//!   with the built-in `tools/listChanged` row (PR #56 QA blocker #2). Built-in subsystems
//!   (`tools` / `resources` / `prompts`) own the un-prefixed slot; everything user-provided
//!   lives under `custom:`.
//!
//! A custom notification that provides no `_meta.theway_dedup_key` is dropped at the
//! adapter with `dropped_count += 1`; the runtime never sees it. Adapters do **not** dedup
//! themselves — the runtime owns the dedup window. We surface a stable, server-scoped key
//! per source/method so the runtime can do its job.
//!
//! Privacy contract: `payload_visibility = Local` means the full `params` blob is dropped
//! before persistence; only `payload_summary` survives into the audit. The summary is
//! method-name-only for custom / unknown notifications (PR #56 QA blocker) — a sentinel
//! secret tucked into a custom notification's params must never end up in the persisted
//! `Custom { custom_type: "trigger" }` audit entry. Adapters that genuinely need
//! human-readable per-event detail can opt in via `_meta.theway_summary: "<text>"`, capped and
//! redacted before persistence. Unsafe resource URIs / custom dedup keys are hashed before
//! they enter persisted trigger audit fields.

use std::sync::Arc;

use crate::trigger_engine::notification_hook::{
    HookError, HookState, NotificationHook, NotificationHookStatus, TriggerSink,
};
use crate::trigger_engine::types::{
    CredentialScope, PayloadVisibility, ReplacementPolicy, SourceKind, Trigger, TriggerAuthority,
    TriggerSource,
};
use async_trait::async_trait;
use chrono::Utc;
use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use theway_mcp::client::McpServerNotification;
use tokio::sync::mpsc::UnboundedReceiver;
use uuid::Uuid;

/// One MCP server's notification stream as a runtime `NotificationHook`.
///
/// The constructor consumes the `UnboundedReceiver` returned by
/// [`theway_mcp::McpClient::take_notifications`]; the hook owns the receiver for the lifetime
/// of `run`. The supervisor (RFC 1 sub-PR 2) is expected to call `run` exactly once on a
/// dedicated task and to drop the hook on shutdown — there is no re-entrant restart path
/// because each server has its own `McpClient`, and a recovery cycle re-creates the whole
/// stack (client + transport + hook) rather than reusing the inbound receiver.
pub struct McpNotificationHook {
    /// `mcp:<server_name>`. Stable across the hook's lifetime; used in
    /// `NotificationHookStatus.subscription_labels` and `Trigger.source_label`.
    label: String,
    /// Plain server name from `mcp.toml` (e.g. `"filesystem"`), without the `mcp:` prefix.
    /// Threaded into `TriggerSource::Mcp.server_name` so the rule engine can match on it.
    server_name: String,
    /// Receiver of normalized server pushes. `Mutex<Option<...>>` so `run` can `.take()` it
    /// exactly once and the type stays `Send + Sync` even though the receiver itself is
    /// `!Sync`. After the first run, subsequent calls return `HookError::SinkClosed`
    /// because there is nothing left to drain.
    rx: Mutex<Option<UnboundedReceiver<McpServerNotification>>>,
    /// Atomic-cheap status snapshot. Re-read frequently by `/triggers sources`; we keep it
    /// behind `parking_lot::Mutex` (matches the trait's "atomic loads or
    /// `parking_lot::Mutex`" guidance).
    status: Arc<Mutex<NotificationHookStatus>>,
}

impl McpNotificationHook {
    /// Build a hook for the named MCP server. `server_name` is what the user wrote in
    /// `mcp.toml`; `rx` comes from [`theway_mcp::McpClient::take_notifications`].
    pub fn new(
        server_name: impl Into<String>,
        rx: UnboundedReceiver<McpServerNotification>,
    ) -> Self {
        let server_name = server_name.into();
        let label = format!("mcp:{server_name}");
        let mut status = NotificationHookStatus::pending();
        // The hook's only "subscription" is the server itself — MCP push frames are not
        // per-topic.
        status.subscription_labels = vec![label.clone()];
        Self {
            label,
            server_name,
            rx: Mutex::new(Some(rx)),
            status: Arc::new(Mutex::new(status)),
        }
    }

    /// Test-only accessor for assertions on the live status. Production code reads via the
    /// trait method [`NotificationHook::status`] which clones the snapshot.
    #[cfg(test)]
    fn debug_status_handle(&self) -> Arc<Mutex<NotificationHookStatus>> {
        self.status.clone()
    }
}

#[async_trait]
impl NotificationHook for McpNotificationHook {
    fn label(&self) -> &str {
        &self.label
    }

    async fn run(&self, sink: TriggerSink) -> Result<(), HookError> {
        let mut rx = self.rx.lock().take().ok_or_else(|| {
            HookError::Other(format!(
                "{} hook already ran; receiver consumed",
                self.label
            ))
        })?;

        // First successful receiver checkout flips the state to Connected — the read pump
        // ran the JSON-RPC initialize handshake before constructing this hook, so by the
        // time we get here the transport is live.
        self.status.lock().state = HookState::Connected;

        while let Some(notification) = rx.recv().await {
            let trigger = match map_notification(&self.server_name, &notification) {
                Some(t) => t,
                None => {
                    // Custom notification without a dedup key — drop and surface count.
                    let mut st = self.status.lock();
                    st.dropped_count = st.dropped_count.saturating_add(1);
                    st.last_error = Some(format!(
                        "dropped custom notification {:?}: missing `_meta.theway_dedup_key`",
                        notification.method
                    ));
                    continue;
                }
            };
            if sink.send(trigger).is_err() {
                // Runtime is shutting down; exit cleanly. The supervisor will reap the
                // hook task and mark the hook Disconnected.
                self.status.lock().state = HookState::Disconnected {
                    reason: "sink closed".into(),
                };
                return Err(HookError::SinkClosed);
            }
            // Bookkeeping after successful push so `/triggers sources` shows the latest event
            // even if the runtime is still draining the sink.
            let mut st = self.status.lock();
            st.last_event_at = Some(Utc::now());
            st.last_error = None;
        }

        // Pump exited because the transport closed. Update status and return cleanly so
        // the supervisor records a Disconnected hook rather than a hard failure.
        self.status.lock().state = HookState::Disconnected {
            reason: "mcp transport closed".into(),
        };
        Ok(())
    }

    fn status(&self) -> NotificationHookStatus {
        self.status.lock().clone()
    }
}

/// Translate one MCP push frame to a `Trigger`, or `None` if the frame should be dropped at
/// the adapter (custom method without `_meta.theway_dedup_key`).
///
/// Pure function so the test suite can pin every row of the §4.2.3 table without spinning
/// up a real `McpClient`.
fn map_notification(server_name: &str, n: &McpServerNotification) -> Option<Trigger> {
    let (idempotency_key, replacement_policy) = idempotency_for(server_name, &n.method, &n.params)?;
    let payload_summary = render_summary(&n.method, &n.params);
    Some(Trigger {
        source: TriggerSource::Mcp {
            server_name: server_name.to_string(),
            method: n.method.clone(),
        },
        source_kind: SourceKind::Mcp,
        source_label: format!("mcp:{server_name}"),
        event_label: n.method.clone(),
        payload_visibility: PayloadVisibility::Local,
        payload_summary,
        payload: None,
        idempotency_key,
        replacement_policy,
        trace_id: Uuid::new_v4().to_string(),
        authority: TriggerAuthority {
            // Stable principal id per server — the user-visible server name acts as the
            // opaque-stable id since `mcp.toml` enforces uniqueness.
            principal_id: format!("mcp:{server_name}"),
            principal_label: server_name.to_string(),
            credential_scope: CredentialScope::User,
            allowed_source_actions: Vec::new(),
            expires_at: None,
        },
        received_at: Utc::now(),
    })
}

pub(crate) fn safe_display(value: &str, cap: usize) -> String {
    let redacted = redact_notification_text(value).replace('\n', " ");
    truncate_chars(&redacted, cap)
}

fn redact_notification_text(value: &str) -> String {
    value
        .split_whitespace()
        .map(|part| {
            let lower = part.to_ascii_lowercase();
            if lower.starts_with("hub_agent_")
                || lower.starts_with("hub_hs_")
                || lower.starts_with("hub_ep_")
                || lower.starts_with("sk-")
                || lower.contains("bearer")
                || lower.contains("token")
            {
                "[redacted]"
            } else {
                part
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn truncate_chars(value: &str, cap: usize) -> String {
    if value.chars().count() <= cap {
        return value.to_string();
    }
    let mut out = value
        .chars()
        .take(cap.saturating_sub(1))
        .collect::<String>();
    out.push('…');
    out
}

/// Derive `(idempotency_key, replacement_policy)` for a given method + params per RFC 1
/// §4.2.3 / PR #35 QA follow-up. Returns `None` for custom methods that don't supply a
/// dedup key — the caller drops those at the adapter with diagnostics.
///
/// Every key is namespaced with `mcp:{server_name}:` so two MCP servers that legitimately
/// emit the same intrinsic key (both `tools/listChanged`, both with the same custom
/// `_meta.theway_dedup_key`) do not dedup each other in the runtime. The runtime dedup window
/// is global per harness; namespacing at the adapter is the only place we can prevent
/// cross-server collisions.
fn idempotency_for(
    server_name: &str,
    method: &str,
    params: &serde_json::Value,
) -> Option<(String, ReplacementPolicy)> {
    let prefix = format!("mcp:{server_name}:");
    match method {
        "notifications/tools/listChanged" => {
            Some((format!("{prefix}tools"), ReplacementPolicy::LatestReplaces))
        }
        "notifications/resources/listChanged" => Some((
            format!("{prefix}resources"),
            ReplacementPolicy::LatestReplaces,
        )),
        "notifications/prompts/listChanged" => Some((
            format!("{prefix}prompts"),
            ReplacementPolicy::LatestReplaces,
        )),
        "notifications/resources/updated" => {
            // Per-URI keying so multiple updates to different resources don't collapse into
            // one event. If the server omitted `uri` (shouldn't happen per MCP spec but
            // defensive), fall back to the unscoped `"resources"` key.
            let uri = params
                .get("uri")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            Some((
                format!("{prefix}resources:{}", safe_idempotency_segment(uri)),
                ReplacementPolicy::LatestReplaces,
            ))
        }
        _ => {
            // Custom notification — require an explicit dedup key in the canonical
            // `_meta.theway_dedup_key` location. Every explicit key is treated as `Drop`
            // semantics: one logical event per key, no replacement.
            //
            // The `custom:` segment after the server prefix keeps custom keys in their
            // own namespace within the server so a user supplying
            // `_meta.theway_dedup_key = "tools"` does NOT collide with the built-in
            // `tools/listChanged` row. Built-in subsystems (`tools` / `resources` /
            // `prompts`) own the un-prefixed slot; everything user-provided lives under
            // `custom:`. PR #56 QA re-review blocker.
            extract_dedup_key(params).map(|k| {
                (
                    format!("{prefix}custom:{}", safe_idempotency_segment(&k)),
                    ReplacementPolicy::Drop,
                )
            })
        }
    }
}

/// Pull a dedup key out of a custom notification's params, reading only the canonical
/// `_meta.theway_dedup_key` location.
fn extract_dedup_key(params: &serde_json::Value) -> Option<String> {
    params
        .get("_meta")
        .and_then(|m| m.get("theway_dedup_key"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

pub(crate) fn safe_idempotency_segment(value: &str) -> String {
    let redacted = redact_notification_text(value);
    let has_sensitive_text = redacted != value;
    let is_unbounded = value.chars().count() > 200;
    let has_control_chars = value.chars().any(|ch| ch.is_control());
    if has_sensitive_text || is_unbounded || has_control_chars {
        let digest = Sha256::digest(value.as_bytes());
        return format!("hash:{}", hex::encode(&digest[..6]));
    }
    value.to_string()
}

/// Render a short human-readable summary for `payload_summary`. Capped well below the
/// runtime 4 KiB persistence cap; the runtime will still re-truncate if a future caller
/// emits more.
///
/// Privacy contract (RFC 0 §3.2.2 / RFC 1 §4.2.3 + PR #56 QA follow-up): the hook is
/// configured with `payload_visibility = Local`, which means `payload` is dropped and only
/// `payload_summary` survives into the persisted audit. So this function must not echo
/// arbitrary params content — a sentinel secret in a custom notification's params field
/// would otherwise persist into the trigger audit entry.
///
/// The contract: only **method name** plus bounded/redacted display metadata (`uri` for
/// `resources/updated`) appear in the summary. Adapters that need per-event detail must opt
/// in via `_meta.theway_summary: "<human-safe text>"`; we still cap and redact that string
/// because it is MCP-server-controlled input.
fn render_summary(method: &str, params: &serde_json::Value) -> Option<String> {
    match method {
        // `uri` is part of the MCP resource identity, but an MCP server can still stuff
        // token-like values into it. Keep useful display metadata while applying the same
        // redaction as other user-visible/audit-visible strings.
        "notifications/resources/updated" => {
            if let Some(uri) = params.get("uri").and_then(|v| v.as_str()) {
                Some(format!("{method} uri={}", safe_display(uri, 200)))
            } else {
                Some(method.to_string())
            }
        }
        // Standard listChanged events have no per-event detail worth rendering.
        "notifications/tools/listChanged"
        | "notifications/resources/listChanged"
        | "notifications/prompts/listChanged" => Some(method.to_string()),
        // Custom / unknown methods: NEVER serialize arbitrary params. Allow explicit
        // opt-in via `_meta.theway_summary`; otherwise just the method name. This is what
        // prevents secrets in a server's custom params from leaking into the audit.
        _ => {
            if let Some(s) = params
                .get("_meta")
                .and_then(|m| m.get("theway_summary"))
                .and_then(|v| v.as_str())
            {
                Some(format!("{method} {}", safe_display(s, 200)))
            } else {
                Some(method.to_string())
            }
        }
    }
}

#[cfg(test)]
// Test files live in `tests/triggers/mcp_notification_hook/` (mirror of src), pulled in by
// path so they keep unit-test semantics (private access). See docs/rust-test-files.md.
tests_bridge_macro::tests_bridge!("triggers/mcp_notification_hook");
