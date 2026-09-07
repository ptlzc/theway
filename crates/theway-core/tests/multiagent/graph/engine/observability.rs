use crate::observability::{
    ObservationContext, OperationDetail, OperationKind, OperationOutcome, RuntimeObservation,
    RuntimeObserver,
};

use super::*;

#[derive(Default)]
struct RecordingObserver {
    observations: Mutex<Vec<RuntimeObservation>>,
}

impl RuntimeObserver for RecordingObserver {
    fn observe(&self, observation: RuntimeObservation) {
        self.observations.lock().push(observation);
    }
}

#[test]
fn dag_observations_link_run_node_and_terminal_measurements() {
    let recording = Arc::new(RecordingObserver::default());
    let observer: Arc<dyn RuntimeObserver> = recording.clone();
    let engine = DagEngine::with_observer(observer);
    let launcher = Arc::new(FakeLauncher::new());
    engine.set_launcher(Some(launcher));

    let run = engine
        .plan(
            run_def(
                "observed",
                None,
                None,
                &[("node-a", "agent-a", "secret-task", &[])],
            ),
            None,
            Some("session-a".into()),
        )
        .unwrap();

    let starts = recording.observations.lock().clone();
    let run_start = starts.iter().find_map(|observation| match observation {
        RuntimeObservation::OperationStarted(start)
            if matches!(start.detail, OperationDetail::DagRun) =>
        {
            Some(start)
        }
        _ => None,
    });
    let run_start = run_start.expect("dag run start");
    let node_start = starts.iter().find_map(|observation| match observation {
        RuntimeObservation::OperationStarted(start)
            if matches!(start.detail, OperationDetail::DagNode) =>
        {
            Some(start)
        }
        _ => None,
    });
    let node_start = node_start.expect("dag node start");
    assert_eq!(node_start.parent_id, Some(run_start.id));
    assert_eq!(node_start.context.run_id.as_deref(), Some(run.id.as_str()));
    assert_eq!(node_start.context.node_id.as_deref(), Some("node-a"));
    assert!(!format!("{starts:?}").contains("secret-task"));

    engine.on_node_completed(&run.id, "node-a", ok_outcome());

    let observations = recording.observations.lock();
    let node_finish = observations.iter().find_map(|observation| match observation {
        RuntimeObservation::OperationFinished(finish)
            if finish.kind == OperationKind::DagNode =>
        {
            Some(finish)
        }
        _ => None,
    });
    let node_finish = node_finish.expect("dag node finish");
    assert_eq!(node_finish.outcome, OperationOutcome::Succeeded);
    assert_eq!(node_finish.measurements.input_tokens, 5);
    assert_eq!(node_finish.measurements.output_tokens, 7);
    let run_finish = observations.iter().find_map(|observation| match observation {
        RuntimeObservation::OperationFinished(finish) if finish.kind == OperationKind::DagRun => {
            Some(finish)
        }
        _ => None,
    });
    let run_finish = run_finish.expect("dag run finish");
    assert_eq!(run_finish.outcome, OperationOutcome::Succeeded);
    assert_eq!(run_finish.measurements.input_tokens, 5);
    assert_eq!(run_finish.measurements.output_tokens, 7);
}

#[test]
fn cancelling_dag_finishes_node_and_run_as_cancelled() {
    let recording = Arc::new(RecordingObserver::default());
    let observer: Arc<dyn RuntimeObserver> = recording.clone();
    let engine = DagEngine::with_observer(observer);
    let launcher = Arc::new(FakeLauncher::new());
    engine.set_launcher(Some(launcher));
    let run = engine
        .plan(
            run_def(
                "cancel",
                None,
                None,
                &[
                    ("node-a", "agent-a", "task", &[]),
                    ("node-b", "agent-b", "task", &["node-a"]),
                ],
            ),
            None,
            None,
        )
        .unwrap();

    engine.cancel_run(&run.id, Some("secret-cancel-reason"));

    let observations = recording.observations.lock();
    let outcomes: Vec<_> = observations
        .iter()
        .filter_map(|observation| match observation {
            RuntimeObservation::OperationFinished(finish) => Some(finish.outcome),
            _ => None,
        })
        .collect();
    assert_eq!(outcomes, vec![OperationOutcome::Cancelled; 3]);
    assert!(!format!("{observations:?}").contains("secret-cancel-reason"));
}

