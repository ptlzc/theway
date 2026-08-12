//! Tests for `trigger_engine::runtime` — split out of src (see docs/RUST_TEST_FILES.md).

use super::*;

mod tests {
    use super::*;
    use crate::trigger_engine::types::{
        CredentialScope, PayloadVisibility, SourceKind, TriggerAuthority, TriggerSource,
    };

    fn make_trigger(idempotency: &str, trace: &str, policy: ReplacementPolicy) -> Trigger {
        make_trigger_at(idempotency, trace, policy, fixed_now())
    }

    fn make_trigger_at(
        idempotency: &str,
        trace: &str,
        policy: ReplacementPolicy,
        received_at: DateTime<Utc>,
    ) -> Trigger {
        Trigger {
            source: TriggerSource::Local {
                subkind: "test".into(),
            },
            source_kind: SourceKind::Local,
            source_label: "test".into(),
            event_label: "fire".into(),
            payload_visibility: PayloadVisibility::Local,
            payload_summary: None,
            payload: None,
            idempotency_key: idempotency.into(),
            replacement_policy: policy,
            trace_id: trace.into(),
            authority: TriggerAuthority {
                principal_id: "test:principal".into(),
                principal_label: "test".into(),
                credential_scope: CredentialScope::Project,
                allowed_source_actions: vec![],
                expires_at: None,
            },
            received_at,
        }
    }

