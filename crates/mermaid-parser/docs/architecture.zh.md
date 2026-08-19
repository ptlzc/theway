# Mermaid parser 架构

[English](architecture.md) | 中文

## 职责

`mermaid-rs-parser` 负责从源文本解析到中间表示。它识别图 header、校验初始化 directive、分派到图类型专用 parser 并返回 `ParseOutput`；不执行终端或图形布局，也不定义 theway 的 DAG schema。

## 解析流水线

[`parse_mermaid`](../src/parser.rs) 先校验 Mermaid 初始化 directive 并检测 [`DiagramKind`](../src/ir.rs)，再分派到 flowchart、sequence、class、state、ER、pie、mindmap、journey、timeline、Gantt、requirement、GitGraph、C4、Sankey、quadrant、ZenUML、block、packet、Kanban、architecture、radar、treemap 或 XY chart parser。

[`ParseOutput`](../src/parser.rs) 包含 [`Graph`](../src/ir.rs) 和可选初始化 JSON。`Graph` 使用 `BTreeMap` 保存节点，使用有序 vector 保存边与 subgraph，同时保存方向、类型和图类型专用集合。解析失败使用 `anyhow::Result`；[`ParseError`](../src/error.rs) 为更严格调用方提供带类型的错误词汇。

## DAG 适配器边界

[`theway-core/src/multiagent/graph/mermaid.rs`](../../theway-core/src/multiagent/graph/mermaid.rs) 负责 `dag_plan` 的 flowchart 子集。它的 preprocessor 分类源行、为 vendored parser 规范化带连字符标识，并收集带行号诊断；postprocessor 恢复标识、拆分 `agent: task` label、推导依赖，并拒绝与声明节点集合不一致的 parser 输出。

本 crate 不吸收 DAG 专用 label、标识重写、依赖规则或用户纠错策略。适配器保持独立，使 vendored parser 可继续服务更广泛的 Mermaid 中间表示。

## 源码归属

[`src/parser.rs`](../src/parser.rs)、[`src/ir.rs`](../src/ir.rs) 和 [`src/error.rs`](../src/error.rs) 是 vendored 源文件组。[`src/lib.rs`](../src/lib.rs)、[`Cargo.toml`](../Cargo.toml)、example 和文档属于本地 shell。`parser.rs` 不受工作区文件大小限制，并为来源比较保持单文件。
