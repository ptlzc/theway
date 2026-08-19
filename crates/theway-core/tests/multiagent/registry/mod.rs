//! Registry metrics, lifecycle, and control behavior.

use super::*;

mod messages;
mod operations;

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
fn registry_keeps_all_running_jobs_over_history_cap() {
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
    assert_eq!(registry.list().len(), MAX_JOBS + 2);
    assert!(registry.job(first.as_ref().unwrap()).is_some());
}

#[test]
fn node_operations_target_latest_registered_attempt() {
    use std::sync::atomic::{AtomicBool, Ordering};

    let registry = AgentJobRegistry::new();
    let old = registry.register(JobInit {
        agent: "a".into(),
        source: "dag".into(),
        run_id: Some("run".into()),
        node_id: Some("node".into()),
        session_id: None,
    });
    registry.update(&old, |job| {
        job.output = "old".into();
        job.messages = vec![serde_json::json!({"text": "old"})];
    });
    registry.finish(&old, JobStatus::Failed, Some("retry".into()));

    let interrupted = Arc::new(AtomicBool::new(false));
    let steered = Arc::new(std::sync::Mutex::new(None));
    let latest = registry.register(JobInit {
        agent: "a".into(),
        source: "dag".into(),
        run_id: Some("run".into()),
        node_id: Some("node".into()),
        session_id: None,
    });
    registry.update(&latest, |job| {
        job.output = "latest".into();
        job.messages = vec![serde_json::json!({"text": "latest"})];
    });
    registry.set_control(
        &latest,
        Some(AgentControlHandle {
            interrupt: {
                let interrupted = interrupted.clone();
                Arc::new(move || interrupted.store(true, Ordering::SeqCst))
            },
            steer: {
                let steered = steered.clone();
                Arc::new(move |text| *steered.lock().unwrap() = Some(text))
            },
        }),
    );

    assert_eq!(registry.find_node("run", "node").unwrap().id, latest);
    assert_eq!(registry.node_messages("run", "node").unwrap()[0]["text"], "latest");
    assert!(registry.interrupt_node("run", "node"));
    assert!(registry.steer_node("run", "node", "next".into()));
    assert!(interrupted.load(Ordering::SeqCst));
    assert_eq!(steered.lock().unwrap().as_deref(), Some("next"));
}

#[test]
fn control_handle_debug_does_not_leak() {
    let handle = AgentControlHandle {
        interrupt: Arc::new(|| {}),
        steer: Arc::new(|_| {}),
    };
    assert_eq!(format!("{handle:?}"), "AgentControlHandle");
}
