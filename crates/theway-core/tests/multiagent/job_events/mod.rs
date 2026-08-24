//! Tests for `multiagent::job_events` — split out of src
//! (see docs/rust-test-files.md).

use super::*;

#[test]
fn broadcast_capacity_is_256() {
    // Arrange/Act
    let capacity = SUBAGENT_JOB_EVENT_BROADCAST_CAPACITY;

    // Assert
    assert_eq!(capacity, 256);
}

#[test]
fn job_status_as_str_returns_lowercase_variant_names() {
    // Arrange/Act/Assert
    assert_eq!(SubagentJobStatus::Running.as_str(), "running");
    assert_eq!(SubagentJobStatus::Succeeded.as_str(), "succeeded");
    assert_eq!(SubagentJobStatus::Failed.as_str(), "failed");
    assert_eq!(SubagentJobStatus::Cancelled.as_str(), "cancelled");
    assert_eq!(SubagentJobStatus::Interrupted.as_str(), "interrupted");
}

#[test]
fn job_status_into_static_str_matches_as_str() {
    // Arrange
    let status = SubagentJobStatus::Succeeded;

    // Act
    let as_static: &'static str = status.into();

    // Assert
    assert_eq!(as_static, "succeeded");
    assert_eq!(as_static, status.as_str());
}

#[test]
fn agent_job_event_variants_carry_their_fields() {
    // Arrange
    let started = SubagentJobEvent::Started {
        id: "job-1".into(),
        agent: "researcher".into(),
        source: "dag".into(),
        run_id: Some("run-1".into()),
        node_id: Some("node-1".into()),
        session_id: Some("sess-1".into()),
    };
    let output = SubagentJobEvent::Output {
        id: "job-1".into(),
        chunk: "partial output".into(),
        session_id: Some("sess-1".into()),
    };
    let metrics = SubagentJobEvent::Metrics {
        id: "job-1".into(),
        tps: Some(12.5),
        cps: None,
        chars: 100,
        tokens_in: 20,
        tokens_out: 30,
        tools_called: 2,
        turn: 1,
        session_id: Some("sess-1".into()),
    };
    let completed = SubagentJobEvent::Completed {
        id: "job-1".into(),
        status: SubagentJobStatus::Succeeded,
        error: None,
        chars: 100,
        tokens_in: 20,
        tokens_out: 30,
        tools_called: 2,
        session_id: Some("sess-1".into()),
    };

    // Act
    let events = [started, output, metrics, completed];

    // Assert
    match &events[0] {
        SubagentJobEvent::Started {
            id,
            agent,
            source,
            run_id,
            node_id,
            session_id,
        } => {
            assert_eq!(id, "job-1");
            assert_eq!(agent, "researcher");
            assert_eq!(source, "dag");
            assert_eq!(run_id.as_deref(), Some("run-1"));
            assert_eq!(node_id.as_deref(), Some("node-1"));
            assert_eq!(session_id.as_deref(), Some("sess-1"));
        }
        other => panic!("expected Started event, got {other:?}"),
    }
    match &events[1] {
        SubagentJobEvent::Output {
            id,
            chunk,
            session_id,
        } => {
            assert_eq!(id, "job-1");
            assert_eq!(chunk, "partial output");
            assert_eq!(session_id.as_deref(), Some("sess-1"));
        }
        other => panic!("expected Output event, got {other:?}"),
    }
    match &events[2] {
        SubagentJobEvent::Metrics {
            id,
            tps,
            cps,
            chars,
            tokens_in,
            tokens_out,
            tools_called,
            turn,
            session_id,
        } => {
            assert_eq!(id, "job-1");
            assert_eq!(*tps, Some(12.5));
            assert_eq!(*cps, None);
            assert_eq!(*chars, 100);
            assert_eq!(*tokens_in, 20);
            assert_eq!(*tokens_out, 30);
            assert_eq!(*tools_called, 2);
            assert_eq!(*turn, 1);
            assert_eq!(session_id.as_deref(), Some("sess-1"));
        }
        other => panic!("expected Metrics event, got {other:?}"),
    }
    match &events[3] {
        SubagentJobEvent::Completed {
            id,
            status,
            error,
            chars,
            tokens_in,
            tokens_out,
            tools_called,
            session_id,
        } => {
            assert_eq!(id, "job-1");
            assert_eq!(*status, SubagentJobStatus::Succeeded);
            assert_eq!(error.as_deref(), None);
            assert_eq!(*chars, 100);
            assert_eq!(*tokens_in, 20);
            assert_eq!(*tokens_out, 30);
            assert_eq!(*tools_called, 2);
            assert_eq!(session_id.as_deref(), Some("sess-1"));
        }
        other => panic!("expected Completed event, got {other:?}"),
    }
}

#[test]
fn agent_job_events_are_clone_and_debug() {
    // Arrange
    let event = SubagentJobEvent::Started {
        id: "job-1".into(),
        agent: "researcher".into(),
        source: "dag".into(),
        run_id: None,
        node_id: None,
        session_id: Some("sess-1".into()),
    };

    // Act
    let cloned = event.clone();
    let debugged = format!("{event:?}");

    // Assert
    assert!(debugged.contains("Started"));
    let _ = cloned;
}
