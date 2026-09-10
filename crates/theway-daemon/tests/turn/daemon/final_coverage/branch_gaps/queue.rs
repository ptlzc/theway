//! `queue.rs` gaps: parked-turn start/finish branches.

use futures::stream::FuturesUnordered;

use super::super::*;
use crate::turn::daemon::SessionRuntimeState;

// ── queue.rs gaps ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn start_parked_turn_missing_session_and_busy_session() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();
    let mut unordered = FuturesUnordered::new();

    assert!(
        !host.start_parked_turn("missing", &mut unordered),
        "missing parked session must report false"
    );

    host.sessions.insert(SessionRuntimeState::for_test("parked-busy"));
    host.sessions
        .get_mut("parked-busy")
        .unwrap()
        .queue
        .push_back(QueuedTurn::UserPrompt {
            display: "held".into(),
            prompt: "held".into(),
            images: Vec::new(),
        input: None,
        persisted: false,});
    host.sessions.get_mut("parked-busy").unwrap().busy = true;
    assert!(
        !host.start_parked_turn("parked-busy", &mut unordered),
        "busy parked session must report false"
    );
}

#[tokio::test]
async fn start_parked_turn_reports_remaining_and_filters_busy_or_empty_sessions() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();

    host.sessions.insert(SessionRuntimeState::for_test("parked-many"));
    {
        let session = host.sessions.get_mut("parked-many").unwrap();
        session.queue.push_back(QueuedTurn::UserPrompt {
            display: "first".into(),
            prompt: "first".into(),
            images: Vec::new(),
        input: None,
        persisted: false,});
        session.queue.push_back(QueuedTurn::UserPrompt {
            display: "second".into(),
            prompt: "second".into(),
            images: Vec::new(),
        input: None,
        persisted: false,});
    }

    let mut unordered = FuturesUnordered::new();
    assert!(host.start_parked_turn("parked-many", &mut unordered));
    let parked = host.sessions.get("parked-many").unwrap();
    assert_eq!(parked.queue.len(), 1);
    assert!(!unordered.is_empty());
    drop(unordered);

    // `start_parked_turns` must evaluate its filter for a session that does
    // not match (busy with a queued job) and a session with an empty queue.
    host.sessions.insert(SessionRuntimeState::for_test("parked-filtered"));
    host.sessions
        .get_mut("parked-filtered")
        .unwrap()
        .queue
        .push_back(QueuedTurn::UserPrompt {
            display: "filtered".into(),
            prompt: "filtered".into(),
            images: Vec::new(),
        input: None,
        persisted: false,});
    host.sessions.get_mut("parked-filtered").unwrap().busy = true;
    let mut unordered = FuturesUnordered::new();
    host.start_parked_turns(&mut unordered);
    assert!(unordered.is_empty());
}

#[tokio::test]
async fn finish_parked_turn_missing_session_aborted_and_no_usage() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();
    let mut unordered = FuturesUnordered::new();

    // Missing session: the method returns without pushing a follow-up future.
    host.finish_parked_turn("missing", Ok(None), &mut unordered)
        .await;
    assert!(unordered.is_empty());

    // Aborted parked session: aborted branch in finish_parked_turn.
    host.sessions.insert(SessionRuntimeState::for_test("parked-aborted"));
    host.sessions.get_mut("parked-aborted").unwrap().aborted = true;
    host.finish_parked_turn("parked-aborted", Ok(None), &mut unordered)
        .await;
    let parked = host.sessions.get("parked-aborted").unwrap();
    assert!(!parked.aborted, "aborted flag is cleared after finish");
    assert!(
        parked
            .projection
            .feed
            .plain_lines(120)
            .iter()
            .any(|line| line.contains("[aborted]"))
    );

    // Non-aborted parked session without assistant usage: the usage lookup
    // must be empty and the Ok(Some) message is echoed.
    host.sessions.insert(SessionRuntimeState::for_test("parked-no-usage"));
    host.finish_parked_turn("parked-no-usage", Ok(Some("parked done".into())), &mut unordered)
        .await;
    let parked = host.sessions.get("parked-no-usage").unwrap();
    assert!(
        parked
            .projection
            .feed
            .plain_lines(120)
            .iter()
            .any(|line| line.contains("parked done"))
    );
}
