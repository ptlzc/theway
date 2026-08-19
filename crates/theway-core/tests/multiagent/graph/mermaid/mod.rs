use super::*;

mod error_paths;
mod helpers_and_rendering;
mod output;

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
    // Standard mermaid `&` multi-target (the non-standard comma form was
    // removed in pi-src cab2995 and must now be reported as an error).
    let res =
        parse_mermaid("graph TD\nA[\"a: 1\"] --> B[\"b: 2\"] & C[\"c: 3\"]\nB -.-> D[\"d: 4\"]");
    assert!(res.errors.is_empty(), "{:?}", res.errors);
    let b = res.nodes.iter().find(|n| n.id == "B").unwrap();
    assert_eq!(b.depends_on, Some(vec!["A".to_string()]));
    let c = res.nodes.iter().find(|n| n.id == "C").unwrap();
    assert_eq!(c.depends_on, Some(vec!["A".to_string()]));
    let d = res.nodes.iter().find(|n| n.id == "D").unwrap();
    assert_eq!(d.depends_on, Some(vec!["B".to_string()]));
}

#[test]
fn parse_hyphen_ids_survive_mmdr() {
    // `impl-api` / `1-explore` are core dag_plan ids; the vendored mmdr parser
    // treats `-` as edge syntax, so preprocess must rewrite and map back.
    let res = parse_mermaid(
        "graph TD\n1-explore[\"explorer: 调研\"] --> impl-api[\"impl: 实现\"] & impl-web[\"impl: 网页\"]",
    );
    assert!(res.errors.is_empty(), "{:?}", res.errors);
    let ids: Vec<&str> = res.nodes.iter().map(|n| n.id.as_str()).collect();
    assert_eq!(ids, vec!["1-explore", "impl-api", "impl-web"]);
    let impl_api = res.nodes.iter().find(|n| n.id == "impl-api").unwrap();
    assert_eq!(impl_api.depends_on, Some(vec!["1-explore".to_string()]));
    let impl_web = res.nodes.iter().find(|n| n.id == "impl-web").unwrap();
    assert_eq!(impl_web.depends_on, Some(vec!["1-explore".to_string()]));
}

#[test]
fn parse_comma_multi_target_reports_error() {
    // The old dag_plan extension syntax `A --> B, C` is not standard mermaid;
    // it must be a clear error, not silently mangled nodes.
    let res = parse_mermaid("graph TD\nA[\"a: 1\"] --> B[\"b: 2\"], C[\"c: 3\"]");
    assert!(
        res.errors
            .iter()
            .any(|e| e.contains("Line 2") && e.contains("unable to parse target node")),
        "{:?}",
        res.errors
    );
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
fn parse_label_with_ampersand_is_not_split() {
    // `&` inside a quoted label must not be treated as a multi-target split.
    let res = parse_mermaid("graph TD\nA[\"a: x & y\"] --> B[\"b: 2\"]");
    assert!(res.errors.is_empty(), "{:?}", res.errors);
    assert_eq!(res.nodes.len(), 2);
    let a = res.nodes.iter().find(|n| n.id == "A").unwrap();
    assert_eq!(a.task, "x & y");
}

#[test]
fn parse_reports_bad_lines() {
    let res = parse_mermaid("graph TD\nhello world\nA[\"a: 1\"] --> B,");
    assert!(
        res.errors
            .iter()
            .any(|e| e.contains("Line 2") && e.contains("unable to parse"))
    );
    assert!(
        res.errors
            .iter()
            .any(|e| e.contains("Line 3") && e.contains("unable to parse target node"))
    );
    // B registered via the edge but has no label.
    assert!(
        res.errors
            .iter()
            .any(|e| e.contains("missing a task description"))
    );
}

#[test]
fn parse_chain_edges_and_dedupe() {
    // A --> B --> C plus explicit node lines for B must not duplicate nodes.
    let res = parse_mermaid(
        "graph TD\nA[\"a: 1\"] --> B[\"b: 2\"] --> C[\"c: 3\"]\nB[\"b: 2\"]",
    );
    assert!(res.errors.is_empty(), "{:?}", res.errors);
    assert_eq!(res.nodes.len(), 3);
    let c = res.nodes.iter().find(|n| n.id == "C").unwrap();
    assert_eq!(c.depends_on, Some(vec!["B".to_string()]));
}
