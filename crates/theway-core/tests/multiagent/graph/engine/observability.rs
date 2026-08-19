use crate::observability::{
    OperationDetail, OperationKind, OperationOutcome, RuntimeObservation, RuntimeObserver,
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
