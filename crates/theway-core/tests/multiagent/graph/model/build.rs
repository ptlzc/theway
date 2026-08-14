use super::*;

#[test]
fn build_run_passes_budget_and_tools_from_def() {
    let def = run_def(
        "budget",
        vec![
            DagNodeDef {
                max_iterations: Some(8),
                tools: Some(vec!["read".into(), "bash".into()]),
                ..node_def("a", "general", "t1", &[])
            },
            node_def("b", "general", "t2", &["a"]),
        ],
    );
    let run = build_run(&def);

    let a = run.node("a").unwrap();
    assert_eq!(a.max_iterations, Some(8));
    assert_eq!(a.tools, Some(vec!["read".into(), "bash".into()]));

    let b = run.node("b").unwrap();
    assert_eq!(b.max_iterations, None);
    assert_eq!(b.tools, None);
}
