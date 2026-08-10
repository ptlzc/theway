//! dag_plan 子集兼容性验证 demo。
//!
//! 对照 dag-orchestrator 扩展声明的 mermaid 子集:
//!   graph TD|LR (or flowchart) · A["agent: task"] · A --> B ·
//!   A --> B & C (standard multi-target) · A -.-> B · %% comments
//! 逐项验证 mermaid-rs-parser 的行为;逗号多目标已从 dag_plan 删除
//! (pi-src cab2995),此处仅展示 mmdr 对它的静默容错。

use mermaid_rs_parser::{EdgeStyle, parse_mermaid};

fn show(title: &str, src: &str) {
    println!("── {title}");
    println!("  输入: {src:?}");
    match parse_mermaid(src) {
        Ok(out) => {
            let g = &out.graph;
            println!(
                "  方向: {:?}  节点: {}  边: {}",
                g.direction,
                g.nodes.len(),
                g.edges.len()
            );
            for n in g.nodes.values() {
                println!("    node {} shape={:?} label={:?}", n.id, n.shape, n.label);
            }
            for e in &g.edges {
                println!("    edge {} --{:?}--> {}", e.from, e.style, e.to);
            }
        }
        Err(e) => println!("  解析失败: {e}"),
    }
    println!();
}

fn main() {
    // 1. 基础子集: graph TD + 双引号 label + 实线/虚线边
    show(
        "graph TD, A[\"agent: task\"], --> 与 -.->",
        "graph TD\nA[\"explorer: 调研\"] --> B[\"planner: 计划\"]\nB -.-> C[\"checker: 验证\"]",
    );

    // 2. flowchart LR + 分号分隔
    show(
        "flowchart LR, 分号单行",
        "flowchart LR; A[\"exec: 实现\"] --> B[\"verify: 验证\"]",
    );

    // 3. 真实 mermaid 多目标语法: &
    show(
        "多目标 & (真实 mermaid 语法)",
        "graph TD\nA[\"explore\"] --> B[\"impl-api\"] & C[\"impl-web\"]",
    );

    // 4. 非标准逗号多目标: A --> B, C (removed from dag_plan, cab2995)
    //    mmdr itself silently mangles it — theway's wrapper reports an error.
    show(
        "多目标 , (非标准, 已删除)",
        "graph TD\nA[\"explore\"] --> B[\"impl-api\"], C[\"impl-web\"]",
    );

    // 5. %% 注释行
    show(
        "%% 注释",
        "%% dag run\nflowchart LR\nA[\"a\"] --> B[\"b\"] %% inline",
    );

    // 6. 链式边 A --> B --> C
    show("链式边", "graph LR; A[\"a\"] --> B[\"b\"] --> C[\"c\"]");

    // 7. Dotted style stays distinguishable in the IR for downstream use.
    let out = parse_mermaid("graph TD; A --> B").unwrap();
    let _ = EdgeStyle::Dotted;
    println!(
        "Dotted 样式在 IR 中可区分: {:?} / {:?}",
        out.graph.edges[0].style,
        EdgeStyle::Dotted
    );
}
