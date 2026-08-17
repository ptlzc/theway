//! Tests for `multiagent::registry::events` — split out of src
//! (see docs/rust-test-files.md).

use super::*;

#[test]
fn broadcast_capacity_is_256() {
    // Arrange/Act
    let capacity = AGENT_JOB_EVENT_BROADCAST_CAPACITY;

    // Assert
    assert_eq!(capacity, 256);
}

#[test]
fn job_status_as_str_returns_lowercase_variant_names() {
    // Arrange/Act/Assert
    assert_eq!(JobStatus::Running.as_str(), "running");
    assert_eq!(JobStatus::Succeeded.as_str(), "succeeded");
    assert_eq!(JobStatus::Failed.as_str(), "failed");
    assert_eq!(JobStatus::Cancelled.as_str(), "cancelled");
    assert_eq!(JobStatus::Interrupted.as_str(), "interrupted");
}

#[test]
fn job_status_into_static_str_matches_as_str() {
    // Arrange
    let status = JobStatus::Succeeded;

    // Act
    let as_static: &'static str = status.into();

    // Assert
    assert_eq!(as_static, "succeeded");
    assert_eq!(as_static, status.as_str());
}

#[test]
fn agent_job_event_variants_carry_their_fields() {
    // Arrange
    let started = AgentJobEvent::Started {
        id: "job-1".into(),
        agent: "researcher".into(),
        source: "dag".into(),
        run_id: Some("run-1".into()),
        node_id: Some("node-1".into()),
    };
    let output = AgentJobEvent::Output {
        id: "job-1".into(),
        chunk: "partial output".into(),
    };
    let metrics = AgentJobEvent::Metrics {
        id: "job-1".into(),
        tps: Some(12.5),
        cps: None,
        chars: 100,
        tokens_in: 20,
        tokens_out: 30,
        tools_called: 2,
        turn: 1,
    };
    let completed = AgentJobEvent::Completed {
        id: "job-1".into(),
        status: JobStatus::Succeeded,
        error: None,
        chars: 100,
        tokens_in: 20,
        tokens_out: 30,
        tools_called: 2,
    };

    // Act
    let events = [started, output, metrics, completed];

    // Assert
    match &events[0] {
        AgentJobEvent::Started {
            id,
            agent,
            source,
            run_id,
            node_id,
        } => {
            assert_eq!(id, "job-1");
            assert_eq!(agent, "researcher");
            assert_eq!(source, "dag");
            assert_eq!(run_id.as_deref(), Some("run-1"));
            assert_eq!(node_id.as_deref(), Some("node-1"));
        }
        other => panic!("expected Started event, got {other:?}"),
    }
    match &events[1] {
        AgentJobEvent::Output { id, chunk } => {
            assert_eq!(id, "job-1");
            assert_eq!(chunk, "partial output");
        }
        other => panic!("expected Output event, got {other:?}"),
    }
    match &events[2] {
        AgentJobEvent::Metrics {
            id,
            tps,
            cps,
            chars,
            tokens_in,
            tokens_out,
            tools_called,
            turn,
        } => {
            assert_eq!(id, "job-1");
            assert_eq!(*tps, Some(12.5));
            assert_eq!(*cps, None);
            assert_eq!(*chars, 100);
            assert_eq!(*tokens_in, 20);
            assert_eq!(*tokens_out, 30);
            assert_eq!(*tools_called, 2);
            assert_eq!(*turn, 1);
        }
        other => panic!("expected Metrics event, got {other:?}"),
    }
    match &events[3] {
        AgentJobEvent::Completed {
            id,
            status,
            error,
            chars,
            tokens_in,
            tokens_out,
            tools_called,
        } => {
            assert_eq!(id, "job-1");
            assert_eq!(*status, JobStatus::Succeeded);
            assert_eq!(error.as_deref(), None);
            assert_eq!(*chars, 100);
            assert_eq!(*tokens_in, 20);
            assert_eq!(*tokens_out, 30);
            assert_eq!(*tools_called, 2);
        }
        other => panic!("expected Completed event, got {other:?}"),
    }
}

#[test]
fn agent_job_events_are_clone_and_debug() {
    // Arrange
    let event = AgentJobEvent::Started {
        id: "job-1".into(),
        agent: "researcher".into(),
        source: "dag".into(),
        run_id: None,
        node_id: None,
    };

    // Act
    let cloned = event.clone();
    let debugged = format!("{event:?}");

    // Assert
    assert!(debugged.contains("Started"));
    let _ = cloned;
}
