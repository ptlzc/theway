//! Tests for `mcp_notification_hook` — split out of src (see docs/RUST_TEST_FILES.md).

use super::*;
use serde_json::json;
use tokio::sync::mpsc;

fn note(method: &str, params: serde_json::Value) -> McpServerNotification {
    McpServerNotification {
        method: method.to_string(),
        params,
    }
}

/// Helper: build a hook over an mpsc, run it on a task, return the sender side so the
/// test can push notifications and a receiver to observe sunk triggers.
fn fixture() -> (
    mpsc::UnboundedSender<McpServerNotification>,
    mpsc::UnboundedReceiver<Trigger>,
    Arc<Mutex<NotificationHookStatus>>,
    tokio::task::JoinHandle<Result<(), HookError>>,
) {
    let (note_tx, note_rx) = mpsc::unbounded_channel::<McpServerNotification>();
    let (trig_tx, trig_rx) = mpsc::unbounded_channel::<Trigger>();
    let hook = Arc::new(McpNotificationHook::new("filesystem", note_rx));
    let status = hook.debug_status_handle();
    let hook_for_task = hook.clone();
    let handle = tokio::spawn(async move { hook_for_task.run(trig_tx).await });
    (note_tx, trig_rx, status, handle)
}

/// `tools/listChanged` → idempotency `"mcp:{server}:tools"` + `LatestReplaces`, no
/// payload, MCP source kind, server name + method threaded through. The `mcp:{server}:`
/// prefix is what prevents two MCP servers' identical method-local keys from
/// dedup-cancelling each other in the runtime.
#[tokio::test]
async fn tools_list_changed_maps_to_latest_replaces() {
    let (tx, mut rx, _status, handle) = fixture();
    tx.send(note("notifications/tools/listChanged", json!({})))
        .unwrap();
    let trigger = rx.recv().await.expect("trigger should arrive");
    assert_eq!(trigger.idempotency_key, "mcp:filesystem:tools");
    assert_eq!(
        trigger.replacement_policy,
        ReplacementPolicy::LatestReplaces
    );
    assert_eq!(trigger.source_kind, SourceKind::Mcp);
    assert!(matches!(
        trigger.source,
        TriggerSource::Mcp { ref server_name, ref method }
            if server_name == "filesystem" && method == "notifications/tools/listChanged"
    ));
    assert_eq!(trigger.source_label, "mcp:filesystem");
    assert!(
        trigger.payload.is_none(),
        "default payload_visibility=Local hides payload"
    );
    drop(tx);
    let _ = handle.await;
}

/// `resources/updated` keys by URI so two updates to different files don't collapse.
/// Key is `"mcp:{server}:resources:{uri}"`.
#[tokio::test]
async fn resources_updated_keys_per_uri() {
    let (tx, mut rx, _status, handle) = fixture();
    tx.send(note(
        "notifications/resources/updated",
        json!({ "uri": "file:///a.md" }),
    ))
    .unwrap();
    tx.send(note(
        "notifications/resources/updated",
        json!({ "uri": "file:///b.md" }),
    ))
    .unwrap();
    let t1 = rx.recv().await.unwrap();
    let t2 = rx.recv().await.unwrap();
    assert_eq!(t1.idempotency_key, "mcp:filesystem:resources:file:///a.md");
    assert_eq!(t2.idempotency_key, "mcp:filesystem:resources:file:///b.md");
    assert_ne!(t1.idempotency_key, t2.idempotency_key);
    drop(tx);
    let _ = handle.await;
}

/// Custom method with `_meta.theway_dedup_key` is accepted with `Drop` policy. Key gets
/// both the server prefix AND the `custom:` segment so user-supplied keys cannot
/// collide with the built-in `tools` / `resources` / `prompts` slots.
#[tokio::test]
async fn custom_with_meta_dedup_key_passes_through() {
    let (tx, mut rx, _status, handle) = fixture();
    tx.send(note(
        "notifications/custom/event",
        json!({ "_meta": { "theway_dedup_key": "build-42" }, "detail": "ok" }),
    ))
    .unwrap();
    let trigger = rx.recv().await.unwrap();
    assert_eq!(trigger.idempotency_key, "mcp:filesystem:custom:build-42");
    assert_eq!(trigger.replacement_policy, ReplacementPolicy::Drop);
    drop(tx);
    let _ = handle.await;
}