#[test]
fn skipping_pending_node_emits_a_paired_skipped_observation() {
    let recording = Arc::new(RecordingObserver::default());
    let observer: Arc<dyn RuntimeObserver> = recording.clone();
    let engine = DagEngine::with_observer(observer);
    engine.set_launcher(Some(Arc::new(FakeLauncher::new())));
    let run = engine
        .plan(
            run_def(
                "skip",
                None,
                None,
                &[
                    ("node-a", "agent-a", "task", &[]),
                    ("node-b", "agent-b", "task", &["node-a"]),
                ],
            ),
            None,
            None,
        )
        .unwrap();

    assert!(engine.skip(&run.id, "node-b"));

    let observations = recording.observations.lock();
    let node_b_starts = observations
        .iter()
        .filter(|observation| {
            matches!(
                observation,
                RuntimeObservation::OperationStarted(start)
                    if matches!(start.detail, OperationDetail::DagNode)
                        && start.context.node_id.as_deref() == Some("node-b")
            )
        })
        .count();
    let node_b_finishes = observations
        .iter()
        .filter(|observation| {
            matches!(
                observation,
                RuntimeObservation::OperationFinished(finish)
                    if finish.kind == OperationKind::DagNode
                        && finish.context.node_id.as_deref() == Some("node-b")
                        && finish.outcome == OperationOutcome::Skipped
            )
        })
        .count();
    assert_eq!(node_b_starts, 1);
    assert_eq!(node_b_finishes, 1);
}

#[derive(Default)]
struct IncludeObserver {
    observations: Mutex<Vec<RuntimeObservation>>,
}

impl RuntimeObserver for IncludeObserver {
    fn observe(&self, observation: RuntimeObservation) {
        self.observations.lock().push(observation);
    }

    fn include_content(&self) -> bool {
        true
    }
}

#[test]
fn begin_observations_are_noops_for_missing_run_or_node() {
    let engine = DagEngine::new();
    engine.begin_run_observation("missing-run");
    assert!(engine.run_operation_id("missing-run").is_none());

    engine.set_launcher(Some(Arc::new(FakeLauncher::new())));
    let run = engine.plan(run_def("t", None, None, &[("a", "x", "t", &[])]), None, None).unwrap();
    engine.begin_node_observation("missing-run", "a");
    engine.begin_node_observation(&run.id, "missing-node");
    assert!(engine.node_operation_id("missing-run", "a").is_none());
    assert!(engine.node_operation_id(&run.id, "missing-node").is_none());
}

#[test]
fn finish_node_observation_attaches_content_when_enabled() {
    let observer = Arc::new(IncludeObserver::default());
    let dyn_observer: Arc<dyn RuntimeObserver> = observer.clone();
    let engine = DagEngine::with_observer(dyn_observer);
    let launcher = Arc::new(FakeLauncher::new());
    engine.set_launcher(Some(launcher));
    let run = engine
        .plan(
            run_def(
                "t",
                None,
                None,
                &[("a", "x", "secret", &[]), ("b", "x", "task", &["a"])],
            ),
            None,
            None,
        )
        .unwrap();

    engine.on_node_completed(&run.id, "a", ok_outcome());

    let observations = observer.observations.lock();
    let node_finish = observations.iter().find_map(|observation| match observation {
        RuntimeObservation::OperationFinished(finish)
            if finish.kind == OperationKind::DagNode =>
        {
            Some(finish)
        }
        _ => None,
    });
    let node_finish = node_finish.expect("dag node finish");
    assert!(node_finish.context.node_id.as_deref() == Some("a"));
}

#[test]
fn finish_observations_with_missing_entity_and_content_still_finish_scopes() {
    let observer: Arc<dyn RuntimeObserver> = Arc::new(IncludeObserver::default());
    let engine = DagEngine::with_observer(observer.clone());

    // Manually insert a node scope for a node that does not exist in the run,
    // then finish it: the missing-node branch inside finish_node_observation
    // is defensive and must not panic.
    let node_scope = OperationScope::start(
        engine.observer(),
        None,
        ObservationContext {
            session_id: None,
            run_id: Some("run".into()),
            node_id: Some("ghost".into()),
            ..ObservationContext::default()
        },
        OperationDetail::DagNode,
    );
    engine
        .node_operations
        .lock()
        .insert(("run".to_string(), "ghost".to_string()), node_scope);
    engine.finish_node_observation("run", "ghost", NodeStatus::Succeeded);

    // Same for a run scope: insert, remove the run, then finish.
    let run_scope = OperationScope::start(
        engine.observer(),
        None,
        ObservationContext {
            session_id: None,
            run_id: Some("run".into()),
            ..ObservationContext::default()
        },
        OperationDetail::DagRun,
    );
    engine.run_operations.lock().insert("run".to_string(), run_scope);
    engine.finish_run_observation("run", DagStatus::Completed);
}

#[test]
fn finish_run_observation_attaches_content_when_run_still_exists() {
    let observer = Arc::new(IncludeObserver::default());
    let dyn_observer: Arc<dyn RuntimeObserver> = observer.clone();
    let engine = DagEngine::with_observer(dyn_observer);
    engine.set_launcher(Some(Arc::new(FakeLauncher::new())));
    let run = engine
        .plan(
            run_def(
                "t",
                None,
                None,
                &[("a", "x", "secret", &[])],
            ),
            None,
            None,
        )
        .unwrap();

    engine.cancel_run(&run.id, Some("shutdown"));

    let observations = observer.observations.lock();
    let run_finish = observations.iter().find_map(|observation| match observation {
        RuntimeObservation::OperationFinished(finish)
            if finish.kind == OperationKind::DagRun =>
        {
            Some(finish)
        }
        _ => None,
    });
    assert!(run_finish.is_some());
}
