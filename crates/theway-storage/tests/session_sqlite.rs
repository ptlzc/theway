//! End-to-end SQLite session storage tests. The `SessionStorage` contract and
//! `Session` come from theway-core; the SQLite backend under test lives in
//! this crate (`sqlite_repo` / `sqlite_storage`).

use tempfile::tempdir;
use theway_storage::sqlite_repo::SqliteSessionRepo;

fn user_message(text: &str) -> theway_core::AgentMessage {
    theway_core::AgentMessage::Llm(theway_llm_provider::Message::User(
        theway_llm_provider::UserMessage {
            role: theway_llm_provider::UserRole::User,
            content: theway_llm_provider::UserContent::Text(text.into()),
            timestamp: chrono::Utc::now().timestamp_millis(),
        },
    ))
}

#[tokio::test]
async fn sqlite_session_persists_across_open() {
    let dir = tempdir().unwrap();
    let repo = SqliteSessionRepo::new(dir.path());

    let session = repo.create("/some/cwd").await.unwrap();
    session.append_message(user_message("hello")).await.unwrap();
    let leaf = session.leaf_id().await.unwrap().expect("leaf id");

    // Re-open the database and verify the message is still there.
    let files = repo.list().await.unwrap();
    assert_eq!(files.len(), 1);
    let reopened = repo.open(&files[0]).await.unwrap();
    let entries = reopened.entries().await.unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].id(), leaf);
}

#[tokio::test]
async fn sqlite_metadata_id_matches_session_file_stem() {
    let dir = tempdir().unwrap();
    let repo = SqliteSessionRepo::new(dir.path());

    let session = repo.create("/some/cwd").await.unwrap();
    let files = repo.list().await.unwrap();
    let stem = files[0].file_stem().and_then(|s| s.to_str()).unwrap();
    let meta = session.storage().get_metadata_json().await.unwrap();

    assert_eq!(meta.get("id").and_then(|v| v.as_str()), Some(stem));
}

#[tokio::test]
async fn sqlite_explicit_leaf_moves_are_overridden_by_new_entries() {
    let dir = tempdir().unwrap();
    let repo = SqliteSessionRepo::new(dir.path());

    let session = repo.create("/some/cwd").await.unwrap();
    let id_a = session.append_message(user_message("a")).await.unwrap();
    let _id_b = session.append_message(user_message("b")).await.unwrap();

    session.move_to(Some(&id_a), None).await.unwrap();
    let id_c = session.append_message(user_message("c")).await.unwrap();

    let files = repo.list().await.unwrap();
    let reopened = repo.open(&files[0]).await.unwrap();
    assert_eq!(
        reopened.leaf_id().await.unwrap().as_deref(),
        Some(id_c.as_str())
    );

    let branch = reopened.branch(None).await.unwrap();
    let ids: Vec<&str> = branch.iter().map(|e| e.id()).collect();
    assert_eq!(ids, vec![id_a.as_str(), id_c.as_str()]);
}

#[tokio::test]
async fn sqlite_can_move_leaf_to_root() {
    let dir = tempdir().unwrap();
    let repo = SqliteSessionRepo::new(dir.path());

    let session = repo.create("/some/cwd").await.unwrap();
    session.append_message(user_message("a")).await.unwrap();
    session.move_to(None, None).await.unwrap();

    let files = repo.list().await.unwrap();
    let reopened = repo.open(&files[0]).await.unwrap();
    assert_eq!(reopened.leaf_id().await.unwrap(), None);
    assert!(reopened.branch(None).await.unwrap().is_empty());
}

#[tokio::test]
async fn sqlite_compaction_summary_replaces_history_up_to_first_kept() {
    let dir = tempdir().unwrap();
    let repo = SqliteSessionRepo::new(dir.path());

    let session = repo.create("/some/cwd").await.unwrap();
    let _id1 = session
        .append_message(user_message("dropped"))
        .await
        .unwrap();
    let first_kept = session.append_message(user_message("kept")).await.unwrap();
    let _comp = session
        .append_compaction("summary text", &first_kept, 100, None, false)
        .await
        .unwrap();
    let _id3 = session.append_message(user_message("after")).await.unwrap();

    let ctx = session.build_context().await.unwrap();
    assert_eq!(ctx.messages.len(), 3);
    match &ctx.messages[0] {
        theway_core::AgentMessage::Custom(c) => assert_eq!(c.role, "compaction_summary"),
        _ => panic!("expected compaction_summary custom message"),
    }
}

// ── SQLite corruption handling ─────────────────────────────────────────────

/// A session db with a clobbered header must fail open with Corrupted and
/// leave the file in place (no auto-rebuild, no delete — the transcript is the
/// user's data; they decide).
#[tokio::test]
async fn sqlite_corrupt_header_reports_corrupted_and_keeps_file() {
    use std::io::{Seek, SeekFrom, Write};
    let dir = tempdir().unwrap();
    let repo = SqliteSessionRepo::new(dir.path());

    let files = {
        let session = repo.create("/some/cwd").await.unwrap();
        session
            .append_message(user_message("precious"))
            .await
            .unwrap();
        let files = repo.list().await.unwrap();
        assert_eq!(files.len(), 1);
        files
    };
    // Drop the session so the process-local turso handle is released, then
    // clobber the header (first 64 bytes) — simulates a damaged file on disk
    // that a fresh open must detect.
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .open(&files[0])
        .unwrap();
    let zeros = [0u8; 64];
    f.seek(SeekFrom::Start(0)).unwrap();
    f.write_all(&zeros).unwrap();
    drop(f);

    match repo.open(&files[0]).await {
        Err(e) => {
            assert_eq!(e.code, theway_core::SessionErrorCode::Corrupted);
        }
        Ok(_) => panic!("corrupt db must not open"),
    }
    // File still exists (we never delete user data).
    assert!(files[0].exists());
}

/// A session db with a damaged data page (but intact header) must be caught
/// by the quick_check integrity scan on open.
#[tokio::test]
async fn sqlite_corrupt_data_page_caught_by_integrity_check() {
    use std::io::{Seek, SeekFrom, Write};
    let dir = tempdir().unwrap();
    let repo = SqliteSessionRepo::new(dir.path());

    let path = {
        let session = repo.create("/some/cwd").await.unwrap();
        // Enough messages to spill past page 1 (4096 B) so a middle data
        // page exists to corrupt.
        for i in 0..300 {
            session
                .append_message(user_message(&format!("msg {i} {}", "x".repeat(200))))
                .await
                .unwrap();
        }
        let files = repo.list().await.unwrap();
        assert_eq!(files.len(), 1);
        files[0].clone()
    };
    // Sanity: the db really has >1 page (else the corruption below would
    // land past EOF and be ignored).
    let len = std::fs::metadata(&path).unwrap().len();
    assert!(len > 4096, "db too small for page-corruption test: {len} B");
    // Corrupt page 2 (a data page; page 1 is the root — turso tolerates
    // root-page damage, so we hit a real data page).
    let mut f = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    let garbage = [0xFFu8; 4096];
    f.seek(SeekFrom::Start(4096)).unwrap();
    f.write_all(&garbage).unwrap();
    drop(f);

    match repo.open(&path).await {
        Err(e) => assert_eq!(e.code, theway_core::SessionErrorCode::Corrupted),
        Ok(_) => panic!("corrupt data page must be caught by quick_check"),
    }
    // File untouched (never deleted).
    assert!(path.exists());
}
