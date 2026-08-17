//! RFC 1 (issue #20) trigger envelope, source taxonomy, authority, state machine, and the
//! `TriggerRecord` persisted as `SessionTreeEntry::Custom { custom_type: "trigger" }`.
//!
//! Moved out of theway-core into the CLI host (`trigger_engine`): the core runtime only
//! maintains state and exposes the agent loop; external-event-driven invocation — the
//! envelope types, dedup/cycle runtime, permission hooks, sub-agent execution and result
//! promotion — is a host-level concern. The host consumes core's public API
//! (`Session::append_custom`, `Agent::prompt`, harness events) to act on the core state.
//!
//! Transport adapters (MCP push, cron, file-watch, webhook) live in `crates/server/src/
//! triggers` and consume the [`NotificationHook`](super::notification_hook) trait.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// The runtime-facing envelope for a single external event. Constructed by an upstream
/// adapter (typically inside `crates/harness::triggers`) and handed to
/// `AgentHarness::handle_trigger(...)`. Once accepted, the runtime persists a
/// [`TriggerRecord`] derived from this envelope.
///
/// `Trigger` is the boundary type between transport-specific source adapters (which know
/// about webhooks, MCP push frames, WebSocket frames, etc.) and the runtime. Adding new
/// fields here is additive — readers must tolerate unknown fields per
/// [`TriggerRecord::SCHEMA_VERSION`] strategy.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Trigger {
    /// Typed source descriptor. Lets the rule engine match on adapter family + adapter id.
    pub source: TriggerSource,
    /// First-class display dimension. UI groups `/triggers` by this.
    pub source_kind: SourceKind,
    /// Human-readable source label supplied by the adapter (e.g. "MCP filesystem").
    pub source_label: String,
    /// Human-readable event label supplied by the adapter (e.g. "file changed", "pr merged").
    pub event_label: String,
    /// Default-`Local`: only `payload_summary` carries data to the runtime; full `payload`
    /// is `null`. Sources opt into `Shared` per RFC 0 §2.2.1 / RFC 1 §2.2 #1; `Redacted`
    /// forces `payload = null` regardless.
    pub payload_visibility: PayloadVisibility,
    /// Truncated human-readable summary; bounded by the runtime persist cap (4 KiB).
    pub payload_summary: Option<String>,
    /// Source-specific full payload. Default `None` (envelope-only). The runtime always
    /// truncates to `payload_summary` before persistence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
    /// Required: dedup key. Runtime drops events with a duplicate key within the
    /// configured dedup window (default 5 minutes).
    pub idempotency_key: String,
    /// How the dedup window collapses repeat events sharing this `idempotency_key`. Sources
    /// declare per-event policy (RFC 1 §5 open decision #4 / §11). Required field — the
    /// runtime does **not** default to `Drop` on missing field at deserialize time so an
    /// adapter that forgot to set it surfaces immediately rather than silently dropping
    /// real events. Adapters that want "no replacement" semantics set [`ReplacementPolicy::Drop`]
    /// explicitly.
    pub replacement_policy: ReplacementPolicy,
    /// Audit lineage. The same `trace_id` propagates to follow-up triggers spawned by the
    /// agent so cycle suppression can fire after a configurable hop count.
    pub trace_id: String,
    /// Authority claim made by the source. The runtime treats this as an audit summary and
    /// an input to the permission evaluator, NOT as proof that the action is authorized.
    /// See RFC 1 §2.3 + RFC 4 §4 for the source-vs-action authority separation.
    pub authority: TriggerAuthority,
    /// When the runtime received the trigger (set by the adapter before sinking).
    pub received_at: DateTime<Utc>,
}

/// Typed source descriptor. Each variant carries enough information for the rule engine to
/// distinguish triggers from different upstream systems without parsing strings.
///
/// Adding a new variant is additive and only needs to be tagged `#[serde(rename_all = ...)]`
/// to keep wire-stable.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TriggerSource {
    /// Notification pushed by an MCP server (per RFC 1 §4.2).
    Mcp { server_name: String, method: String },
    /// Locally fired event (cron / file-watch / agent self-trigger). Tools-MCP-Lead's
    /// adapter taxonomy in RFC 4 §2.1 uses concrete `subkind`s; the runtime envelope only
    /// needs a generic carrier. (`subkind` rather than `kind` because the enum is
    /// `serde(tag = "kind")` and reserves the latter for the discriminator.)
    Local { subkind: String },
    /// An action emitted by another agent in a multi-agent topology (placeholder for
    /// RFC 2 — runtime accepts the variant today but no rule engine consumes it yet).
    AgentDelegate {
        agent_id: String,
        delegation_id: String,
    },
}