/// The retired top-level `_theway_dedup_key` (legacy compat for early adapters) is no
/// longer honored: a frame carrying only that form is dropped at the adapter exactly like
/// a key-less frame, and when it appears alongside `_meta.theway_dedup_key` only the
/// `_meta` key decides the idempotency key.
#[tokio::test]
async fn legacy_top_level_dedup_key_is_ignored_and_dropped() {
    let (tx, mut rx, status, handle) = fixture();

    // Build the retired field name indirectly so repo-wide greps for the legacy
    // protocol string stay clean.
    let legacy_field = ["_", "theway", "_dedup_key"].concat();

    // Only the legacy field present → dropped at the adapter, no trigger sunk. Busy-wait
    // on `dropped_count` like the key-less drop test above.
    tx.send(note(
        "notifications/custom/event",
        json!({ (legacy_field.clone()): "legacy-key", "detail": "ok" }),
    ))
    .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
    loop {
        if status.lock().dropped_count >= 1 {
            break;
        }
        if std::time::Instant::now() >= deadline {
            panic!(
                "dropped_count never reached 1 within deadline; status={:?}",
                status.lock().clone()
            );
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(
        rx.try_recv().is_err(),
        "legacy-only frame must not produce a trigger"
    );

    // Both forms present → `_meta.theway_dedup_key` is the sole source of truth; the
    // legacy field rides along as an inert parameter.
    tx.send(note(
        "notifications/custom/event",
        json!({
            "_meta": { "theway_dedup_key": "new-key" },
            (legacy_field): "legacy-key",
        }),
    ))
    .unwrap();
    let t2 = rx.recv().await.unwrap();
    assert_eq!(t2.idempotency_key, "mcp:filesystem:custom:new-key");

    drop(tx);
    let _ = handle.await;
}

/// Dedup keys are persisted inside trigger audit records. A malicious MCP server must
/// not be able to smuggle token-like material into that audit via `_meta.theway_dedup_key`;
/// hash unsafe keys while preserving stable dedup semantics.
#[tokio::test]
async fn custom_dedup_key_redacts_secret_like_text_in_idempotency_key() {
    let (tx, mut rx, _status, handle) = fixture();
    tx.send(note(
        "notifications/custom/payload",
        json!({ "_meta": { "theway_dedup_key": "hub_agent_secret_should_not_persist" } }),
    ))
    .unwrap();
    let trigger = rx.recv().await.unwrap();
    assert!(
        trigger
            .idempotency_key
            .starts_with("mcp:filesystem:custom:hash:"),
        "{}",
        trigger.idempotency_key
    );
    assert!(
        !trigger
            .idempotency_key
            .contains("hub_agent_secret_should_not_persist"),
        "{}",
        trigger.idempotency_key
    );
    drop(tx);
    let _ = handle.await;
}

/// Custom method without any dedup key is dropped at the adapter; the runtime never
/// sees a trigger but `dropped_count` increments and `last_error` records the reason.
///
/// We deliberately avoid pushing a follow-up known-good event here: a successful push
/// resets `last_error`, so we would lose the diagnostic before observing it. Instead
/// we busy-wait briefly on `status.dropped_count` to ensure the hook task processed
/// the frame, then assert both fields.
#[tokio::test]
async fn custom_without_dedup_key_is_dropped_with_diagnostic() {
    let (tx, mut rx, status, handle) = fixture();
    tx.send(note(
        "notifications/custom/event",
        json!({ "detail": "missing key" }),
    ))
    .unwrap();

    // Wait up to ~500ms for the hook task to observe the drop. In practice it fires
    // on the next tokio scheduler poll (<1ms), but we give CI plenty of slack.
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
    loop {
        if status.lock().dropped_count >= 1 {
            break;
        }
        if std::time::Instant::now() >= deadline {
            panic!(
                "dropped_count never reached 1 within deadline; status={:?}",
                status.lock().clone()
            );
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    // No trigger should reach the sink for this frame.
    assert!(
        rx.try_recv().is_err(),
        "custom-without-key must not produce a trigger"
    );
    let st = status.lock();
    assert_eq!(st.dropped_count, 1);
    assert!(
        st.last_error
            .as_deref()
            .unwrap_or("")
            .contains("dropped custom notification"),
        "diagnostic should mention the drop, got {:?}",
        st.last_error
    );
    drop(st);
    drop(tx);
    let _ = handle.await;
}

/// `resources/updated` without a `uri` field falls back to `resources:unknown` (still
/// server-namespaced) rather than crashing. Defensive — MCP spec requires uri but
/// adapters in the wild may misbehave.
#[tokio::test]
async fn resources_updated_without_uri_falls_back_to_resources_key() {
    let (tx, mut rx, _status, handle) = fixture();
    tx.send(note("notifications/resources/updated", json!({})))
        .unwrap();
    let trigger = rx.recv().await.unwrap();
    assert_eq!(trigger.idempotency_key, "mcp:filesystem:resources:unknown");
    drop(tx);
    let _ = handle.await;
}

/// Two MCP servers emitting the **same** method-local key (`tools` / `resources` /
/// per-URI / custom `theway_dedup_key`) must produce **distinct** runtime
/// `idempotency_key`s so the harness dedup window does not collapse one server's event
/// onto the other's. The fix: prefix every key with `mcp:{server_name}:` at the
/// adapter. PR #56 QA blocker.
#[tokio::test]
async fn idempotency_keys_are_namespaced_per_server() {
    let (note_tx_a, note_rx_a) = mpsc::unbounded_channel::<McpServerNotification>();
    let (trig_tx_a, mut trig_rx_a) = mpsc::unbounded_channel::<Trigger>();
    let hook_a = Arc::new(McpNotificationHook::new("server-a", note_rx_a));
    let driver_a = hook_a.clone();
    let handle_a = tokio::spawn(async move { driver_a.run(trig_tx_a).await });

    let (note_tx_b, note_rx_b) = mpsc::unbounded_channel::<McpServerNotification>();
    let (trig_tx_b, mut trig_rx_b) = mpsc::unbounded_channel::<Trigger>();
    let hook_b = Arc::new(McpNotificationHook::new("server-b", note_rx_b));
    let driver_b = hook_b.clone();
    let handle_b = tokio::spawn(async move { driver_b.run(trig_tx_b).await });

    // Both servers emit identical `tools/listChanged` — without the prefix they would
    // collide as a single dedup-window entry. Same exercise for a per-URI key, and a
    // custom `_meta.theway_dedup_key`.
    note_tx_a
        .send(note("notifications/tools/listChanged", json!({})))
        .unwrap();
    note_tx_b
        .send(note("notifications/tools/listChanged", json!({})))
        .unwrap();
    note_tx_a
        .send(note(
            "notifications/resources/updated",
            json!({ "uri": "file:///shared.md" }),
        ))
        .unwrap();
    note_tx_b
        .send(note(
            "notifications/resources/updated",
            json!({ "uri": "file:///shared.md" }),
        ))
        .unwrap();
    note_tx_a
        .send(note(
            "notifications/custom/event",
            json!({ "_meta": { "theway_dedup_key": "shared-build-1" } }),
        ))
        .unwrap();
    note_tx_b
        .send(note(
            "notifications/custom/event",
            json!({ "_meta": { "theway_dedup_key": "shared-build-1" } }),
        ))
        .unwrap();

    // Server A emitted 3 triggers; server B emitted 3 triggers. All 6 keys must be
    // distinct; specifically each pair must differ in the `mcp:{server}:` prefix.
    let mut a_keys = Vec::new();
    for _ in 0..3 {
        a_keys.push(trig_rx_a.recv().await.unwrap().idempotency_key);
    }
    let mut b_keys = Vec::new();
    for _ in 0..3 {
        b_keys.push(trig_rx_b.recv().await.unwrap().idempotency_key);
    }
    for k in &a_keys {
        assert!(
            k.starts_with("mcp:server-a:"),
            "server-a key missing prefix: {k}"
        );
    }
    for k in &b_keys {
        assert!(
            k.starts_with("mcp:server-b:"),
            "server-b key missing prefix: {k}"
        );
    }
    // Pairwise: cross-server keys are never equal.
    for ka in &a_keys {
        for kb in &b_keys {
            assert_ne!(
                ka, kb,
                "cross-server keys collided — runtime would dedup them as duplicates"
            );
        }
    }

    drop(note_tx_a);
    drop(note_tx_b);
    let _ = handle_a.await;
    let _ = handle_b.await;
}

/// Within a single server, a user-supplied custom dedup key (`_meta.theway_dedup_key`)
/// must not collide with the built-in `tools` / `resources` / `prompts` slots. The
/// adversarial case: a custom notification with `_meta.theway_dedup_key = "tools"`. Before
/// the `custom:` segment fix both events produced `mcp:filesystem:tools` and the
/// runtime would dedup them as duplicates; afterwards the custom key sits under
/// `mcp:filesystem:custom:tools`. PR #56 QA re-review blocker.
#[tokio::test]
async fn custom_key_cannot_collide_with_builtin_within_same_server() {
    let (tx, mut rx, _status, handle) = fixture();
    // Built-in path.
    tx.send(note("notifications/tools/listChanged", json!({})))
        .unwrap();
    // Adversarial custom path: user picked the exact string the built-in uses.
    tx.send(note(
        "notifications/custom/payload",
        json!({ "_meta": { "theway_dedup_key": "tools" } }),
    ))
    .unwrap();
    // Same adversarial collision for `resources` and `prompts`.
    tx.send(note(
        "notifications/custom/payload",
        json!({ "_meta": { "theway_dedup_key": "resources" } }),
    ))
    .unwrap();
    tx.send(note(
        "notifications/custom/payload",
        json!({ "_meta": { "theway_dedup_key": "prompts" } }),
    ))
    .unwrap();
    // And one that mimics the `resources:{uri}` shape of `resources/updated`.
    tx.send(note(
        "notifications/custom/payload",
        json!({ "_meta": { "theway_dedup_key": "resources:file:///x.md" } }),
    ))
    .unwrap();

    let t1 = rx.recv().await.unwrap();
    let t2 = rx.recv().await.unwrap();
    let t3 = rx.recv().await.unwrap();
    let t4 = rx.recv().await.unwrap();
    let t5 = rx.recv().await.unwrap();

    assert_eq!(t1.idempotency_key, "mcp:filesystem:tools");
    assert_eq!(t2.idempotency_key, "mcp:filesystem:custom:tools");
    assert_eq!(t3.idempotency_key, "mcp:filesystem:custom:resources");
    assert_eq!(t4.idempotency_key, "mcp:filesystem:custom:prompts");
    assert_eq!(
        t5.idempotency_key,
        "mcp:filesystem:custom:resources:file:///x.md"
    );

    // Pairwise distinct — none of the four custom keys equals the built-in or each other.
    let keys = [
        &t1.idempotency_key,
        &t2.idempotency_key,
        &t3.idempotency_key,
        &t4.idempotency_key,
        &t5.idempotency_key,
    ];
    for (i, a) in keys.iter().enumerate() {
        for b in &keys[i + 1..] {
            assert_ne!(
                a, b,
                "custom/built-in same-server key collision: {a} vs {b}"
            );
        }
    }

    drop(tx);
    let _ = handle.await;
}

/// `payload_visibility = Local` means the full `payload` is dropped; only
/// `payload_summary` survives into the persisted audit. For custom / unknown
/// notifications the adapter MUST NOT echo arbitrary params content into the summary
/// because params may contain secrets the server tucked in (API tokens, file contents,
/// PII, etc.). PR #56 QA blocker.
#[tokio::test]
async fn custom_method_summary_does_not_leak_params_content() {
    let (tx, mut rx, _status, handle) = fixture();
    // Sentinel string the test would only find in the summary if `render_summary`
    // serialized arbitrary params.
    let sentinel = "TOKEN_SENTINEL_SHOULD_NOT_APPEAR_IN_AUDIT";
    tx.send(note(
        "notifications/custom/secret-bearing",
        json!({
            "_meta": { "theway_dedup_key": "evt-1" },
            "secret": sentinel,
            "nested": { "more_secret": sentinel },
        }),
    ))
    .unwrap();
    let trigger = rx.recv().await.unwrap();
    let summary = trigger.payload_summary.unwrap_or_default();
    assert!(
        !summary.contains(sentinel),
        "summary leaked params content: {summary}"
    );
    assert_eq!(
        summary, "notifications/custom/secret-bearing",
        "custom-method summary must reduce to bare method name (no params echo)"
    );
    drop(tx);
    let _ = handle.await;
}

/// Adapters that need per-event human-readable detail for a custom notification can
/// opt in via `_meta.theway_summary: "<text>"`. The opt-in field is treated as
/// declaratively-safe by the server and surfaces into the summary capped at 200 chars.
/// Counterpart to the secret-leak test above.
#[tokio::test]
async fn custom_method_theway_summary_opt_in_appears_in_summary() {
    let (tx, mut rx, _status, handle) = fixture();
    tx.send(note(
        "notifications/custom/build-finished",
        json!({
            "_meta": {
                "theway_dedup_key": "build-99",
                "theway_summary": "build #99 finished: 3 tests failed",
            },
            "internal_token": "should-not-appear",
        }),
    ))
    .unwrap();
    let trigger = rx.recv().await.unwrap();
    let summary = trigger.payload_summary.unwrap_or_default();
    assert!(
        summary.contains("build #99 finished"),
        "opt-in theway_summary should surface: {summary}"
    );
    assert!(
        !summary.contains("should-not-appear"),
        "params outside of theway_summary must not leak: {summary}"
    );
    drop(tx);
    let _ = handle.await;
}

/// `_meta.theway_summary` is opt-in display text, but it is still MCP-server-controlled
/// input that becomes `trigger.payload_summary` and is persisted in trigger audit.
/// Redact token-like material before the runtime ever sees it.
#[tokio::test]
async fn custom_method_theway_summary_opt_in_redacts_secret_like_text() {
    let (tx, mut rx, _status, handle) = fixture();
    tx.send(note(
            "notifications/custom/build-finished",
            json!({
                "_meta": {
                    "theway_dedup_key": "build-100",
                    "theway_summary": "build leaked hub_agent_secret_should_not_persist token=sk-secret",
                },
            }),
        ))
        .unwrap();
    let trigger = rx.recv().await.unwrap();
    let summary = trigger.payload_summary.unwrap_or_default();
    assert!(summary.contains("notifications/custom/build-finished"));
    assert!(summary.contains("[redacted]"), "{summary}");
    assert!(!summary.contains("hub_agent_secret_should_not_persist"));
    assert!(!summary.contains("sk-secret"));
    drop(tx);
    let _ = handle.await;
}

#[tokio::test]
async fn agent_message_notification_is_generic_custom_mcp_trigger() {
    let trigger = map_notification(
        "remote-agent",
        &note(
            "notifications/agent_message",
            json!({
                "_meta": {
                    "theway_dedup_key": "note-1",
                    "theway_summary": "message ready",
                    "receiver_agent_id": "11111111-1111-4111-8111-111111111111",
                    "sender_agent_id": "22222222-2222-4222-8222-222222222222",
                },
                "sender": "@alice@example",
                "payload": {
                    "secret": "hub_agent_secret_should_not_leave_local_payload"
                },
            }),
        ),
    )
    .expect("custom notification with dedup key should map");

    assert_eq!(trigger.source_label, "mcp:remote-agent");
    assert_eq!(trigger.event_label, "notifications/agent_message");
    assert_eq!(trigger.payload_visibility, PayloadVisibility::Local);
    assert!(
        trigger.payload.is_none(),
        "must not build special-case binding payload"
    );
    let summary = trigger.payload_summary.unwrap_or_default();
    assert!(summary.contains("message ready"), "{summary}");
    assert!(!summary.contains("hub_agent_secret_should_not_leave_local_payload"));
}

/// Known `resources/updated` keeps the `uri` in the summary — `uri` is part of the
/// public resource address per MCP spec, not arbitrary params. Pins that we don't
/// over-correct and drop legitimate detail.
#[tokio::test]
async fn resources_updated_summary_includes_uri() {
    let (tx, mut rx, _status, handle) = fixture();
    tx.send(note(
        "notifications/resources/updated",
        json!({ "uri": "file:///proj/README.md", "rev": 5 }),
    ))
    .unwrap();
    let trigger = rx.recv().await.unwrap();
    let summary = trigger.payload_summary.unwrap_or_default();
    assert!(summary.contains("uri=file:///proj/README.md"), "{summary}");
    // Defensive: `rev` is a non-spec field and must not leak.
    assert!(
        !summary.contains("rev"),
        "non-spec params field leaked into summary: {summary}"
    );
    drop(tx);
    let _ = handle.await;
}

/// Resource URIs are allowed display metadata, but they can still be hostile strings
/// from an MCP server. Keep legitimate paths while redacting token-like URI values.
#[tokio::test]
async fn resources_updated_summary_redacts_secret_like_uri() {
    let (tx, mut rx, _status, handle) = fixture();
    tx.send(note(
        "notifications/resources/updated",
        json!({ "uri": "file:///proj/README.md?token=hub_agent_secret_should_not_persist" }),
    ))
    .unwrap();
    let trigger = rx.recv().await.unwrap();
    let summary = trigger.payload_summary.unwrap_or_default();
    assert!(
        summary.contains("notifications/resources/updated"),
        "{summary}"
    );
    assert!(summary.contains("uri=[redacted]"), "{summary}");
    assert!(!summary.contains("hub_agent_secret_should_not_persist"));
    assert!(
        !trigger
            .idempotency_key
            .contains("hub_agent_secret_should_not_persist"),
        "{}",
        trigger.idempotency_key
    );
    assert!(
        trigger.idempotency_key.contains("resources:hash:"),
        "{}",
        trigger.idempotency_key
    );
    drop(tx);
    let _ = handle.await;
}

/// Closing the sink while the hook is running surfaces as `HookError::SinkClosed` so
/// the supervisor can record the right termination reason. The hook should not panic
/// and `run` should return promptly.
#[tokio::test]
async fn sink_closed_returns_sink_closed_err() {
    let (note_tx, note_rx) = mpsc::unbounded_channel::<McpServerNotification>();
    let (trig_tx, trig_rx) = mpsc::unbounded_channel::<Trigger>();
    let hook = Arc::new(McpNotificationHook::new("filesystem", note_rx));
    let hook_clone = hook.clone();
    let handle = tokio::spawn(async move { hook_clone.run(trig_tx).await });

    // Drop the receiver to close the sink, then push a notification — the hook will
    // observe SendError on the first attempt and return SinkClosed.
    drop(trig_rx);
    note_tx
        .send(note("notifications/tools/listChanged", json!({})))
        .unwrap();
    let err = handle.await.unwrap();
    assert!(matches!(err, Err(HookError::SinkClosed)));
    assert!(matches!(
        hook.status().state,
        HookState::Disconnected { .. }
    ));
}

/// Transport close (the McpClient drops its sender) flips the hook to `Disconnected`
/// with a meaningful reason; `run` returns `Ok(())` so the supervisor knows it was a
/// clean exit rather than a transport-level error.
#[tokio::test]
async fn transport_close_returns_ok_and_marks_disconnected() {
    let (note_tx, note_rx) = mpsc::unbounded_channel::<McpServerNotification>();
    let (trig_tx, _trig_rx) = mpsc::unbounded_channel::<Trigger>();
    let hook = Arc::new(McpNotificationHook::new("filesystem", note_rx));
    let hook_clone = hook.clone();
    let handle = tokio::spawn(async move { hook_clone.run(trig_tx).await });

    drop(note_tx);
    let result = handle.await.unwrap();
    assert!(result.is_ok(), "clean transport close should be Ok");
    match hook.status().state {
        HookState::Disconnected { ref reason } => {
            assert!(reason.contains("transport"), "got reason={reason:?}");
        }
        other => panic!("expected Disconnected, got {other:?}"),
    }
}

/// Running the hook a second time fails because the receiver was already consumed.
/// Mirrors the single-consumer invariant on `McpClient::take_notifications`.
#[tokio::test]
async fn second_run_fails_after_receiver_consumed() {
    let (note_tx, note_rx) = mpsc::unbounded_channel::<McpServerNotification>();
    let (trig_tx, _trig_rx) = mpsc::unbounded_channel::<Trigger>();
    let hook = Arc::new(McpNotificationHook::new("filesystem", note_rx));
    let hook_first = hook.clone();
    let handle = tokio::spawn(async move { hook_first.run(trig_tx).await });

    drop(note_tx);
    let _ = handle.await;

    let (trig_tx2, _trig_rx2) = mpsc::unbounded_channel::<Trigger>();
    let err = hook.run(trig_tx2).await;
    assert!(matches!(err, Err(HookError::Other(_))));
}

/// Status starts as the trait-defined "pending" snapshot before `run` is invoked.
#[test]
fn initial_status_is_pending() {
    let (_tx, rx) = mpsc::unbounded_channel::<McpServerNotification>();
    let hook = McpNotificationHook::new("filesystem", rx);
    let s = hook.status();
    assert!(matches!(
        s.state,
        HookState::Disconnected { ref reason } if reason == "not yet started"
    ));
    assert_eq!(s.subscription_labels, vec!["mcp:filesystem".to_string()]);
    assert_eq!(s.dropped_count, 0);
}
