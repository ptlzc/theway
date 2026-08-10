//! Hook side of dynamic triggers: the periodic check hook that turns enabled rules into
//! runtime `Trigger` envelopes, the before-action hooks that build the evaluator prompt
//! (including the MCP direct-inject wrapper), the fire-once listener, and prompt
//! rendering.

use std::sync::Arc;

use crate::trigger_engine::event::{TriggerEvent, TriggerListener};
use crate::trigger_engine::execution::{
    BeforeTriggerActionContext, BeforeTriggerActionHook, PromoteAction, TriggerAction,
    TriggerDelivery,
};
use crate::trigger_engine::notification_hook::{
    HookError, HookState, NotificationHook, NotificationHookStatus, TriggerSink,
};
use crate::trigger_engine::types::{
    CredentialScope, PayloadVisibility, ReplacementPolicy, SourceKind, Trigger, TriggerAuthority,
    TriggerSource,
};
use async_trait::async_trait;
use chrono::{Local, Utc};
use parking_lot::Mutex;
use tokio::time::{Duration, MissedTickBehavior};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::{DynamicTriggerRegistry, DynamicTriggerRule, dynamic_trigger_poll_interval_secs};

pub struct DynamicTriggerCheckHook {
    registry: DynamicTriggerRegistry,
    interval: Duration,
    status: Arc<Mutex<NotificationHookStatus>>,
}

impl DynamicTriggerCheckHook {
    pub fn new(registry: DynamicTriggerRegistry) -> Self {
        Self::with_interval(
            registry,
            Duration::from_secs(dynamic_trigger_poll_interval_secs()),
        )
    }

    pub fn with_interval(registry: DynamicTriggerRegistry, interval: Duration) -> Self {
        let mut status = NotificationHookStatus::pending();
        status.subscription_labels = vec!["dynamic trigger periodic check".into()];
        Self {
            registry,
            interval,
            status: Arc::new(Mutex::new(status)),
        }
    }

    fn build_trigger(&self, rule_count: usize) -> Trigger {
        let now_utc = Utc::now();
        let now_local = Local::now();
        let current_dir = std::env::current_dir()
            .ok()
            .map(|path| path.display().to_string());
        // RFC 0 §3.2.2 / RFC 1 §4.2.3: when `payload_visibility = Local`, consumers see
        // only `payload_summary`. Folding the context fields into the summary instead of
        // putting them in `payload` keeps the envelope internally consistent — the
        // sub-agent prompt renderer drops `payload` for Local sources, so anything in
        // `payload` here would never be visible to the evaluator anyway. The dynamic
        // check's needs (cwd + clock + rule count) all fit in the summary string.
        let summary = format!(
            "Periodic dynamic trigger check at local time {} / UTC {} with {} enabled rule(s); cwd: {}",
            now_local.format("%Y-%m-%d %H:%M:%S %Z"),
            now_utc.to_rfc3339(),
            rule_count,
            current_dir.as_deref().unwrap_or("<unknown>"),
        );
        Trigger {
            source: TriggerSource::Local {
                subkind: "dynamic".into(),
            },
            source_kind: SourceKind::Local,
            source_label: "local:dynamic".into(),
            event_label: "dynamic periodic check".into(),
            payload_visibility: PayloadVisibility::Local,
            payload_summary: Some(summary),
            payload: None,
            idempotency_key: format!("local:dynamic:{}", now_utc.timestamp_millis()),
            replacement_policy: ReplacementPolicy::Drop,
            trace_id: Uuid::new_v4().to_string(),
            authority: TriggerAuthority {
                principal_id: "local:dynamic".into(),
                principal_label: "dynamic trigger checker".into(),
                credential_scope: CredentialScope::User,
                allowed_source_actions: Vec::new(),
                expires_at: None,
            },
            received_at: now_utc,
        }
    }
}

#[async_trait]
impl NotificationHook for DynamicTriggerCheckHook {
    fn label(&self) -> &str {
        "local:dynamic"
    }

    async fn run(&self, sink: TriggerSink) -> Result<(), HookError> {
        self.status.lock().state = HookState::Connected;

        let mut interval = tokio::time::interval(self.interval);
        interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

        loop {
            interval.tick().await;
            let enabled_count = self
                .registry
                .list()
                .into_iter()
                .filter(|rule| rule.enabled)
                .count();
            if enabled_count == 0 {
                continue;
            }

            let trigger = self.build_trigger(enabled_count);
            if sink.send(trigger).is_err() {
                self.status.lock().state = HookState::Disconnected {
                    reason: "sink closed".into(),
                };
                return Err(HookError::SinkClosed);
            }
            let mut status = self.status.lock();
            status.last_event_at = Some(Utc::now());
            status.last_error = None;
        }
    }

