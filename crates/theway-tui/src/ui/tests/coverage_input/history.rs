use super::*;

// ── history nav (app_input/history.rs) ────────────────────────────────

#[tokio::test]
async fn history_prev_next_covers_empty_and_all_bounds() {
    let (mut app, _rx) = test_app().await;

    // Empty history: both directions are no-ops.
    app.history_prev();
    assert!(
        app.history_idx.is_none(),
        "idx={:?} entries={:?}",
        app.history_idx,
        app.history.entries()
    );
    app.history_idx = None;
    app.history_next();
    assert!(app.history_idx.is_none());

    // Seed two entries and a draft.
    app.history.append("first");
    app.history.append("second");
    app.set_input("draft");

    // prev from None saves the draft and selects newest.
    app.history_prev();
    assert_eq!(app.history_idx, Some(1));
    assert_eq!(app.input_text(), "second");
    assert_eq!(app.draft, "draft");

    // prev from a non-zero index moves to the previous entry.
    app.history_prev();
    assert_eq!(app.history_idx, Some(0));
    assert_eq!(app.input_text(), "first");

    // prev from index 0 clamps at 0.
    app.history_prev();
    assert_eq!(app.history_idx, Some(0));
    assert_eq!(app.input_text(), "first");

    // next advances, then at the end restores the draft.
    app.history_next();
    assert_eq!(app.history_idx, Some(1));
    assert_eq!(app.input_text(), "second");
    app.history_next();
    assert_eq!(app.history_idx, None);
    assert_eq!(app.input_text(), "draft");
}
