use super::*;

#[test]
fn fmt_dur_cases() {
    assert_eq!(fmt_dur(1_500), "1.5s");
    assert_eq!(fmt_dur(150_000), "2m30s");
    assert_eq!(fmt_dur(3_720_000), "1h2m");
    assert_eq!(fmt_dur(-1), "–");
}

#[test]
fn status_tags_and_prefix() {
    assert_eq!(status_tag(&NodeStatus::Pending), "[wait]");
    assert_eq!(status_tag(&NodeStatus::Succeeded), "[done]");
    assert_eq!(status_tag(&NodeStatus::Running), "[run]");
    assert_eq!(node_status_label(&NodeStatus::Running), "running");
    let def = run_def("t", vec![node_def("b", "x", "t", &["a", "c"])]);
    let run = build_run(&def);
    assert_eq!(deps_prefix(run.node("b").unwrap()), "[a,c] ");
}

#[test]
fn status_label_variants_and_predicates() {
    assert_eq!(node_status_label(&NodeStatus::Pending), "pending");
    assert_eq!(node_status_label(&NodeStatus::Ready), "ready");
    assert_eq!(node_status_label(&NodeStatus::Running), "running");
    assert_eq!(node_status_label(&NodeStatus::Succeeded), "succeeded");
    assert_eq!(node_status_label(&NodeStatus::Failed), "failed");
    assert_eq!(node_status_label(&NodeStatus::Skipped), "skipped");
    assert_eq!(node_status_label(&NodeStatus::Cancelled), "cancelled");

    assert_eq!(dag_status_label(&DagStatus::Running), "running");
    assert_eq!(dag_status_label(&DagStatus::Completed), "completed");
    assert_eq!(dag_status_label(&DagStatus::Failed), "failed");
    assert_eq!(dag_status_label(&DagStatus::Cancelled), "cancelled");

    assert!(is_terminal(&NodeStatus::Succeeded));
    assert!(is_terminal(&NodeStatus::Failed));
    assert!(is_terminal(&NodeStatus::Skipped));
    assert!(is_terminal(&NodeStatus::Cancelled));
    assert!(!is_terminal(&NodeStatus::Running));

    assert!(is_blocked(&NodeStatus::Failed));
    assert!(is_blocked(&NodeStatus::Cancelled));
    assert!(!is_blocked(&NodeStatus::Running));
}