/// UI grouping dimension. `/triggers --source <kind>` filters on this.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Local,
    Mcp,
}

/// Privacy tier for the carried payload. Enforced by the runtime when persisting and by
/// adapters when rendering.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PayloadVisibility {
    /// `payload` is `None`. Only `payload_summary` is available to consumers. Default.
    Local,
    /// `payload` may be `Some(...)`. Runtime still truncates to `payload_summary` for
    /// persistence (4 KiB cap).
    Shared,
    /// `payload` is forced to `None` and `payload_summary` must be de-identified.
    Redacted,
}

/// Audit / authorization summary attached to every trigger. Token material is **never**
/// stored here. `principal_id` is opaque-stable (ULID-style); `principal_label` is for
/// display only; `credential_scope` is the source's declared scope, never used as a secret
/// lookup key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriggerAuthority {
    pub principal_id: String,
    pub principal_label: String,
    pub credential_scope: CredentialScope,
    /// Adapter-declared subset of actions the source's credential is scoped for (e.g.
    /// `["read", "comment"]` for a GitHub installation). The runtime permission evaluator
    /// MAY intersect this with the local policy when deciding whether to execute a tool
    /// call.
    #[serde(default)]
    pub allowed_source_actions: Vec<String>,
    /// Source-stated expiry (for short-lived source credentials). Optional; runtime does not act
    /// on it directly — adapters refresh tokens themselves.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
}

/// How the runtime dedup window collapses repeat events sharing the same
/// `idempotency_key`. Declared per-event by the source adapter; the runtime applies the
/// declared policy when it sees a duplicate within the dedup window (default 5 minutes per
/// RFC 1 §5).
///
/// RFC 1 §5 + RFC 1 §11 open decision #4: the field is **required** on the wire — the
/// runtime does not coerce a missing field into `Drop` so adapters that forgot to set it
/// fail loud at deserialize time. Adapters that want "ignore subsequent duplicates"
/// semantics set [`Self::Drop`] explicitly.
///
/// Recommended choice per source family:
/// - MCP `notifications/tools/listChanged` / `notifications/resources/listChanged` →
///   [`Self::LatestReplaces`] (the latest catalog snapshot supersedes earlier ones).
/// - MCP `notifications/resources/updated` per resource URI → [`Self::LatestReplaces`].
/// - Custom MCP notifications without a `_meta.theway_dedup_key` agreement → [`Self::Drop`].
/// - Webhook-style events where every occurrence matters (e.g. PR comments) →
///   [`Self::Drop`] keyed by a per-event id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplacementPolicy {
    /// Replace the in-flight / queued trigger with the latest occurrence. Useful for
    /// "snapshot of current state" events.
    LatestReplaces,
    /// Combine duplicates into one trigger, preserving merged context for the rule layer.
    /// The runtime treats this identically to [`Self::LatestReplaces`] for v1 (audit
    /// records both arrivals); future RFC 4 rule actions may use the distinction.
    Coalesce,
    /// Drop duplicate occurrences during the dedup window; only the first event in the
    /// window fires the rule. Default for sources that did not explicitly opt in.
    Drop,
}

/// Audit/authorization summary enum shared with provider/auth credential resolution. v1
/// values are part of the credential-scope contract. The runtime treats this as opaque and
/// passes it through to the evaluator and the session audit record.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum CredentialScope {
    User,
    Project,
    Team,
    Agent,
    None,
}

