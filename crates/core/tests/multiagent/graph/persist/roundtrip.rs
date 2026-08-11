use super::*;

fn temp_db(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("dag-persist-{name}-{}.db", std::process::id()))
}

async fn open_clean(name: &str) -> SqliteDagStore {
    let path = temp_db(name);
    let _ = std::fs::remove_file(&path);
    SqliteDagStore::open(&path).await.unwrap()
}

#[tokio::test]
async fn save_load_round_trip() {
    let store = open_clean("roundtrip").await;
    store
        .save(&[sample_run("dag-1", DagStatus::Running)])
        .await
        .unwrap();
    let loaded = store.load().await.unwrap();
    assert_eq!(loaded.len(), 1);
    let p = &loaded[0];
    assert_eq!(p.id, "dag-1");
    assert_eq!(p.name, "run dag-1");
    assert_eq!(p.max_concurrency, 3);
    assert!(p.fail_fast);
    assert_eq!(p.direction, Direction::Td);
    assert_eq!(p.created_at, 500);
    assert_eq!(p.session_id.as_deref(), Some("sess-1"));
    assert_eq!(p.nodes.len(), 3);
    let n = &p.nodes[0];
    assert_eq!(n.id, "root");
    assert_eq!(n.agent, "explorer");
    assert_eq!(n.task, "task root");
    assert_eq!(n.depends_on, vec!["root"]);
    assert_eq!(n.timeout, Some(120));
    assert_eq!(n.model.as_deref(), Some("m1"));
    assert_eq!(n.thinking.as_deref(), Some("high"));
    assert_eq!(n.status, NodeStatus::Succeeded);
    assert_eq!(n.attempt, 2);
    assert_eq!(n.started_at, Some(1000));
    assert_eq!(n.input_tokens, Some(11));
    assert_eq!(n.output_tokens, Some(22));
    assert_eq!(n.output.as_deref(), Some("tail"));
    assert_eq!(n.live_preview.as_deref(), Some("preview"));
    assert_eq!(p.nodes[1].status, NodeStatus::Running);
    assert_eq!(p.nodes[2].status, NodeStatus::Pending);
    let _ = std::fs::remove_file(temp_db("roundtrip"));
}

#[tokio::test]
async fn payload_shape_matches_ts_projection() {
    let store = open_clean("shape").await;
    store
        .save(&[sample_run("dag-1", DagStatus::Running)])
        .await
        .unwrap();
    let loaded = store.load().await.unwrap();
    // Serialize the loaded PersistedRun and check camelCase keys (the TS
    // projection shape).
    let raw = serde_json::to_string(&loaded[0]).unwrap();
    for key in [
        "dependsOn",
        "maxConcurrency",
        "failFast",
        "createdAt",
        "sessionId",
        "startedAt",
        "inputTokens",
        "livePreview",
    ] {
        assert!(
            raw.contains(&format!("\"{key}\"")),
            "missing key {key} in {raw}"
        );
    }
    // lowercase enum values + TD/LR direction
    assert!(raw.contains("\"running\""));
    assert!(raw.contains("\"TD\""));
    let _ = std::fs::remove_file(temp_db("shape"));
}

#[tokio::test]
async fn hydrate_demotes_running_nodes() {
    let p = PersistedRun {
        id: "dag-9".to_string(),
        name: "n".to_string(),
        max_concurrency: 2,
        fail_fast: false,
        direction: Direction::Lr,
        created_at: 1,
        session_id: None,
        kind: RunKind::Dag,
        nodes: vec![
            PersistedNode {
                id: "a".to_string(),
                agent: "x".to_string(),
                task: "t".to_string(),
                depends_on: vec![],
                timeout: None,
                cwd: None,
                model: None,
                thinking: None,
                status: NodeStatus::Running,
                attempt: 1,
                started_at: Some(42),
                completed_at: None,
                error: None,
                input_tokens: Some(3),
                output_tokens: None,
                result: None,
                output: None,
                live_preview: Some("live".to_string()),
            },
            PersistedNode {
                id: "b".to_string(),
                agent: "x".to_string(),
                task: "t".to_string(),
                depends_on: vec!["a".to_string()],
                timeout: None,
                cwd: None,
                model: None,
                thinking: None,
                status: NodeStatus::Succeeded,
                attempt: 3,
                started_at: Some(10),
                completed_at: Some(20),
                error: None,
                input_tokens: Some(1),
                output_tokens: Some(2),
                result: Some(NodeResult {
                    success: true,
                    error: None,
                    duration_ms: Some(5),
                    attempt: 3,
                    total_attempts: 3,
                }),
                output: Some("out".to_string()),
                live_preview: None,
            },
        ],
    };
    let run = hydrate(p);
    assert_eq!(run.status, DagStatus::Running);
    assert!(run.last_activity_at > 0);
    let a = run.node("a").unwrap();
    assert_eq!(a.status, NodeStatus::Ready);
    assert_eq!(a.started_at, None);
    assert_eq!(a.job_id, None);
    assert_eq!(a.live_preview.as_deref(), Some("live"));
    let b = run.node("b").unwrap();
    assert_eq!(b.status, NodeStatus::Succeeded);
    assert_eq!(b.started_at, Some(10));
    assert_eq!(b.completed_at, Some(20));
    assert!(b.result.is_some());
    assert_eq!(run.direction, Direction::Lr);
    assert_eq!(run.created_at, 1);
}

