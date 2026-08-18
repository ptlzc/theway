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

#[test]
fn validate_empty_known_agents_accepts_unknown_agent() {
    let nodes = vec![node_def("a", "bogus", "t", &[])];
    let errors = validate_graph(&nodes, Some(&[]));
    assert!(errors.is_empty());
}

#[test]
fn validate_catches_invalid_id_missing_agent_and_empty_task() {
    let nodes = vec![
        DagNodeDef {
            id: "bad id!".into(),
            agent: "x".into(),
            task: "t".into(),
            depends_on: None,
            timeout: None,
            cwd: None,
            model: None,
            thinking: None,
            max_iterations: None,
            tools: None,
        },
        DagNodeDef {
            id: "ok".into(),
            agent: String::new(),
            task: "   ".into(),
            depends_on: None,
            timeout: None,
            cwd: None,
            model: None,
            thinking: None,
            max_iterations: None,
            tools: None,
        },
    ];
    let errors = validate_graph(&nodes, None);
    assert!(errors.iter().any(|e| e.contains("非法")));
    assert!(errors.iter().any(|e| e.contains("缺少 agent")));
    assert!(errors.iter().any(|e| e.contains("缺少 task")));
}
