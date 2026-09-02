use super::*;

#[tokio::test]
async fn list_returns_repo_summaries_without_live_current_flags() {
    let dir = tempdir().unwrap();
    let repo = Arc::new(SqliteSessionRepo::new(dir.path()));
    let session = repo.create("/cwd").await.unwrap();
    let id = session_id_of(&session).await;

    let ops = ops(repo, &id);
    let summaries = ops.list().await.unwrap();
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].session_id, id);
    assert!(!summaries[0].busy, "no daemon-side current busy flag");
    assert_eq!(summaries[0].graph_count, 0);
    assert_eq!(summaries[0].active_graph_count, 0);
}

#[tokio::test]
async fn list_skips_corrupt_session_db() {
    let dir = tempdir().unwrap();
    let repo = Arc::new(SqliteSessionRepo::new(dir.path()));
    let session = repo.create("/cwd").await.unwrap();
    let id = session_id_of(&session).await;

    // An empty file is not a valid SQLite session; it must not take down
    // session listing (one corrupt/orphaned db should be skipped).
    std::fs::write(dir.path().join("corrupt-session.db"), b"").unwrap();

    let ops = ops(repo, &id);
    let summaries = ops.list().await.unwrap();
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].session_id, id);
}

#[tokio::test]
async fn create_makes_new_session_with_inherited_cwd() {
    let dir = tempdir().unwrap();
    let repo = Arc::new(SqliteSessionRepo::new(dir.path()));
    let first = repo.create("/cwd").await.unwrap();
    let first_id = session_id_of(&first).await;

    let ops = ops(repo.clone(), &first_id);
    let new_id = ops.create(None, &HashMap::new()).await.unwrap();
    assert_ne!(new_id, first_id);
    let summaries = ops.list().await.unwrap();
    assert_eq!(summaries.len(), 2);
    assert!(summaries.iter().all(|s| s.cwd == "/cwd"));
}

#[tokio::test]
async fn rename_round_trips_through_list() {
    let dir = tempdir().unwrap();
    let repo = Arc::new(SqliteSessionRepo::new(dir.path()));
    let session = repo.create("/cwd").await.unwrap();
    let id = session_id_of(&session).await;

    let ops = ops(repo, &id);
    ops.rename(&id, "  my session  ").await.unwrap();
    let summaries = ops.list().await.unwrap();
    assert_eq!(summaries[0].name, "my session");

    let err = ops.rename(&id, "   ").await.unwrap_err().to_string();
    assert!(err.contains("must not be empty"), "{err}");
    let err = ops
        .rename("no-such-session", "x")
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("no session matches"), "{err}");
}

#[tokio::test]
async fn delete_removes_session_when_no_active_graphs() {
    let dir = tempdir().unwrap();
    let work = dir.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    let repo = Arc::new(SqliteSessionRepo::new(dir.path()));
    let session = repo.create(work.display().to_string()).await.unwrap();
    let id = session_id_of(&session).await;

    let ops = ops(repo.clone(), &id);
    ops.session_execution
        .set(
            id.clone(),
            theway_contract::session::SessionBinding {
                client_key: "client-1".into(),
                runtime: theway_contract::session::SessionRuntimeContext {
                    work_dir: work.display().to_string(),
                    provider: None,
                    model: None,
                    base_url: None,
                    thinking: None,
                },
            },
        )
        .unwrap();
    ops.session_execution
        .set_credential(&id, "faux", b"sentinel".to_vec())
        .unwrap();
    let active = ops.delete(&id).await.unwrap();
    assert!(active.is_empty(), "no graphs → delete succeeds");
    assert!(ops.list().await.unwrap().is_empty());
    assert!(ops.session_execution.get_credential(&id, "faux").is_none());
    assert!(ops.session_execution.get(&id).is_none());
}

#[tokio::test]
async fn delete_refuses_session_with_active_dag_run() {
    let dir = tempdir().unwrap();
    let repo = Arc::new(SqliteSessionRepo::new(dir.path()));
    let session = repo.create("/cwd").await.unwrap();
    let id = session_id_of(&session).await;

    let engine = Arc::new(DagEngine::new());
    // A goal run is a real engine run; stamp it to this session like the goal hook does.
    let run_id = engine.plan_goal("test condition", Some(id.clone()));
    let ops = AppSessionOps::new(
        repo.clone(),
        engine.clone(),
        "/cwd".into(),
        SessionExecutionRegistry::new(),
    );

    let active = ops.delete(&id).await.unwrap();
    assert_eq!(
        active,
        vec![run_id.clone()],
        "active run must refuse the delete"
    );
    assert_eq!(ops.list().await.unwrap().len(), 1, "session must survive");

    // Terminal run → protection lifts.
    engine.cancel_run(&run_id, Some("test cleanup"));
    let active = ops.delete(&id).await.unwrap();
    assert!(active.is_empty(), "aborted run must not block delete");
    assert!(ops.list().await.unwrap().is_empty());
}

