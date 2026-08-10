//! Sub-agent execution — Accepted → Running → Completed/Failed lifecycle, aborts,
//! failure reasons, and result-summary truncation (RFC 1 sub-PR 5a).

use super::*;

// ─────────────────────────────────────────────────────────────────────────────────────────
// Sub-agent execution — RFC 1 sub-PR 5a (Accepted → Running → Completed/Failed)
// ─────────────────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn accepted_trigger_spawns_sub_agent_and_writes_trigger_result_audit() {
    // Faux model returns one assistant message and stops; sub-agent runs cleanly.
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let trigger_runtime = TriggerRuntimeConfig::default();
    let before_trigger: Option<BeforeTriggerHook> = None;
    let on_trigger_prompt: Option<OnTriggerPromptHook> = None;
    let before_trigger_action: Option<BeforeTriggerActionHook> = None;
    let stream_fn = Some(faux_stream_fn("sub-agent done"));
    let harness = AgentHarness::new(AgentHarnessOptions::new(faux_model(), session.clone()));
    let executor = Arc::new(TriggerExecutor::new(
        harness.agent_arc(),
        session.clone(),
        trigger_runtime,
        before_trigger,
        on_trigger_prompt,
        before_trigger_action,
        stream_fn,
        None,
        None,
    ));

    let events = Arc::new(std::sync::Mutex::new(Vec::<TriggerEvent>::new()));
    let sink = events.clone();
    let _unsub = executor.subscribe(Arc::new(move |ev| {
        sink.lock().unwrap().push(ev);
    }));

    let _ = executor
        .handle_trigger(sample_trigger("k-spawn", "trace-spawn"))
        .await;

    // Wait for TriggerCompleted.
    let completed = wait_for_event(&events, 5, |evs| {
        evs.iter().find_map(|e| match e {
            TriggerEvent::TriggerCompleted {
                trace_id, summary, ..
            } if trace_id == "trace-spawn" => Some(summary.clone()),
            _ => None,
        })
    })
    .await;
    assert!(completed.is_some(), "must emit TriggerCompleted");

    // trigger_result audit must exist with success=true, trace_id link, summary from
    // sub-agent's assistant message.
    let entries = session.entries().await.unwrap();
    let record = entries
        .iter()
        .find_map(|e| match e {
            SessionTreeEntry::Custom {
                custom_type, data, ..
            } if custom_type == "trigger_result" => Some(data.clone()),
            _ => None,
        })
        .expect("trigger_result audit must exist");
    let data = record.expect("trigger_result must carry data");
    assert_eq!(data["trace_id"].as_str(), Some("trace-spawn"));
    assert_eq!(data["success"].as_bool(), Some(true));
    assert_eq!(
        data["summary"].as_str(),
        Some("sub-agent done"),
        "summary must be the sub-agent's final assistant text"
    );
    assert!(data["branch_id"].is_null(), "5a in-memory: branch_id null");
}

