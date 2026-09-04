//! Run retention: `evict`, `clear_session_runs`, `clear_run`.

use super::*;

/// Insert a run directly with a chosen creation time and status so the
/// retention tests have deterministic, non-colliding timestamps.
fn insert_run(
    engine: &DagEngine,
    id: &str,
    status: DagStatus,
    session_id: Option<&str>,
    created_at: i64,
) {
    let node_status = match status {
        DagStatus::Completed => NodeStatus::Succeeded,
        DagStatus::Failed => NodeStatus::Failed,
        DagStatus::Cancelled => NodeStatus::Cancelled,
        DagStatus::Running => NodeStatus::Running,
    };
    let terminal = status != DagStatus::Running;
    engine.inner.lock().runs.insert(
        id.to_string(),
        DagRun {
            id: id.to_string(),
            name: "retention".to_string(),
            nodes: vec![DagNode {
                id: "a".to_string(),
                agent: "x".to_string(),
                task: "task".to_string(),
                depends_on: Vec::new(),
                timeout: None,
                cwd: None,
                provider: None,
                model: None,
                thinking: None,
                max_iterations: None,
                tools: None,
                status: node_status,
                job_id: None,
                attempt: 0,
                launch_gen: 0,
                started_at: Some(created_at),
                completed_at: if terminal { Some(created_at) } else { None },
                error: None,
                input_tokens: None,
                output_tokens: None,
                result: None,
                output: None,
                live_preview: None,
                last_active_at: None,
            }],
            status,
            kind: RunKind::Dag,
            max_concurrency: 1,
            fail_fast: false,
            direction: Direction::Td,
            created_at,
            session_id: session_id.map(ToString::to_string),
            completed_at: if terminal { Some(created_at) } else { None },
            last_activity_at: created_at,
            error: None,
        },
    );
}

#[test]
fn evict_over_cap_removes_oldest_terminal_and_keeps_running() {
    let engine = DagEngine::new();

    // MAX_TERMINAL_RUNS + 1 terminal runs with distinct timestamps.
    for i in 0..MAX_TERMINAL_RUNS + 1 {
        insert_run(
            &engine,
            &format!("t-{i}"),
            DagStatus::Completed,
            Some("s"),
            1000 + i as i64,
        );
    }
    // Two Running runs in the same session: they must never be evicted.
    insert_run(&engine, "run-a", DagStatus::Running, Some("s"), 9000);
    insert_run(&engine, "run-b", DagStatus::Running, Some("s"), 9001);

    let evicted = engine.evict(Some("s"));
    assert_eq!(evicted, vec!["t-0".to_string()]);

    // The oldest terminal run was removed; the remaining MAX_TERMINAL_RUNS stay.
    assert!(engine.get_run("t-0").is_none());
    for i in 1..MAX_TERMINAL_RUNS + 1 {
        assert!(engine.get_run(&format!("t-{i}")).is_some());
    }
    // Running runs are preserved.
    assert!(engine.get_run("run-a").is_some());
    assert!(engine.get_run("run-b").is_some());
}

#[test]
fn evict_below_cap_and_other_session_are_noop() {
    let engine = DagEngine::new();

    // Session "a" has exactly MAX_TERMINAL_RUNS terminal runs -> no eviction.
    for i in 0..MAX_TERMINAL_RUNS {
        insert_run(
            &engine,
            &format!("a-{i}"),
            DagStatus::Completed,
            Some("a"),
            i as i64,
        );
    }
    assert!(engine.evict(Some("a")).is_empty());
    assert!(engine.get_run("a-0").is_some());

    // A single run in another session is also below cap -> no-op.
    insert_run(&engine, "b-0", DagStatus::Failed, Some("b"), 1);
    assert!(engine.evict(Some("b")).is_empty());

    // Evicting session "b" must not touch session "a" runs.
    assert!(engine.get_run("a-0").is_some());
    assert!(engine.get_run("b-0").is_some());
}

