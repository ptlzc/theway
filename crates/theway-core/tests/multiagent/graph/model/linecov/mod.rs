//! Additional line-coverage tests for `multiagent::graph::model` (see docs/rust-test-files.md).

use super::super::*;

#[test]
fn status_tag_covers_ready() {
    assert_eq!(status_tag(&NodeStatus::Ready), "[ready]");
}
