//! 边界行为实测: dag_plan 子集输入 → mmdr 行为
use mermaid_rs_parser::{DiagramKind, parse_mermaid};

fn probe(name: &str, src: &str) {
    println!("── {name}");
    match parse_mermaid(src) {
        Err(e) => println!("  ERR: {e}"),
        Ok(p) => {
            if p.graph.kind != DiagramKind::Flowchart {
                println!("  kind={:?} (非 flowchart, 跳过)", p.graph.kind);
                return;
            }
            println!(
                "  direction={:?} nodes={} edges={}",
                p.graph.direction,
                p.graph.nodes.len(),
                p.graph.edges.len()
            );
            for (id, n) in &p.graph.nodes {
                println!("    node {id}: label={:?}", n.label);
            }
            for e in &p.graph.edges {
                println!("    edge {} -> {} (style={:?})", e.from, e.to, e.style);
            }
        }
    }
}

fn main() {
    probe(
        "标准多目标 &",
        "graph TD\nA[\"a: 1\"] --> B[\"b: 2\"] & C[\"c: 3\"]",
    );
    probe(
        "逗号多目标(非标准)",
        "graph TD\nA[\"a: 1\"] --> B[\"b: 2\"], C[\"c: 3\"]",
    );
    probe("残缺目标 B,", "graph TD\nA[\"a: 1\"] --> B,");
    probe("未知行 hello world", "graph TD\nhello world\nA[\"a: 1\"]");
    probe("单引号 label", "graph TD\nA['writer: 写文档']");
    probe("无引号 label", "graph TD\nA[noquote: 裸标签]");
    probe(
        "链式边",
        "graph TD\nA[\"a: 1\"] --> B[\"b: 2\"] --> C[\"c: 3\"]",
    );
    probe("全角冒号", "graph TD\nA[\"explorer：调研代码库\"]");
    probe(
        "%% 注释 + 尾随注释",
        "%% header\ngraph LR\nA[\"a: 1\"] --> B[\"b: 2\"] %% trailing",
    );
    probe("graph TB 默认方向", "graph TB\nA[\"a: 1\"]");
    probe("孤立节点", "graph TD\nA[\"a: 1\"]\nB[\"b: 2\"]");
    probe(
        "id 含下划线",
        "graph TD\nexplore-api[\"explorer: 调研\"] --> impl_api[\"impl: 实现\"]",
    );
}
