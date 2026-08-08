use super::*;

#[test]
fn parse_basic_graph() {
    let res = parse_mermaid("graph TD\nA[\"explorer: 调研代码库\"] --> B[\"planner: 制定计划\"]\n");
    assert!(res.errors.is_empty(), "{:?}", res.errors);
    assert_eq!(res.direction, Direction::Td);
    assert_eq!(res.nodes.len(), 2);
    assert_eq!(res.nodes[0].id, "A");
    assert_eq!(res.nodes[0].agent, "explorer");
    assert_eq!(res.nodes[0].task, "调研代码库");
    assert_eq!(res.nodes[0].depends_on, None);
    assert_eq!(res.nodes[1].agent, "planner");
    assert_eq!(res.nodes[1].depends_on, Some(vec!["A".to_string()]));
}

#[test]
fn parse_multi_target_and_dotted_edges() {
    let res =
        parse_mermaid("graph TD\nA[\"a: 1\"] --> B[\"b: 2\"], C[\"c: 3\"]\nB -.-> D[\"d: 4\"]");
    assert!(res.errors.is_empty(), "{:?}", res.errors);
    let b = res.nodes.iter().find(|n| n.id == "B").unwrap();
    assert_eq!(b.depends_on, Some(vec!["A".to_string()]));
    let c = res.nodes.iter().find(|n| n.id == "C").unwrap();
    assert_eq!(c.depends_on, Some(vec!["A".to_string()]));
    let d = res.nodes.iter().find(|n| n.id == "D").unwrap();
    assert_eq!(d.depends_on, Some(vec!["B".to_string()]));
}

#[test]
fn parse_comments_and_directions() {
    let res = parse_mermaid(
        "%% header comment\nflowchart LR\nA[\"a: 1\"] --> B[\"b: 2\"] %% trailing\n%% tail",
    );
    assert!(res.errors.is_empty(), "{:?}", res.errors);
    assert_eq!(res.direction, Direction::Lr);
    assert_eq!(res.nodes.len(), 2);

    let tb = parse_mermaid("graph TB\nA[\"a: 1\"]");
    assert_eq!(tb.direction, Direction::Td);
}

#[test]
fn parse_fullwidth_colon_and_quotes() {
    let res = parse_mermaid(
        "graph TD\nA[\"explorer：调研代码库\"]\nB['writer: 写文档']\nC[noquote: 裸标签]",
    );
    assert!(res.errors.is_empty(), "{:?}", res.errors);
    assert_eq!(res.nodes[0].agent, "explorer");
    assert_eq!(res.nodes[0].task, "调研代码库");
    assert_eq!(res.nodes[1].agent, "writer");
    assert_eq!(res.nodes[1].task, "写文档");
    assert_eq!(res.nodes[2].agent, "noquote");
    assert_eq!(res.nodes[2].task, "裸标签");
}

#[test]
fn parse_reports_bad_lines() {
    let res = parse_mermaid("graph TD\nhello world\nA[\"a: 1\"] --> B,");
    assert!(
        res.errors
            .iter()
            .any(|e| e.contains("第 2 行") && e.contains("无法解析"))
    );
    assert!(
        res.errors
            .iter()
            .any(|e| e.contains("第 3 行") && e.contains("无法解析目标节点"))
    );
    // B registered via the edge but has no label.
    assert!(res.errors.iter().any(|e| e.contains("缺少 task")));
}