/// Lifecycle state of a single trigger as it moves through the runtime state machine. Maps
/// 1:1 to the RFC 0 5-stage ack lifecycle, plus the runtime-only `received` / `accepted` /
/// `deduped` / `cycle_suppressed` / `permission_denied` / `needs_approval` / `running` /
/// `failed` / `completed` set from RFC 1 §2.7.
///
/// `received`, `accepted`, and `running` are transitional; the rest are terminal for the
/// purposes of `TriggerRecord.state`. See [`Self::is_terminal`] for the canonical predicate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerState {
    /// Frame schema OK + entered local dedup queue. Audit not yet persisted.
    Received,
    /// Dedup pass + permission `Allow` or `Prompt` + audit persisted.
    Accepted,
    /// Same `idempotency_key` already seen within the dedup window.
    Deduped,
    /// Same `trace_id` exceeded the cycle hop cap.
    CycleSuppressed,
    /// Permission evaluator returned `Deny`. Terminal, unrecoverable except via policy.
    PermissionDenied,
    /// Permission evaluator returned `Prompt`. Soft terminal — UI offers replay.
    NeedsApproval,
    /// Agent loop is currently executing the action. Transitional.
    Running,
    /// Agent loop or persistence failed mid-execution. Terminal.
    Failed,
    /// Action completed normally. Terminal.
    Completed,
}

impl TriggerState {
    /// `true` when the state is one a consumer can wait on without more transitions.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Deduped
                | Self::CycleSuppressed
                | Self::PermissionDenied
                | Self::NeedsApproval
                | Self::Failed
                | Self::Completed
        )
    }
}

/// Persistent audit record written under `SessionTreeEntry::Custom { custom_type: "trigger" }`
/// per RFC 1 §2.6. Schema is additive-only inside `SCHEMA_VERSION = 1`; breaking changes
/// bump to v2 with a parallel deserializer.
///
/// **Never** contains raw token material. `authority` is the summary attached to the
/// trigger, not a credential. `payload_summary` is truncated and bounded.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriggerRecord {
    /// Frozen at v=1 for the first runtime release. New optional fields are tolerated by
    /// older readers; breaking changes increment this and gain a parallel v=2 deserializer.
    pub schema_version: u32,
    pub source: TriggerSource,
    pub source_kind: SourceKind,
    pub source_label: String,
    pub event_label: String,
    pub trace_id: String,
    pub authority: TriggerAuthority,
    pub idempotency_key: String,
    pub replacement_policy: ReplacementPolicy,
    pub received_at: DateTime<Utc>,
    pub state: TriggerState,
    pub payload_visibility: PayloadVisibility,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload_summary: Option<String>,
    /// Snapshot of the evaluator decision (Allow / Deny { reason } / Prompt { ... }) at the
    /// moment the trigger was admitted. Opaque JSON so the evaluator schema can evolve
    /// without breaking the audit record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evaluator_decision: Option<serde_json::Value>,
    /// Opaque local id pointing to the follow-up `SessionTreeEntry::Message` produced by
    /// handling this trigger. Filled after the agent loop finalises.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_link: Option<String>,
    /// Set by future RFC 4 work to associate the trigger with the rule that fired. The
    /// runtime persists whatever the caller passes; rule attribution is an upstream concern.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule_name: Option<String>,
}

impl TriggerRecord {
    /// Current schema version. Bump only on breaking changes.
    pub const SCHEMA_VERSION: u32 = 1;

    /// Construct an in-progress record from a `Trigger`. The runtime fills `state` /
    /// `evaluator_decision` / `result_link` as the trigger advances; this helper produces
    /// the initial `Received` snapshot suitable for the first persistence step.
    pub fn received_from(trigger: &Trigger) -> Self {
        Self {
            schema_version: Self::SCHEMA_VERSION,
            source: trigger.source.clone(),
            source_kind: trigger.source_kind,
            source_label: trigger.source_label.clone(),
            event_label: trigger.event_label.clone(),
            trace_id: trigger.trace_id.clone(),
            authority: trigger.authority.clone(),
            idempotency_key: trigger.idempotency_key.clone(),
            replacement_policy: trigger.replacement_policy,
            received_at: trigger.received_at,
            state: TriggerState::Received,
            payload_visibility: trigger.payload_visibility,
            payload_summary: trigger.payload_summary.clone(),
            evaluator_decision: None,
            result_link: None,
            rule_name: None,
        }
    }

    /// `custom_type` tag the runtime uses when writing this record under
    /// `SessionTreeEntry::Custom`. Stable identifier for downstream readers.
    pub const CUSTOM_TYPE: &'static str = "trigger";
}

#[cfg(test)]
// Test files live in `tests/trigger_engine/types/` (mirror of src), pulled in by
// path so they keep unit-test semantics (private access). See docs/rust-test-files.md.
tests_bridge_macro::tests_bridge!("trigger_engine/types");
