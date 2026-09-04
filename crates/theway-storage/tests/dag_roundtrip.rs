//! End-to-end tests for the SQLite DAG snapshot store.

use std::path::PathBuf;

use theway_contract::dag::{Direction, NodeStatus, PersistedNode, PersistedRun, RunKind};
use theway_storage::sqlite_dag::SqliteDagStore;

fn sample_run(id: &str) -> PersistedRun {
    let node = |id: &str, status: NodeStatus, started: bool| PersistedNode {
        id: id.to_string(),
        agent: "explorer".to_string(),
        task: format!("task {id}"),
        depends_on: vec!["root".to_string()],
        timeout: Some(120),
        cwd: None,
        provider: Some("p1".to_string()),
        model: Some("m1".to_string()),
        thinking: Some("high".to_string()),
        max_iterations: None,
        tools: None,
        status,
        attempt: 2,
        started_at: if started { Some(1000) } else { None },
        completed_at: None,
        error: None,
        input_tokens: Some(11),
        output_tokens: Some(22),
        result: None,
        output: Some("tail".to_string()),
        live_preview: Some("preview".to_string()),
    };
    PersistedRun {
        id: id.to_string(),
        name: format!("run {id}"),
        nodes: vec![
            node("root", NodeStatus::Succeeded, true),
            node("mid", NodeStatus::Running, true),
            node("tail", NodeStatus::Pending, false),
        ],
        kind: RunKind::Dag,
        max_concurrency: 3,
        fail_fast: true,
        direction: Direction::Td,
        created_at: 500,
        session_id: Some("sess-1".to_string()),
    }
}

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
    store.save(&[sample_run("dag-1")]).await.unwrap();
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
    store.save(&[sample_run("dag-1")]).await.unwrap();
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
async fn save_replaces_the_complete_snapshot_set() {
    let store = open_clean("replace").await;
    store
        .save(&[sample_run("dag-1"), sample_run("dag-2")])
        .await
        .unwrap();
    store.save(&[sample_run("dag-3")]).await.unwrap();
    let loaded = store.load().await.unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].id, "dag-3");
    let _ = std::fs::remove_file(temp_db("replace"));
}

#[tokio::test]
async fn kind_round_trips() {
    let store = open_clean("kind").await;
    let mut run = sample_run("goal-1");
    run.kind = RunKind::Goal;
    store.save(&[run]).await.unwrap();
    let loaded = store.load().await.unwrap();
    assert_eq!(loaded[0].kind, RunKind::Goal);
    let _ = std::fs::remove_file(temp_db("kind"));
}

#[tokio::test]
async fn missing_file_yields_empty() {
    let path = temp_db("missing");
    let _ = std::fs::remove_file(&path);
    let store = SqliteDagStore::open(&path).await.unwrap();
    assert!(store.load().await.unwrap().is_empty());
}

// ── corruption handling ─────────────────────────────────────────────────────

/// Clobber the db header (first 64 bytes) — turso fails with "invalid page
/// size in database header" on open, which is exactly the corruption path
/// `SqliteDagStore::open` must recover from by discarding + rebuilding.
#[tokio::test]
async fn open_rebuilds_after_header_corruption() {
    let path = temp_db("corrupt-header");
    let _ = std::fs::remove_file(&path);
    {
        let store = SqliteDagStore::open(&path).await.unwrap();
        let run = sample_run("dag-1");
        store.save(&[run]).await.unwrap();
    }
    // Corrupt the header.
    {
        use std::io::{Seek, SeekFrom, Write};
        let mut f = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        let zeros = [0u8; 64];
        f.seek(SeekFrom::Start(0)).unwrap();
        f.write_all(&zeros).unwrap();
    }
    // Open must discard + rebuild: file exists again, schema valid, empty.
    let store = SqliteDagStore::open(&path).await.unwrap();
    assert!(path.exists());
    assert!(store.load().await.unwrap().is_empty());
    // And a subsequent save works on the fresh db.
    let run = sample_run("dag-2");
    store.save(&[run]).await.unwrap();
    let loaded = store.load().await.unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].id, "dag-2");
    let _ = std::fs::remove_file(&path);
}

/// A zero-byte file is a legit "never written" state — must open fine (not be
/// mistaken for corruption).
#[tokio::test]
async fn empty_file_opens_clean() {
    let path = temp_db("empty-file");
    let _ = std::fs::remove_file(&path);
    std::fs::write(&path, b"").unwrap();
    let store = SqliteDagStore::open(&path).await.unwrap();
    assert!(store.load().await.unwrap().is_empty());
    let _ = std::fs::remove_file(&path);
}

/// Write failure on a corrupt store must rebuild and retry (once) so the save
/// eventually lands.
#[tokio::test]
async fn save_rebuilds_and_retries_on_write_failure() {
    let path = temp_db("corrupt-save");
    let _ = std::fs::remove_file(&path);
    let store = SqliteDagStore::open(&path).await.unwrap();
    // Corrupt the file behind the store's back (drop all handles first so the
    // file is not locked, then re-open the store — open rebuilds, so instead
    // corrupt AFTER open by clobbering the on-disk file; the store keeps its
    // open handle and the next write hits the damaged file).
    let run = sample_run("dag-1");
    store.save(&[run]).await.unwrap();
    // Clobber the file mid-write: truncate + garbage (this simulates the file
    // on disk being damaged between saves).
    {
        use std::io::{Seek, SeekFrom, Write};
        let mut f = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        let garbage = [0xFFu8; 4096];
        f.seek(SeekFrom::Start(0)).unwrap();
        f.write_all(&garbage).unwrap();
    }
    // The store's next save must rebuild + retry and succeed.
    let run2 = sample_run("dag-2");
    store.save(&[run2]).await.unwrap();
    let loaded = store.load().await.unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].id, "dag-2");
    let _ = std::fs::remove_file(&path);
}
