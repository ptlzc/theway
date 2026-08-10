use super::*;

#[tokio::test]
async fn wait_for_runs_returns_on_completion() {
    let (engine, _launcher) = engine_with_launcher();
    let run = engine
        .plan(
            run_def("t", None, None, &[("a", "x", "t", &[])]),
            None,
            None,
        )
        .unwrap();
    let id = run.id.clone();
    let waiter_id = id.clone();
    let engine2 = engine.clone();
    let waiter = tokio::spawn(async move {
        engine2
            .wait_for_runs(&[waiter_id], Duration::from_secs(5), None)
            .await
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    engine.on_node_completed(&id, "a", ok_outcome());
    let results = waiter.await.unwrap();
    assert_eq!(results, vec![(id, false)]);
}

#[tokio::test]
async fn wait_for_runs_times_out_and_multi_run() {
    let (engine, _launcher) = engine_with_launcher();
    let r1 = engine
        .plan(
            run_def("t1", None, None, &[("a", "x", "t", &[])]),
            None,
            None,
        )
        .unwrap();
    let r2 = engine
        .plan(
            run_def("t2", None, None, &[("b", "x", "t", &[])]),
            None,
            None,
        )
        .unwrap();
    let id1 = r1.id.clone();
    let id2 = r2.id.clone();
    engine.on_node_completed(&id1, "a", ok_outcome());
    // r1 terminal → immediate; r2 never completes → timed out.
    let results = engine
        .wait_for_runs(
            &[id1.clone(), id2.clone()],
            Duration::from_millis(100),
            None,
        )
        .await;
    assert_eq!(results, vec![(id1, false), (id2, true)]);
}

#[tokio::test]
async fn wait_for_runs_idle_watchdog() {
    let (engine, _launcher) = engine_with_launcher();
    let run = engine
        .plan(
            run_def("t", None, None, &[("a", "x", "t", &[])]),
            None,
            None,
        )
        .unwrap();
    let results = engine
        .wait_for_runs(
            std::slice::from_ref(&run.id),
            Duration::from_secs(10),
            Some(Duration::from_millis(80)),
        )
        .await;
    assert_eq!(results, vec![(run.id, true)]);
}