#[tokio::test]
async fn create_with_custom_id_and_metadata_round_trips_through_list() {
    let dir = tempdir().unwrap();
    let repo = Arc::new(SqliteSessionRepo::new(dir.path()));
    let ops = ops(repo, "current");

    let mut metadata = HashMap::new();
    metadata.insert("tenant".to_string(), "acme".to_string());
    metadata.insert("source".to_string(), "workmate".to_string());

    let id = ops.create(Some("custom-session"), &metadata).await.unwrap();
    assert_eq!(id, "custom-session");

    let summaries = ops.list().await.unwrap();
    let summary = summaries
        .iter()
        .find(|s| s.session_id == "custom-session")
        .expect("custom session must be listed");
    assert_eq!(
        summary.metadata.get("tenant").map(String::as_str),
        Some("acme")
    );
    assert_eq!(
        summary.metadata.get("source").map(String::as_str),
        Some("workmate")
    );
}

#[tokio::test]
async fn create_with_duplicate_custom_id_returns_already_exists() {
    let dir = tempdir().unwrap();
    let repo = Arc::new(SqliteSessionRepo::new(dir.path()));
    let ops = ops(repo, "current");

    ops.create(Some("dup"), &HashMap::new()).await.unwrap();
    let err = ops
        .create(Some("dup"), &HashMap::new())
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.to_lowercase().contains("already exists") || err.contains("exists"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn update_metadata_merges_and_appears_in_list() {
    let dir = tempdir().unwrap();
    let repo = Arc::new(SqliteSessionRepo::new(dir.path()));
    let ops = ops(repo, "current");

    let mut initial = HashMap::new();
    initial.insert("tenant".to_string(), "acme".to_string());
    let id = ops.create(Some("meta-session"), &initial).await.unwrap();

    let mut update = HashMap::new();
    update.insert("env".to_string(), "prod".to_string());
    update.insert("tenant".to_string(), "globex".to_string());
    ops.update_metadata(&id, &update).await.unwrap();

    let summary = ops
        .list()
        .await
        .unwrap()
        .into_iter()
        .find(|s| s.session_id == id)
        .unwrap();
    assert_eq!(
        summary.metadata.get("tenant").map(String::as_str),
        Some("globex")
    );
    assert_eq!(
        summary.metadata.get("env").map(String::as_str),
        Some("prod")
    );
}

#[tokio::test]
async fn update_metadata_unknown_session_returns_not_found() {
    let dir = tempdir().unwrap();
    let repo = Arc::new(SqliteSessionRepo::new(dir.path()));
    let ops = ops(repo, "current");

    let err = ops
        .update_metadata("missing", &HashMap::new())
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("no session matches"), "{err}");
}

#[tokio::test]
async fn harness_introduction_metadata_is_readable_after_create() {
    let dir = tempdir().unwrap();
    let repo = Arc::new(SqliteSessionRepo::new(dir.path()));
    let ops = ops(repo.clone(), "current");

    let mut metadata = HashMap::new();
    metadata.insert(
        "harnessIntroduction".to_string(),
        "You are a database migration specialist.".to_string(),
    );
    let id = ops.create(Some("intro-session"), &metadata).await.unwrap();

    let session = SessionRepository::open(repo.as_ref(), &id)
        .await
        .unwrap()
        .unwrap();
    let meta = read_session_metadata(session.as_ref()).await.unwrap();
    assert_eq!(
        meta.get("harnessIntroduction").map(String::as_str),
        Some("You are a database migration specialist.")
    );
}

#[tokio::test]
async fn metadata_is_persisted_across_ops_instances() {
    let dir = tempdir().unwrap();
    let repo = Arc::new(SqliteSessionRepo::new(dir.path()));
    let mut initial = HashMap::new();
    initial.insert("tenant".to_string(), "acme".to_string());

    let first = ops(repo.clone(), "current");
    let id = first
        .create(Some("persistent-meta"), &initial)
        .await
        .unwrap();

    let mut update = HashMap::new();
    update.insert("env".to_string(), "prod".to_string());
    first.update_metadata(&id, &update).await.unwrap();

    // A fresh AppSessionOps must read the same metadata from the repo, not
    // from an in-memory cache.
    let second = ops(repo, "current");
    let summary = second
        .list()
        .await
        .unwrap()
        .into_iter()
        .find(|s| s.session_id == id)
        .unwrap();
    assert_eq!(
        summary.metadata.get("tenant").map(String::as_str),
        Some("acme")
    );
    assert_eq!(
        summary.metadata.get("env").map(String::as_str),
        Some("prod")
    );
}