    fn status(&self) -> NotificationHookStatus {
        self.status.lock().clone()
    }
}

pub fn before_trigger_action_hook(registry: DynamicTriggerRegistry) -> BeforeTriggerActionHook {
    Arc::new(
        move |ctx: BeforeTriggerActionContext, _cancel: CancellationToken| {
            let registry = registry.clone();
            Box::pin(async move {
                let rules = registry.list();
                let enabled: Vec<_> = rules.into_iter().filter(|r| r.enabled).collect();
                if enabled.is_empty() {
                    return TriggerAction::default_for(&ctx.trigger);
                }
                let promote_rule_ids: Vec<String> = enabled
                    .iter()
                    .filter(|rule| rule.promote_to_chat)
                    .map(|rule| rule.id.clone())
                    .collect();

                TriggerAction {
                    prompt: render_dynamic_trigger_prompt(&ctx.trigger, &enabled),
                    promote: if promote_rule_ids.is_empty() {
                        PromoteAction::None
                    } else {
                        // Transitional: still uses the deprecated summary-substring path.
                        // Tools-MCP's follow-up PR migrates this to
                        // `PromoteAction::PromoteSummaryWhenResultDetailsMatch` with
                        // `PromotionCondition::AnyOf` once the `mark_dynamic_rule_matched`
                        // tool is wired into the sub-agent. Allowed locally until then.
                        #[allow(deprecated)]
                        PromoteAction::PromoteSummaryWhenSummaryContains {
                            template_body: None,
                            required_substrings: promote_rule_ids,
                        }
                    },
                    promote_requires_approval: false,
                    delivery: TriggerDelivery::SubAgent,
                }
            })
        },
    )
}

/// Wrap a `before_trigger_action` hook so triggers from configured MCP servers bypass the
/// sub-agent. Two structural opt-ins, matched on the MCP `server_name` (never the model):
///
/// - `inject_summary_servers` → [`TriggerDelivery::InjectSummary`]: the pushed
///   `payload_summary` is injected into the parent chat verbatim. No model call.
/// - `inject_and_run_servers` → [`TriggerDelivery::InjectAndRun`]: the summary is injected
///   into the parent chat AND one model turn runs in the parent's full context, so the agent
///   reacts to the notification. `inject_and_run` wins if a server is in both sets.
///
/// Every other trigger falls through to `inner` (the dynamic-rule sub-agent path) unchanged.
/// A configured server is treated as a notification feed: dynamic rules are not consulted for
/// it. The engine still enforces the `[Trigger <id>] ` prefix on whatever is injected.
pub fn direct_inject_action_hook(
    inject_summary_servers: std::collections::HashSet<String>,
    inject_and_run_servers: std::collections::HashSet<String>,
    inner: BeforeTriggerActionHook,
) -> BeforeTriggerActionHook {
    Arc::new(
        move |ctx: BeforeTriggerActionContext, cancel: CancellationToken| {
            let server = match &ctx.trigger.source {
                TriggerSource::Mcp { server_name, .. } => Some(server_name.clone()),
                _ => None,
            };
            let run = server
                .as_ref()
                .is_some_and(|s| inject_and_run_servers.contains(s));
            let summary_only = !run
                && server
                    .as_ref()
                    .is_some_and(|s| inject_summary_servers.contains(s));

            if run {
                // Inject the summary as the prompt and run one turn in the parent context.
                // Fall back to a generic line when the push carried no summary so the agent
                // still has something to react to.
                let prompt = ctx.trigger.payload_summary.clone().unwrap_or_else(|| {
                    format!(
                        "{} fired: {}",
                        ctx.trigger.source_label, ctx.trigger.event_label
                    )
                });
                return Box::pin(async move {
                    TriggerAction {
                        prompt,
                        promote: PromoteAction::None,
                        promote_requires_approval: false,
                        delivery: TriggerDelivery::InjectAndRun,
                    }
                });
            }
            if summary_only {
                let has_summary = ctx.trigger.payload_summary.is_some();
                return Box::pin(async move {
                    TriggerAction {
                        prompt: String::new(),
                        // Render the raw summary verbatim. If the push carried no summary
                        // there is nothing to inject, so promote nothing — but still take the
                        // inject path so the source never spins up a sub-agent.
                        promote: if has_summary {
                            PromoteAction::PromoteSummaryNow {
                                template_body: Some("{{trigger.payload_summary}}".to_string()),
                            }
                        } else {
                            PromoteAction::None
                        },
                        promote_requires_approval: false,
                        delivery: TriggerDelivery::InjectSummary,
                    }
                });
            }
            inner(ctx, cancel)
        },
    )
}