#[tokio::test]
async fn event_ordering_handled_then_started_then_completed() {
    // RFC 1 §5.F: HarnessHandled(Accepted) → TriggerExecutionStarted → TriggerCompleted
    // must always be observed in that order for the same trace_id.
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let trigger_runtime = TriggerRuntimeConfig::default();
    let before_trigger: Option<BeforeTriggerHook> = None;
    let on_trigger_prompt: Option<OnTriggerPromptHook> = None;
    let before_trigger_action: Option<BeforeTriggerActionHook> = None;
    let stream_fn = Some(faux_stream_fn("ok"));
    let harness = AgentHarness::new(AgentHarnessOptions::new(faux_model(), session.clone()));
    let executor = Arc::new(TriggerExecutor::new(
        harness.agent_arc(),
        session.clone(),
        trigger_runtime,
        before_trigger,
        on_trigger_prompt,
        before_trigger_action,
        stream_fn,
        None,
        None,
    ));

    let events = Arc::new(std::sync::Mutex::new(Vec::<TriggerEvent>::new()));
    let sink = events.clone();
    let _unsub = executor.subscribe(Arc::new(move |ev| {
        sink.lock().unwrap().push(ev);
    }));

    let _ = executor
        .handle_trigger(sample_trigger("k-order", "trace-order"))
        .await;

    wait_for_event(&events, 5, |evs| {
        evs.iter().find_map(|e| match e {
            TriggerEvent::TriggerCompleted { trace_id, .. } if trace_id == "trace-order" => {
                Some(())
            }
            _ => None,
        })
    })
    .await
    .expect("must complete");

    // Now find indices of the three events for trace-order.
    let evs = events.lock().unwrap().clone();
    let mut handled_idx = None;
    let mut started_idx = None;
    let mut completed_idx = None;
    for (i, e) in evs.iter().enumerate() {
        match e {
            TriggerEvent::TriggerHandled {
                trace_id,
                state: theway::trigger_engine::types::TriggerState::Accepted,
                ..
            } if trace_id == "trace-order" => handled_idx = Some(i),
            TriggerEvent::TriggerExecutionStarted { trace_id, .. } if trace_id == "trace-order" => {
                started_idx = Some(i)
            }
            TriggerEvent::TriggerCompleted { trace_id, .. } if trace_id == "trace-order" => {
                completed_idx = Some(i)
            }
            _ => {}
        }
    }
    let h = handled_idx.expect("TriggerHandled(Accepted)");
    let s = started_idx.expect("TriggerExecutionStarted");
    let c = completed_idx.expect("TriggerCompleted");
    assert!(
        h < s && s < c,
        "expected Handled({h}) < Started({s}) < Completed({c}); events={:?}",
        evs.iter().map(std::mem::discriminant).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn pump_non_blocking_second_trigger_audited_while_first_runs() {
    // RFC 1 §5.G acceptance #1: even if the first trigger's sub-agent is slow, the second
    // trigger reaches handle_trigger's audit/`TriggerHandled` event promptly.
    // We simulate "slow first" by making the faux stream sleep before emitting Done.
    use tokio::sync::Mutex as TokioMutex;

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let trigger_runtime = TriggerRuntimeConfig::default();
    let before_trigger: Option<BeforeTriggerHook> = None;
    let on_trigger_prompt: Option<OnTriggerPromptHook> = None;
    let before_trigger_action: Option<BeforeTriggerActionHook> = None;
    let stream_fn: Option<StreamFn>;

    // Slow stream: holds Done event for 500ms.
    let stream_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let stream_count_for_fn = stream_count.clone();
    let stream_fn_val: StreamFn = Arc::new(move |_, _, _| {
        let (stream, mut sender) = AssistantMessageEventStream::new();
        let nth = stream_count_for_fn.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        tokio::spawn(async move {
            // Only the FIRST sub-agent's stream is slow; subsequent (here, the second
            // trigger's sub-agent) returns immediately.
            if nth == 0 {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
            let msg = AssistantMessage {
                role: AssistantRole::Assistant,
                content: vec![ContentBlock::text("done")],
                api: theway_llm_provider::Api::from("faux"),
                provider: theway_llm_provider::Provider::from("faux"),
                model: "faux".into(),
                response_model: None,
                response_id: None,
                diagnostics: None,
                usage: Usage::default(),
                stop_reason: StopReason::Stop,
                error_message: None,
                timestamp: 0,
            };
            sender.push(AssistantMessageEvent::Start {
                partial: msg.clone(),
            });
            sender.push(AssistantMessageEvent::Done {
                reason: DoneReason::Stop,
                message: msg,
            });
        });
        stream
    });
    stream_fn = Some(stream_fn_val);
    let harness = AgentHarness::new(AgentHarnessOptions::new(faux_model(), session.clone()));
    let executor = Arc::new(TriggerExecutor::new(
        harness.agent_arc(),
        session.clone(),
        trigger_runtime,
        before_trigger,
        on_trigger_prompt,
        before_trigger_action,
        stream_fn,
        None,
        None,
    ));

    let events = Arc::new(std::sync::Mutex::new(Vec::<TriggerEvent>::new()));
    let sink = events.clone();
    let _unsub = executor.subscribe(Arc::new(move |ev| {
        sink.lock().unwrap().push(ev);
    }));

    // Fire trigger 1 — sub-agent will run slowly in the background.
    let _ = TokioMutex::new(()); // silence import warning if unused
    let t0 = std::time::Instant::now();
    let _ = executor
        .handle_trigger(sample_trigger("k-slow", "trace-slow"))
        .await;
    let elapsed_1 = t0.elapsed();
    // handle_trigger returns promptly even though sub-agent will be slow.
    assert!(
        elapsed_1 < std::time::Duration::from_millis(200),
        "handle_trigger must return promptly; took {elapsed_1:?}"
    );

    // Fire trigger 2 right after. It should reach TriggerHandled audit within a few ms,
    // unaffected by trigger 1's slow sub-agent.
    let t1 = std::time::Instant::now();
    let _ = executor
        .handle_trigger(sample_trigger("k-fast", "trace-fast"))
        .await;
    let elapsed_2 = t1.elapsed();
    assert!(
        elapsed_2 < std::time::Duration::from_millis(200),
        "second handle_trigger must not block on first sub-agent; took {elapsed_2:?}"
    );

    wait_for_event(&events, 2, |evs| {
        evs.iter().find_map(|e| match e {
            TriggerEvent::TriggerHandled { trace_id, .. } if trace_id == "trace-fast" => Some(()),
            _ => None,
        })
    })
    .await
    .expect("second trigger must reach TriggerHandled within 2s");
}

#[tokio::test]
async fn running_snapshot_lists_in_flight_trigger_with_preview() {
    // RFC 1 §5.G: notification_status_snapshot().running shows in-flight trigger with
    // preview-safe bounded fields.
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let trigger_runtime = TriggerRuntimeConfig::default();
    let before_trigger: Option<BeforeTriggerHook> = None;
    let on_trigger_prompt: Option<OnTriggerPromptHook> = None;
    let before_trigger_action: Option<BeforeTriggerActionHook> = None;
    let stream_fn: Option<StreamFn>;

    // Slow stream so the trigger stays in-flight long enough to inspect.
    let stream_fn_val: StreamFn = Arc::new(|_, _, _| {
        let (stream, mut sender) = AssistantMessageEventStream::new();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            let msg = AssistantMessage {
                role: AssistantRole::Assistant,
                content: vec![ContentBlock::text("done")],
                api: theway_llm_provider::Api::from("faux"),
                provider: theway_llm_provider::Provider::from("faux"),
                model: "faux".into(),
                response_model: None,
                response_id: None,
                diagnostics: None,
                usage: Usage::default(),
                stop_reason: StopReason::Stop,
                error_message: None,
                timestamp: 0,
            };
            sender.push(AssistantMessageEvent::Start {
                partial: msg.clone(),
            });
            sender.push(AssistantMessageEvent::Done {
                reason: DoneReason::Stop,
                message: msg,
            });
        });
        stream
    });
    stream_fn = Some(stream_fn_val);
    let harness = AgentHarness::new(AgentHarnessOptions::new(faux_model(), session.clone()));
    let executor = Arc::new(TriggerExecutor::new(
        harness.agent_arc(),
        session.clone(),
        trigger_runtime,
        before_trigger,
        on_trigger_prompt,
        before_trigger_action,
        stream_fn,
        None,
        None,
    ));

    let _ = executor
        .handle_trigger(sample_trigger("k-running", "trace-running"))
        .await;

    // Poll snapshot for the in-flight entry.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    let mut found = None;
    while std::time::Instant::now() < deadline {
        let snap = executor.notification_status_snapshot();
        if let Some(rt) = snap.running.iter().find(|r| r.trace_id == "trace-running") {
            found = Some(rt.clone());
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let rt = found.expect("running snapshot must include in-flight trigger");
    assert_eq!(rt.source_label, "MCP github");
    assert_eq!(rt.event_label, "pr merged");
    // Default action prompt: "MCP github fired: pr merged"
    assert!(
        rt.prompt_preview.contains("MCP github") && rt.prompt_preview.contains("pr merged"),
        "preview must reflect default prompt mapping, got {:?}",
        rt.prompt_preview
    );

    // After completion the entry leaves the snapshot.
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    let snap_after = executor.notification_status_snapshot();
    assert!(
        snap_after
            .running
            .iter()
            .all(|r| r.trace_id != "trace-running"),
        "running snapshot must drop completed triggers"
    );
}

#[tokio::test]
async fn abort_trigger_cancels_in_flight_sub_agent_and_emits_failed() {
    // RFC 1 §5.G acceptance #7: abort_trigger while sub-agent mid-execution → TriggerFailed
    // within 1s, trigger_result.summary == "aborted" (in our impl: status preserved + reason
    // "aborted" carried via TriggerFailed.reason).
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let trigger_runtime = TriggerRuntimeConfig::default();
    let before_trigger: Option<BeforeTriggerHook> = None;
    let on_trigger_prompt: Option<OnTriggerPromptHook> = None;
    let before_trigger_action: Option<BeforeTriggerActionHook> = None;
    let stream_fn: Option<StreamFn>;

    // Very slow stream so we can abort mid-flight.
    let stream_fn_val: StreamFn = Arc::new(|_, _, _| {
        let (stream, mut sender) = AssistantMessageEventStream::new();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            let msg = AssistantMessage {
                role: AssistantRole::Assistant,
                content: vec![ContentBlock::text("done")],
                api: theway_llm_provider::Api::from("faux"),
                provider: theway_llm_provider::Provider::from("faux"),
                model: "faux".into(),
                response_model: None,
                response_id: None,
                diagnostics: None,
                usage: Usage::default(),
                stop_reason: StopReason::Stop,
                error_message: None,
                timestamp: 0,
            };
            sender.push(AssistantMessageEvent::Start {
                partial: msg.clone(),
            });
            sender.push(AssistantMessageEvent::Done {
                reason: DoneReason::Stop,
                message: msg,
            });
        });
        stream
    });
    stream_fn = Some(stream_fn_val);
    let harness = AgentHarness::new(AgentHarnessOptions::new(faux_model(), session.clone()));
    let executor = Arc::new(TriggerExecutor::new(
        harness.agent_arc(),
        session.clone(),
        trigger_runtime,
        before_trigger,
        on_trigger_prompt,
        before_trigger_action,
        stream_fn,
        None,
        None,
    ));

    let events = Arc::new(std::sync::Mutex::new(Vec::<TriggerEvent>::new()));
    let sink = events.clone();
    let _unsub = executor.subscribe(Arc::new(move |ev| {
        sink.lock().unwrap().push(ev);
    }));

    let _ = executor
        .handle_trigger(sample_trigger("k-abort", "trace-abort"))
        .await;

    // Let TriggerExecutionStarted land first so we know the task is running.
    wait_for_event(&events, 2, |evs| {
        evs.iter().find_map(|e| match e {
            TriggerEvent::TriggerExecutionStarted { trace_id, .. } if trace_id == "trace-abort" => {
                Some(())
            }
            _ => None,
        })
    })
    .await
    .expect("ExecutionStarted must fire before abort");

    executor.abort_trigger("trace-abort");

    // TriggerFailed must arrive within ~1s (allow 3s for CI slack).
    let reason = wait_for_event(&events, 3, |evs| {
        evs.iter().find_map(|e| match e {
            TriggerEvent::TriggerFailed { trace_id, reason } if trace_id == "trace-abort" => {
                Some(reason.clone())
            }
            _ => None,
        })
    })
    .await
    .expect("TriggerFailed must arrive within 3s of abort");
    assert_eq!(
        reason, "aborted",
        "abort_trigger must emit TriggerFailed with reason \"aborted\""
    );

    // trigger_result audit reflects the failure.
    let entries = session.entries().await.unwrap();
    let record = entries
        .iter()
        .find_map(|e| match e {
            SessionTreeEntry::Custom {
                custom_type, data, ..
            } if custom_type == "trigger_result" => Some(data.clone()),
            _ => None,
        })
        .expect("trigger_result must be written even on abort");
    let data = record.expect("data");
    assert_eq!(data["success"].as_bool(), Some(false));
    assert_eq!(data["trace_id"].as_str(), Some("trace-abort"));
}

#[tokio::test]
async fn non_accepted_states_do_not_spawn_sub_agent() {
    // Dedup/CycleSuppressed/PermissionDenied must NOT trigger sub-agent execution. Verify
    // by sending a duplicate trigger and confirming no TriggerExecutionStarted fires for
    // it.
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let trigger_runtime = TriggerRuntimeConfig::default();
    let before_trigger: Option<BeforeTriggerHook> = None;
    let on_trigger_prompt: Option<OnTriggerPromptHook> = None;
    let before_trigger_action: Option<BeforeTriggerActionHook> = None;
    let stream_fn = Some(faux_stream_fn("done"));
    let harness = AgentHarness::new(AgentHarnessOptions::new(faux_model(), session.clone()));
    let executor = Arc::new(TriggerExecutor::new(
        harness.agent_arc(),
        session.clone(),
        trigger_runtime,
        before_trigger,
        on_trigger_prompt,
        before_trigger_action,
        stream_fn,
        None,
        None,
    ));

    let events = Arc::new(std::sync::Mutex::new(Vec::<TriggerEvent>::new()));
    let sink = events.clone();
    let _unsub = executor.subscribe(Arc::new(move |ev| {
        sink.lock().unwrap().push(ev);
    }));

    // First admission → spawn (Accepted).
    let _ = executor
        .handle_trigger(sample_trigger("k-dedup-test", "trace-1"))
        .await;
    // Second admission with same key → Deduped, NO spawn.
    let _ = executor
        .handle_trigger(sample_trigger("k-dedup-test", "trace-2"))
        .await;

    // Wait for the FIRST trigger to finish so all expected events are present.
    wait_for_event(&events, 5, |evs| {
        evs.iter().find_map(|e| match e {
            TriggerEvent::TriggerCompleted { trace_id, .. } if trace_id == "trace-1" => Some(()),
            _ => None,
        })
    })
    .await
    .expect("first trigger completes");

    // Now scan for any TriggerExecutionStarted with trace_id == "trace-2".
    let evs = events.lock().unwrap().clone();
    let spawned_for_trace_2 = evs.iter().any(|e| {
        matches!(
            e,
            TriggerEvent::TriggerExecutionStarted { trace_id, .. } if trace_id == "trace-2"
        )
    });
    assert!(
        !spawned_for_trace_2,
        "Deduped trigger must NOT spawn a sub-agent; got events: {:?}",
        evs.iter()
            .filter_map(|e| match e {
                TriggerEvent::TriggerExecutionStarted { trace_id, .. } => Some(trace_id.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn trigger_result_audit_records_failure_reason_for_resume_archaeology() {
    // CLI/TUI review on PR #64: jsonl-only readers (e.g. `theway --resume`, /diag, log
    // tooling) must see WHY a sub-agent failed without replaying the in-memory event bus.
    // Verify the audit Custom entry carries `reason`.
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let trigger_runtime = TriggerRuntimeConfig::default();
    let before_trigger: Option<BeforeTriggerHook> = None;
    let on_trigger_prompt: Option<OnTriggerPromptHook> = None;
    let before_trigger_action: Option<BeforeTriggerActionHook> = None;
    let stream_fn: Option<StreamFn>;

    // Slow stream so we have a window to abort.
    let stream_fn_val: StreamFn = Arc::new(|_, _, _| {
        let (stream, mut sender) = AssistantMessageEventStream::new();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            let msg = AssistantMessage {
                role: AssistantRole::Assistant,
                content: vec![ContentBlock::text("done")],
                api: theway_llm_provider::Api::from("faux"),
                provider: theway_llm_provider::Provider::from("faux"),
                model: "faux".into(),
                response_model: None,
                response_id: None,
                diagnostics: None,
                usage: Usage::default(),
                stop_reason: StopReason::Stop,
                error_message: None,
                timestamp: 0,
            };
            sender.push(AssistantMessageEvent::Start {
                partial: msg.clone(),
            });
            sender.push(AssistantMessageEvent::Done {
                reason: DoneReason::Stop,
                message: msg,
            });
        });
        stream
    });
    stream_fn = Some(stream_fn_val);
    let harness = AgentHarness::new(AgentHarnessOptions::new(faux_model(), session.clone()));
    let executor = Arc::new(TriggerExecutor::new(
        harness.agent_arc(),
        session.clone(),
        trigger_runtime,
        before_trigger,
        on_trigger_prompt,
        before_trigger_action,
        stream_fn,
        None,
        None,
    ));

    let events = Arc::new(std::sync::Mutex::new(Vec::<TriggerEvent>::new()));
    let sink = events.clone();
    let _unsub = executor.subscribe(Arc::new(move |ev| {
        sink.lock().unwrap().push(ev);
    }));

    let _ = executor
        .handle_trigger(sample_trigger("k-reason", "trace-reason"))
        .await;
    wait_for_event(&events, 2, |evs| {
        evs.iter().find_map(|e| match e {
            TriggerEvent::TriggerExecutionStarted { trace_id, .. }
                if trace_id == "trace-reason" =>
            {
                Some(())
            }
            _ => None,
        })
    })
    .await
    .expect("ExecutionStarted first");
    executor.abort_trigger("trace-reason");
    wait_for_event(&events, 3, |evs| {
        evs.iter().find_map(|e| match e {
            TriggerEvent::TriggerFailed { trace_id, .. } if trace_id == "trace-reason" => Some(()),
            _ => None,
        })
    })
    .await
    .expect("TriggerFailed within 3s");

    let entries = session.entries().await.unwrap();
    let data = entries
        .iter()
        .find_map(|e| match e {
            SessionTreeEntry::Custom {
                custom_type, data, ..
            } if custom_type == "trigger_result" => data.clone(),
            _ => None,
        })
        .expect("trigger_result data");
    assert_eq!(data["success"].as_bool(), Some(false));
    assert_eq!(
        data["reason"].as_str(),
        Some("aborted"),
        "trigger_result must persist failure reason so jsonl-only readers see WHY: {data:?}"
    );
    assert!(
        data["cost_usd"].is_null(),
        "5a does not measure cost — null is honest; 0.0 was misleading"
    );
}

#[tokio::test]
async fn trigger_result_summary_truncation_handles_multibyte_codepoint_via_production_path() {
    // Per @CLI-TUI-Dev-Lead's second PR #64 review: the previous test re-implemented the
    // boundary-walk logic locally, which proves the algorithm is sound but doesn't exercise
    // the production `last_assistant_text` helper. Drive the real `handle_trigger` →
    // sub-agent spawn → `last_assistant_text` → `trigger_result` audit path with a CJK-only
    // body engineered to land the 4 KiB cap mid-codepoint. The pre-fix code would panic
    // (turning the spawn task into a silent abort with no audit); the fixed code produces
    // a truncated summary ending in the truncation marker.
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone() as Arc<dyn SessionStorage>);
    let trigger_runtime = TriggerRuntimeConfig::default();
    let before_trigger: Option<BeforeTriggerHook> = None;
    let on_trigger_prompt: Option<OnTriggerPromptHook> = None;
    let before_trigger_action: Option<BeforeTriggerActionHook> = None;
    // 你 is 3 bytes in UTF-8. 1366 copies = 4098 bytes — the 4096-byte cap lands inside the
    // 1366th codepoint. We `Box::leak` because faux_stream_fn takes a `&'static str`.
    let stream_fn = {
        let huge_text: &'static str = Box::leak(("你".repeat(1366)).into_boxed_str());
        Some(faux_stream_fn(huge_text))
    };
    let harness = AgentHarness::new(AgentHarnessOptions::new(faux_model(), session.clone()));
    let executor = Arc::new(TriggerExecutor::new(
        harness.agent_arc(),
        session.clone(),
        trigger_runtime,
        before_trigger,
        on_trigger_prompt,
        before_trigger_action,
        stream_fn,
        None,
        None,
    ));

    let events = Arc::new(std::sync::Mutex::new(Vec::<TriggerEvent>::new()));
    let sink = events.clone();
    let _unsub = executor.subscribe(Arc::new(move |ev| {
        sink.lock().unwrap().push(ev);
    }));

    let _ = executor
        .handle_trigger(sample_trigger("k-utf8-trunc", "trace-utf8-trunc"))
        .await;
    // The production path must complete (not panic). If `last_assistant_text` panicked
    // mid-spawn the TriggerCompleted event would never fire.
    wait_for_event(&events, 5, |evs| {
        evs.iter().find_map(|e| match e {
            TriggerEvent::TriggerCompleted {
                trace_id, summary, ..
            } if trace_id == "trace-utf8-trunc" => Some(summary.clone()),
            _ => None,
        })
    })
    .await
    .expect("TriggerCompleted must fire — pre-fix code would panic and abort the task");

    // trigger_result audit's summary must be valid UTF-8 (otherwise serde_json would have
    // refused to encode it on the way to JSONL) AND must end with the truncation marker.
    let entries = session.entries().await.unwrap();
    let data = entries
        .iter()
        .find_map(|e| match e {
            SessionTreeEntry::Custom {
                custom_type, data, ..
            } if custom_type == "trigger_result" => data.clone(),
            _ => None,
        })
        .expect("trigger_result audit");
    let summary = data["summary"]
        .as_str()
        .expect("summary must be a string (proves valid UTF-8 round-trip through serde_json)");
    assert!(
        summary.ends_with("…[truncated]"),
        "summary must be capped with truncation marker; got len={} ending={:?}",
        summary.len(),
        summary.chars().rev().take(15).collect::<String>(),
    );
    // Per @QA-Release-Lead's PR #65 review: the **final** body (including marker) must
    // respect the 4 KiB cap. Previously we only constrained the pre-marker portion which
    // let the total grow beyond the documented boundary by the marker length.
    assert!(
        summary.len() <= 4096,
        "final summary (including truncation marker) must respect 4 KiB cap; got {}",
        summary.len()
    );
    let body_only = summary.trim_end_matches("…[truncated]");
    // And the truncated text is still composed of valid `你` codepoints (no half codepoint
    // bytes survived).
    assert!(
        body_only.chars().all(|c| c == '你'),
        "truncation MUST land on a char boundary; got non-你 chars in body"
    );
}