#[tokio::test]
async fn only_running_runs_saved() {
    let store = open_clean("onlyrunning").await;
    let runs = vec![
        sample_run("dag-1", DagStatus::Running),
        sample_run("dag-2", DagStatus::Completed),
        sample_run("dag-3", DagStatus::Failed),
    ];
    store.save(&runs).await.unwrap();
    let loaded = store.load().await.unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].id, "dag-1");
    let _ = std::fs::remove_file(temp_db("onlyrunning"));
}

#[tokio::test]
async fn kind_round_trips() {
    // Goal kind survives save → load → hydrate.
    let store = open_clean("kind").await;
    let mut run = sample_run("goal-1", DagStatus::Running);
    run.kind = RunKind::Goal;
    store.save(&[run]).await.unwrap();
    let loaded = store.load().await.unwrap();
    assert_eq!(loaded[0].kind, RunKind::Goal);
    assert_eq!(
        hydrate(loaded.into_iter().next().unwrap()).kind,
        RunKind::Goal
    );
    let _ = std::fs::remove_file(temp_db("kind"));
}

#[tokio::test]
async fn missing_file_yields_empty() {
    let path = temp_db("missing");
    let _ = std::fs::remove_file(&path);
    let store = SqliteDagStore::open(&path).await.unwrap();
    assert!(store.load().await.unwrap().is_empty());
}

#[tokio::test]
async fn state_path_for_project_sanitizes() {
    let pi = Path::new("/tmp/.pi");
    assert_eq!(
        state_path_for_project(pi, None),
        PathBuf::from("/tmp/.pi/graph-engineering-state.db")
    );
    assert_eq!(
        state_path_for_project(pi, Some("sess-1")),
        PathBuf::from("/tmp/.pi/graph-engineering-state-sess-1.db")
    );
    // non-alnum → '_'
    assert_eq!(
        state_path_for_project(pi, Some("a/b c?d")),
        PathBuf::from("/tmp/.pi/graph-engineering-state-a_b_c_d.db")
    );
    // truncation to 60 chars
    let long = "x".repeat(100);
    assert_eq!(
        state_path_for_project(pi, Some(&long)).file_name().unwrap(),
        format!("graph-engineering-state-{}.db", "x".repeat(60)).as_str()
    );
    // empty → "default"; non-empty garbage is only char-mapped, kept
    assert_eq!(
        state_path_for_project(pi, Some("")),
        PathBuf::from("/tmp/.pi/graph-engineering-state-default.db")
    );
    assert_eq!(
        state_path_for_project(pi, Some("!!!")),
        PathBuf::from("/tmp/.pi/graph-engineering-state-___.db")
    );
}

#[test]
fn max_run_counter_parses_dag_n() {
    let mk = |id: &str| DagRun {
        id: id.to_string(),
        name: String::new(),
        nodes: vec![],
        status: DagStatus::Running,
        kind: RunKind::Dag,
        max_concurrency: 1,
        fail_fast: false,
        direction: Direction::Td,
        created_at: 0,
        session_id: None,
        completed_at: None,
        last_activity_at: 0,
        error: None,
    };
    assert_eq!(max_run_counter(&[]), 0);
    assert_eq!(max_run_counter(&[mk("dag-1"), mk("dag-7"), mk("dag-3")]), 7);
    // non-matching ids are ignored
    assert_eq!(
        max_run_counter(&[mk("run-2"), mk("dag-12x"), mk("x-dag-4")]),
        0
    );
    assert_eq!(max_run_counter(&[mk("dag-0")]), 0);
}