pub fn fire_once_trigger_listener(registry: DynamicTriggerRegistry) -> TriggerListener {
    Arc::new(move |event| {
        let TriggerEvent::TriggerCompleted {
            summary: Some(summary),
            ..
        } = event
        else {
            return;
        };
        let ids = extract_dynamic_rule_ids(&summary);
        let _ = registry.mark_rules_fired(&ids);
    })
}

fn render_dynamic_trigger_prompt(trigger: &Trigger, rules: &[DynamicTriggerRule]) -> String {
    let rules_json = serde_json::to_string_pretty(rules).unwrap_or_else(|_| "[]".to_string());
    // RFC 0 §3.2.2 / RFC 1 §4.2.3 privacy contract: the full `payload` only reaches a
    // consumer when `payload_visibility = Shared`. For `Local` (default) and `Redacted`
    // sources we surface only the safe summary; the raw `payload` is null in the prompt
    // even if the adapter populated it. This prevents future hub / file-watcher / local
    // sources that legitimately attach context to `payload` from leaking that context
    // into the sub-agent (and therefore the model provider). The unconditional
    // serialization that existed before bypassed the contract.
    let payload_for_prompt = match trigger.payload_visibility {
        PayloadVisibility::Shared => trigger.payload.clone(),
        PayloadVisibility::Local | PayloadVisibility::Redacted => None,
    };
    let trigger_json = serde_json::json!({
        "source_kind": trigger.source_kind,
        "source": trigger.source.clone(),
        "source_label": trigger.source_label.clone(),
        "event_label": trigger.event_label.clone(),
        "payload_visibility": trigger.payload_visibility,
        "payload_summary": trigger.payload_summary.clone(),
        "payload": payload_for_prompt,
        "received_at": trigger.received_at,
        "idempotency_key": trigger.idempotency_key.clone(),
        "trace_id": trigger.trace_id.clone(),
        "authority": {
            "principal_id": trigger.authority.principal_id.clone(),
            "principal_label": trigger.authority.principal_label.clone(),
            "credential_scope": trigger.authority.credential_scope,
        }
    });
    let trigger_json =
        serde_json::to_string_pretty(&trigger_json).unwrap_or_else(|_| "{}".to_string());
    format!(
        "A trigger check event arrived.\n\nEvent:\n{trigger_json}\n\nDynamic trigger rules:\n{rules_json}\n\nEvaluate each rule's natural-language condition. For source-specific events, compare the rule against the event. For `local:dynamic` periodic checks, inspect current local or remote state with the available tools whenever the condition depends on filesystem state, paths, environment variables, shell expansion, command output, clock time, network/API state, or any fact not already present in the Event JSON. Do not report no match for those conditions until after the needed inspection. If no enabled rule matches after any required inspection, reply with exactly: no dynamic trigger rule matched.\n\nIf one or more rules match, execute each matching rule's action. Treat the action as an instruction from the user. If it asks to read or print a file, use the read tool or a safe shell command, then include the requested file contents in your final response. If it asks to run a local program or shell command, use the bash tool. Keep the final response concise and include the exact matched rule id(s), for example `matched dyn-...`."
    )
}

pub(super) fn extract_dynamic_rule_ids(text: &str) -> Vec<String> {
    let mut ids = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i + 4 <= bytes.len() {
        if &bytes[i..i + 4] != b"dyn-" {
            i += 1;
            continue;
        }

        let start = i;
        i += 4;
        while i < bytes.len() && bytes[i].is_ascii_hexdigit() {
            i += 1;
        }
        if i - start == 36 {
            let id = text[start..i].to_string();
            if !ids.iter().any(|existing| existing == &id) {
                ids.push(id);
            }
        }
    }
    ids
}
