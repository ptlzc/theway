use super::*;

#[test]
fn validate_catches_dup_ids() {
    let nodes = vec![node_def("a", "x", "t", &[]), node_def("a", "x", "t", &[])];
    let errors = validate_graph(&nodes, None);
    assert!(errors.iter().any(|e| e.contains("重复的节点 id")));
}

#[test]
fn validate_catches_self_dependency() {
    let nodes = vec![node_def("a", "x", "t", &["a"])];
    let errors = validate_graph(&nodes, None);
    assert!(errors.iter().any(|e| e.contains("不能依赖自己")));
}

#[test]
fn validate_catches_unknown_dependency() {
    let nodes = vec![node_def("a", "x", "t", &["nope"])];
    let errors = validate_graph(&nodes, None);
    assert!(
        errors
            .iter()
            .any(|e| e.contains("依赖了不存在的节点 \"nope\""))
    );
}

#[test]
fn validate_catches_cycle() {
    let nodes = vec![
        node_def("a", "x", "t", &["c"]),
        node_def("b", "x", "t", &["a"]),
        node_def("c", "x", "t", &["b"]),
    ];
    let errors = validate_graph(&nodes, None);
    assert!(errors.iter().any(|e| e.contains("检测到依赖环")));
}

#[test]
fn validate_catches_unknown_agent() {
    let nodes = vec![node_def("a", "bogus", "t", &[])];
    let known = vec!["explorer".to_string(), "planner".to_string()];
    let errors = validate_graph(&nodes, Some(&known));
    assert!(
        errors
            .iter()
            .any(|e| e.contains("引用了未知 subagent \"bogus\""))
    );
    let ok = validate_graph(&nodes, None);
    assert!(ok.is_empty());
}
