# dag-node-graph-render

Issue: #41

## Problem

DAG/subagent 运行状态目前只有文本表达：

- TUI 的 DAG 状态带（`ui/dag_band.rs`）是「状态符号 + 节点 id」的文本行。
- dag_plan / dag_status 工具输出 mermaid 源码文本，feed 里按纯文本显示。

用户希望节点渲染成图（盒子 + 箭头），而不是文本。

## 可行性结论（已调查，答"有没有办法"：有）

仓库已有现成能力，不必造轮子：

- `crates/theway-markdown/src/mermaid.rs`：完整的 mermaid → Unicode 框图渲染器
  （feed 里 ` ```mermaid` 围栏已用它画图）。支持 graph/flowchart TD/LR/RL/BT、
  矩形/圆角/菱形、箭头边、`max_width` 超宽回退（fallback 源码框）。
  目前 `pub(crate)`，需公开 API。
- `crates/mermaid-parser`：dag_plan 已用它解析 flowchart 子集。
- `WireDagRunSnapshot` 已带 nodes（id/status/agent/depends_on 边）+ direction，
  可直接合成 mermaid 源码送渲染器，无需协议改动。

## What changes

1. **theway-markdown 公开 mermaid 渲染 API**：`mermaid` 模块的
   `MermaidStyles` / `MermaidArt` 与 `render()` 转公开（`render_mermaid_art`，
   styles 提供 `Default` 构造），lib.rs 导出。纯增量，不碰渲染逻辑。
2. **TUI DAG 状态带渲染框图**（`ui/dag_band.rs`）：对每个展示的 run 合成
   mermaid 源码——`graph {direction}` + `id["glyph id"]` 节点（状态符号进
   标签）+ depends_on 边；经 `render_mermaid_art` 渲染成框图替换现在的
   节点文本行；header 行（`dag-1 · name · 2/7 · c/s`）保留。约束：
   - `max_width` = band 宽度；渲染器回退（超宽/源码框）时降级为现有文本行。
   - band 高度预算内只渲染放得下的 run（超出仍显示 `… N more`）。
   - 节点标签截断交给渲染器（MAX_LABEL 28），状态符号用现有 glyph 集合。
3. **feed 中 tool result 的 mermaid 围栏渲染成图**（`ui/feed_render.rs`）：
   ToolResult 块展开时，检测 ` ```mermaid` 围栏区间，把围栏内容走现有的
   mermaid 渲染路径（与 assistant 块一致），其余行保持现状 —— dag_plan /
   dag_status 输出直接受益。

## Out of scope

- PNG/SVG 图片、外部 mermaid CLI（mmdc）、web 渲染。
- 六边形/泳道/gantt 等高级图形（渲染器不支持的不做）。
- 节点状态差异化着色（v1 状态符号进标签；边框分色留待后续）。

## Impact

- 无协议/wire 改动；daemon 不动。
- 新增 theway-markdown 公开 API 面（向后兼容）。
- DAG 状态带行数计算随框图高度变化（`band_rows` 同步改）。

## Acceptance

- 运行 dag 时 TUI 状态带显示盒子 + 箭头节点图，状态符号可见。
- dag_plan / dag_status 输出的 mermaid 围栏在 feed 中渲染成图。
- 无 dag run / 图表过宽时行为与现状一致（不渲染或回退文本行）。
- `make check` / `make test` / `make lint` / `make fmt-check` 通过。
