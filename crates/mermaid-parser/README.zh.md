# mermaid-rs-parser

[English](README.md) | 中文

`mermaid-rs-parser` 是 theway 使用的 vendored Mermaid 解析阶段。它将 Mermaid 源文本转换为结构化 [`Graph`](src/ir.rs)，不包含 SVG renderer、布局引擎、字体栈或 CLI。

公开函数 [`parse_mermaid`](src/parser.rs) 返回 [`ParseOutput`](src/parser.rs)，其中包含解析后的 graph 和可选初始化 directive。中间表示记录检测到的图类型、方向、节点、边、subgraph 和图类型专用数据。

## 消费方契约

`theway-core` 的 DAG 适配器为 `dag_plan` 接受更小的 flowchart 契约。适配器预处理 DAG 节点标识与行级语法，调用本 crate 完成 Mermaid 解析，再恢复标识并推导 agent、task 和依赖字段。子集校验属于适配器；本 crate 保持为通用解析阶段。

## Vendored 源码

[`src/parser.rs`](src/parser.rs)、[`src/ir.rs`](src/ir.rs) 和 [`src/error.rs`](src/error.rs) 承载源自 `mermaid-rs-renderer`（`mmdr`）0.3.1 的解析阶段代码。[`src/lib.rs`](src/lib.rs) 和 [`Cargo.toml`](Cargo.toml) 组成本地 crate shell。上游署名与许可证见 [`LICENSE`](LICENSE)。

Vendored parser 保持单文件，便于与来源代码比较。修改前阅读 [`AGENTS.md`](AGENTS.md)；parser 与 DAG 适配器的边界见 [`docs/architecture.md`](docs/architecture.md)。

## 验证

```bash
cargo test -p mermaid-rs-parser
cargo doc -p mermaid-rs-parser --no-deps --document-private-items
cargo run -p mermaid-rs-parser --example dag_plan_demo
```
