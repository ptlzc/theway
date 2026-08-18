//! External tests for `multiagent::registry` — split out of src
//! (see docs/rust-test-files.md).

use super::super::*;

#[test]
fn job_tps_and_cps_need_elapsed_time() {
    let mut job = AgentJob::new(
        "j1".into(),
        "agent".into(),
        "subagent".into(),
        None,
        None,
        None,
    );
    job.started_at = Some(1_000);
    job.completed_at = Some(2_000);
    job.output_tokens = 10;
    job.chars = 20;

    assert_eq!(job.tps(), Some(10.0));
    assert_eq!(job.cps(), Some(20.0));
}

#[test]
fn job_tps_and_cps_return_none_for_zero_elapsed() {
    let mut job = AgentJob::new(
        "j1".into(),
        "agent".into(),
        "subagent".into(),
        None,
        None,
        None,
    );
    job.started_at = Some(1_000);
    job.completed_at = Some(1_000);
    assert_eq!(job.tps(), None);
    assert_eq!(job.cps(), None);
}

#[test]
fn list_returns_newest_first() {
    let registry = AgentJobRegistry::new();
    let first = registry.register(JobInit {
        agent: "a".into(),
        source: "subagent".into(),
        run_id: None,
        node_id: None,
        session_id: None,
    });
    let second = registry.register(JobInit {
        agent: "b".into(),
        source: "dag".into(),
        run_id: None,
        node_id: None,
        session_id: None,
    });

    let list = registry.list();
    assert_eq!(list[0].id, second);
    assert_eq!(list[1].id, first);
}

#[test]
fn update_finish_and_find_with_unknown_id_are_noops() {
    let registry = AgentJobRegistry::new();
    registry.update("missing", |job| job.chars += 1);
    registry.set_control("missing", None);
    registry.finish("missing", JobStatus::Succeeded, None);
    assert!(registry.job("missing").is_none());
    assert!(registry.job_for_node("run", "node").is_none());
    assert!(registry.find_node("run", "node").is_none());
    assert!(!registry.interrupt("missing"));
    assert!(!registry.steer("missing", "x".into()));
    assert!(!registry.interrupt_node("run", "node"));
    assert!(!registry.steer_node("run", "node", "x".into()));
}

#[test]
fn subscribe_returns_receiver() {
    let registry = AgentJobRegistry::new();
    let mut rx = registry.subscribe();
    let id = registry.register(JobInit {
        agent: "a".into(),
        source: "subagent".into(),
        run_id: None,
        node_id: None,
        session_id: None,
    });
    let event = rx.try_recv().expect("started event");
    match event {
        AgentJobEvent::Started { id: started_id, .. } => assert_eq!(started_id, id),
        other => panic!("expected Started event, got {other:?}"),
    }
}

#[test]
fn evict_drops_oldest_running_when_all_running_over_cap() {
    let registry = AgentJobRegistry::new();
    let mut first = None;
    for i in 0..(MAX_JOBS + 2) {
        let id = registry.register(JobInit {
            agent: "a".into(),
            source: "subagent".into(),
            run_id: None,
            node_id: None,
            session_id: None,
        });
        if i == 0 {
            first = Some(id);
        }
    }
    assert_eq!(registry.list().len(), MAX_JOBS);
    // All jobs were running, so the oldest (first) is dropped defensively.
    assert!(registry.job(first.as_ref().unwrap()).is_none());
}

#[test]
fn control_handle_debug_does_not_leak() {
    let handle = AgentControlHandle {
        interrupt: Arc::new(|| {}),
        steer: Arc::new(|_| {}),
    };
    assert_eq!(format!("{handle:?}"), "AgentControlHandle");
}
