//! Live plugin event seam (issue #88): plugin-defined custom events are
//! routed to same-session `api.on` subscribers through the live event bus.
//! Custom events are not written to the durable session log; the durable
//! channel remains `api.events.append` plus `api.events.replay`.

use serde_json::Value;
use theway_contract::extension::ExtensionLifecycleEvent;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum LiveEventMode {
    #[default]
    Emit,
    Parallel,
    Serial,
    Bail,
    Waterfall,
}

impl LiveEventMode {
    pub(super) fn parse(value: Option<&str>) -> Result<Self, String> {
        match value {
            None | Some("emit") => Ok(Self::Emit),
            Some("parallel") => Ok(Self::Parallel),
            Some("serial") => Ok(Self::Serial),
            Some("bail") => Ok(Self::Bail),
            Some("waterfall") => Ok(Self::Waterfall),
            Some(other) => Err(format!("unknown live event dispatch mode: {other}")),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct LiveEvent {
    pub event_name: String,
    pub payload: Value,
    pub mode: LiveEventMode,
    pub origin_extension_id: Option<String>,
}

impl LiveEvent {
    pub(super) fn new(
        event_name: String,
        payload: Value,
        mode: LiveEventMode,
        origin_extension_id: Option<String>,
    ) -> Self {
        Self {
            event_name,
            payload,
            mode,
            origin_extension_id,
        }
    }
}

/// Custom event names use the same stable, lowercase ASCII shape as public
/// `namespace/action` names. Known public or internal lifecycle names are
/// rejected here: those events are emitted by the host lifecycle seams, not
/// re-published by plugins.
pub(super) fn validate_custom_event_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("custom event name must not be empty".into());
    }
    if name.len() > 128 {
        return Err("custom event name exceeds 128 characters".into());
    }
    if ExtensionLifecycleEvent::from_public_name(name).is_some() {
        return Err("event name is reserved for a host lifecycle event".into());
    }
    if name.starts_with('/') || name.ends_with('/') || name.contains("//") {
        return Err("custom event name has an invalid namespace segment".into());
    }
    if !name.bytes().all(|byte| {
        byte.is_ascii_lowercase()
            || byte.is_ascii_digit()
            || matches!(byte, b'/' | b'-' | b'_' | b'.')
    }) {
        return Err(
            "custom event name must use lowercase ASCII letters, digits, '/', '-', '_', or '.'"
                .into(),
        );
    }
    Ok(())
}