    fn fixed_now() -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_000, 0).unwrap()
    }

    #[test]
    fn first_admission_accepts() {
        let runtime = TriggerRuntime::new();
        let outcome = runtime.evaluate(&make_trigger("k1", "t1", ReplacementPolicy::Drop));
        assert_eq!(outcome, EvaluationOutcome::Accept);
        assert_eq!(runtime.dedup_entry_count(), 1);
        assert_eq!(runtime.cycle_entry_count(), 1);
    }

    #[test]
    fn duplicate_within_window_returns_deduped_with_previous_trace_id() {
        let runtime = TriggerRuntime::new();
        runtime.evaluate(&make_trigger(
            "k1",
            "trace-original",
            ReplacementPolicy::Drop,
        ));
        let outcome = runtime.evaluate(&make_trigger(
            "k1",
            "trace-duplicate",
            ReplacementPolicy::Drop,
        ));
        assert_eq!(
            outcome,
            EvaluationOutcome::Deduped {
                replacement_policy: ReplacementPolicy::Drop,
                previous_trace_id: "trace-original".into()
            }
        );
        // Deduped event does not mutate the registry: cycle counter for the duplicate's
        // trace must NOT have been bumped, because dedup short-circuits before cycle.
        assert_eq!(
            runtime.cycle_entry_count(),
            1,
            "duplicate trigger must not allocate a cycle entry for its trace_id"
        );
    }

    #[test]
    fn deduped_outcome_carries_first_arrivals_replacement_policy() {
        // RFC 1 §5 + §11 #4: the first arrival's policy decides the window's collapse
        // semantics. Verify the engine reports back the FIRST policy even when subsequent
        // duplicates declare a different one.
        let runtime = TriggerRuntime::new();
        runtime.evaluate(&make_trigger("k1", "t1", ReplacementPolicy::LatestReplaces));
        let outcome = runtime.evaluate(&make_trigger("k1", "t2", ReplacementPolicy::Drop));
        match outcome {
            EvaluationOutcome::Deduped {
                replacement_policy, ..
            } => assert_eq!(
                replacement_policy,
                ReplacementPolicy::LatestReplaces,
                "first arrival's policy MUST win in the dedup window per RFC 1 §5"
            ),
            other => panic!("expected Deduped, got {other:?}"),
        }
    }

    #[test]
    fn dedup_window_expiry_re_admits_same_key() {
        let runtime = TriggerRuntime::with_config(TriggerRuntimeConfig {
            dedup_window: Duration::from_secs(60),
            cycle_hop_limit: 10,
        });
        let t0 = fixed_now();
        runtime.evaluate(&make_trigger_at("k1", "t1", ReplacementPolicy::Drop, t0));
        // Within window → dedup.
        let just_under = t0 + chrono::Duration::seconds(59);
        let still_dup = runtime.evaluate(&make_trigger_at(
            "k1",
            "t2",
            ReplacementPolicy::Drop,
            just_under,
        ));
        assert!(matches!(still_dup, EvaluationOutcome::Deduped { .. }));
        // Past window → fresh accept; the prior entry has been pruned.
        let past_window = t0 + chrono::Duration::seconds(61);
        let outcome = runtime.evaluate(&make_trigger_at(
            "k1",
            "t3",
            ReplacementPolicy::Drop,
            past_window,
        ));
        assert_eq!(outcome, EvaluationOutcome::Accept);
        assert_eq!(
            runtime.dedup_entry_count(),
            1,
            "only the freshest entry remains after window expiry + re-admit"
        );
    }

    #[test]
    fn cycle_limit_suppresses_when_trace_exceeds_hop_count() {
        let runtime = TriggerRuntime::with_config(TriggerRuntimeConfig {
            dedup_window: Duration::from_secs(300),
            cycle_hop_limit: 3,
        });
        let trace = "trace-loop";
        // Three accepts get us to hop_count = 3 == limit.
        for i in 0..3 {
            let outcome = runtime.evaluate(&make_trigger(
                &format!("k{i}"),
                trace,
                ReplacementPolicy::Drop,
            ));
            assert_eq!(outcome, EvaluationOutcome::Accept, "iteration {i}");
        }
        // Fourth trigger on same trace: must be suppressed, reporting the current hop
        // count BEFORE the suppression (since we do not advance the counter on suppress).
        let suppressed = runtime.evaluate(&make_trigger("k4", trace, ReplacementPolicy::Drop));
        assert_eq!(
            suppressed,
            EvaluationOutcome::CycleSuppressed { hop_count: 3 },
            "suppression reports the pre-block hop count so the audit shows where the chain stopped"
        );
    }

    #[test]
    fn record_follow_up_hop_does_not_require_a_trigger() {
        // The harness may bump the hop counter before spawning a sub-trigger (e.g. a tool
        // call that will produce an AgentDelegate event); make sure the helper exists and
        // contributes to cycle suppression even without a Trigger envelope on hand.
        let runtime = TriggerRuntime::with_config(TriggerRuntimeConfig {
            dedup_window: Duration::from_secs(300),
            cycle_hop_limit: 2,
        });
        let trace = "trace-followup";
        // One real trigger → hop_count = 1.
        let outcome = runtime.evaluate(&make_trigger("k1", trace, ReplacementPolicy::Drop));
        assert_eq!(outcome, EvaluationOutcome::Accept);
        // A follow-up hop recorded by the harness → hop_count = 2 (= limit).
        runtime.record_follow_up_hop(trace, fixed_now());
        // Next real trigger on the same trace must already be suppressed.
        let suppressed = runtime.evaluate(&make_trigger("k2", trace, ReplacementPolicy::Drop));
        assert_eq!(
            suppressed,
            EvaluationOutcome::CycleSuppressed { hop_count: 2 }
        );
    }

    #[test]
    fn dedup_window_clamped_to_max_24h() {
        let runtime = TriggerRuntime::with_config(TriggerRuntimeConfig {
            dedup_window: Duration::from_secs(48 * 60 * 60),
            cycle_hop_limit: 5,
        });
        assert_eq!(
            runtime.config().dedup_window,
            TriggerRuntimeConfig::MAX_DEDUP_WINDOW,
            "dedup_window MUST be clamped to MAX_DEDUP_WINDOW to bound memory"
        );
    }

    #[test]
    fn cycle_entries_for_unrelated_traces_are_independent() {
        let runtime = TriggerRuntime::with_config(TriggerRuntimeConfig {
            dedup_window: Duration::from_secs(300),
            cycle_hop_limit: 2,
        });
        // trace-a hits the limit
        runtime.evaluate(&make_trigger("k-a-1", "trace-a", ReplacementPolicy::Drop));
        runtime.evaluate(&make_trigger("k-a-2", "trace-a", ReplacementPolicy::Drop));
        assert!(matches!(
            runtime.evaluate(&make_trigger("k-a-3", "trace-a", ReplacementPolicy::Drop)),
            EvaluationOutcome::CycleSuppressed { .. }
        ));
        // trace-b is unaffected
        assert_eq!(
            runtime.evaluate(&make_trigger("k-b-1", "trace-b", ReplacementPolicy::Drop)),
            EvaluationOutcome::Accept
        );
    }

    #[test]
    fn snapshot_counters_track_each_outcome() {
        let runtime = TriggerRuntime::with_config(TriggerRuntimeConfig {
            dedup_window: Duration::from_secs(300),
            cycle_hop_limit: 2,
        });
        // accepted: 2 (two distinct keys / trace)
        runtime.evaluate(&make_trigger("k1", "ta", ReplacementPolicy::Drop));
        runtime.evaluate(&make_trigger("k2", "tb", ReplacementPolicy::Drop));
        // deduped: 1 (duplicate of k1)
        runtime.evaluate(&make_trigger("k1", "tc", ReplacementPolicy::Drop));
        // cycle-suppress: take trace-a to limit + 1 over
        runtime.evaluate(&make_trigger("k3", "ta", ReplacementPolicy::Drop));
        // ta is now at hop 2 (limit). next evaluate hits CycleSuppressed.
        runtime.evaluate(&make_trigger("k4", "ta", ReplacementPolicy::Drop));

        let snap = runtime.snapshot();
        assert_eq!(snap.accepted_total, 3, "snapshot: {snap:?}");
        assert_eq!(snap.deduped_total, 1, "snapshot: {snap:?}");
        assert_eq!(snap.cycle_suppressed_total, 1, "snapshot: {snap:?}");
        assert!(
            snap.dedup_entries >= 1,
            "dedup map must hold at least one live entry"
        );
        assert!(
            snap.active_traces >= 1,
            "cycle map must hold at least one live trace"
        );
    }
}