#[test]
fn clear_session_runs_keeps_most_recent_n_and_is_session_scoped() {
    let engine = DagEngine::new();

    // Session "s1": 3 terminal runs (created_at 1,2,3) + 1 running.
    insert_run(&engine, "s1-old", DagStatus::Completed, Some("s1"), 1);
    insert_run(&engine, "s1-mid", DagStatus::Failed, Some("s1"), 2);
    insert_run(&engine, "s1-new", DagStatus::Cancelled, Some("s1"), 3);
    insert_run(&engine, "s1-run", DagStatus::Running, Some("s1"), 4);

    // Session "s2": 2 terminal runs.
    insert_run(&engine, "s2-a", DagStatus::Completed, Some("s2"), 5);
    insert_run(&engine, "s2-b", DagStatus::Failed, Some("s2"), 6);

    // Session-less runs.
    insert_run(&engine, "none-a", DagStatus::Completed, None, 7);
    insert_run(&engine, "none-b", DagStatus::Cancelled, None, 8);

    // keep=1 on "s1" removes the 2 oldest terminal runs, keeps the newest.
    let removed = engine.clear_session_runs(Some("s1"), 1);
    assert_eq!(removed, 2);
    assert!(engine.get_run("s1-old").is_none());
    assert!(engine.get_run("s1-mid").is_none());
    assert!(engine.get_run("s1-new").is_some());
    // Running run is never removed.
    assert!(engine.get_run("s1-run").is_some());

    // Other sessions are untouched (isolation).
    assert!(engine.get_run("s2-a").is_some());
    assert!(engine.get_run("s2-b").is_some());
    assert!(engine.get_run("none-a").is_some());
    assert!(engine.get_run("none-b").is_some());
}

#[test]
fn clear_session_runs_keep_zero_clears_all_terminal() {
    let engine = DagEngine::new();
    insert_run(&engine, "x-1", DagStatus::Completed, Some("s"), 1);
    insert_run(&engine, "x-2", DagStatus::Failed, Some("s"), 2);
    insert_run(&engine, "x-run", DagStatus::Running, Some("s"), 3);
    insert_run(&engine, "n-1", DagStatus::Completed, None, 4);
    insert_run(&engine, "n-run", DagStatus::Running, None, 5);

    // Session "s": keep=0 removes all terminal runs, keeps Running.
    assert_eq!(engine.clear_session_runs(Some("s"), 0), 2);
    assert!(engine.get_run("x-1").is_none());
    assert!(engine.get_run("x-2").is_none());
    assert!(engine.get_run("x-run").is_some());

    // Session-less (None) runs: keep=0 removes terminal only.
    assert_eq!(engine.clear_session_runs(None, 0), 1);
    assert!(engine.get_run("n-1").is_none());
    assert!(engine.get_run("n-run").is_some());

    // keep >= terminal count removes nothing.
    insert_run(&engine, "y-1", DagStatus::Completed, Some("s"), 6);
    insert_run(&engine, "y-2", DagStatus::Failed, Some("s"), 7);
    let before = engine.list_runs().len();
    assert_eq!(engine.clear_session_runs(Some("s"), 50), 0);
    assert_eq!(engine.list_runs().len(), before);
}

#[test]
fn clear_run_removes_only_terminal_runs() {
    let engine = DagEngine::new();
    insert_run(&engine, "c-done", DagStatus::Completed, Some("s"), 1);
    insert_run(&engine, "c-fail", DagStatus::Failed, Some("s"), 2);
    insert_run(&engine, "c-cancel", DagStatus::Cancelled, Some("s"), 3);
    insert_run(&engine, "c-run", DagStatus::Running, Some("s"), 4);

    assert!(engine.clear_run("c-done"));
    assert!(engine.get_run("c-done").is_none());

    assert!(engine.clear_run("c-fail"));
    assert!(engine.get_run("c-fail").is_none());

    assert!(engine.clear_run("c-cancel"));
    assert!(engine.get_run("c-cancel").is_none());

    // Running runs are never removed.
    assert!(!engine.clear_run("c-run"));
    assert!(engine.get_run("c-run").is_some());

    // Unknown id is a no-op.
    assert!(!engine.clear_run("missing"));
}
