//! DAG model status labels.

use super::super::*;

#[test]
fn status_tag_covers_ready() {
    assert_eq!(status_tag(&NodeStatus::Ready), "[ready]");
}
