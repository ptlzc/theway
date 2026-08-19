# Mermaid parser 修改规则

本文件适用于 `crates/mermaid-parser/`。同时遵循 [`../../AGENTS.md`](../../AGENTS.md) 和 [`docs/architecture.md`](docs/architecture.md) 的归属边界。

## Vendored 源码

- [`src/parser.rs`](src/parser.rs) 保持单文件；它是工作区 800 行限制的明确例外。
- 不对 [`src/parser.rs`](src/parser.rs)、[`src/ir.rs`](src/ir.rs) 和 [`src/error.rs`](src/error.rs) 做机械拆分、格式化、重命名或清理；来源比较依赖较小 diff。
- 上游同步必须作为独立变更，显式核验上游来源与许可证。
- 本地 lint allow 只附着在 vendored 代码边界，不为满足工作区样式而重写来源代码。

## 边界

- 布局、SVG、字体、CLI 和终端渲染依赖不得进入本 parse-only crate。
- Theway 的 `dag_plan` 子集策略保留在 [`../theway-core/src/multiagent/graph/mermaid.rs`](../theway-core/src/multiagent/graph/mermaid.rs)。
- 除非同一变更同步更新消费者，否则保持稳定 graph 顺序和 [`src/lib.rs`](src/lib.rs) 的公开重新导出。
- 来源署名与许可证变化记录在 [`LICENSE`](LICENSE) 和 crate 文档中。

## 验证

运行 `cargo test -p mermaid-rs-parser` 和 `cargo doc -p mermaid-rs-parser --no-deps --document-private-items`。可能影响 DAG 适配器的变更还要运行 `cargo test -p theway-core multiagent::graph::mermaid`。
