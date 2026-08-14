# 设计：工具块渲染、吞吐统计、彩虹转轮、状态修复、选区复制

## 1. 工具调用块（feed_render.rs）

现状：tool call 是 `⏵ name` + dim args 单行，result 折叠为一行摘要（`tools_expanded` 控制）。
改为块化（解决 #33 遗留的"工具调用平铺"）：

```
⏵ bash(command="cat foo", timeout=60)
   │ $ cat foo
   │ hello
   │ world
   │ …(2 more lines)
```

- 首行：`⏵ name` + 参数摘要（现状已有，保留样式）。
- result 预览：从 tool result 文本取前 5 行（按 `\n` 切），每行加左边框缩进；
  超过 5 行时第 5 行后加 `…` 省略行。`tools_expanded` 时保持现状全量展开。
- 预览行沿用 feed_render 的 markdown 行处理（避免裸换行破坏样式），但宽度语义与
  首行一致（content 宽度）。
- 缓存不变：块级渲染仍在 FeedRenderCache 指纹范围内，展开/折叠切换只改 opts
  （`tools_expanded` 已是 FeedRenderOptions 成员）。

## 2. thinking 块统计行

thinking 块（Full/Peek 模式）头部渲染统计行：

```
⏵ thinking · 1.2k char            c/s: 84 · output: 1.2k
```

- 左侧：thinking 文本字符数（human 格式：`<1k` 时原样、`1.2k`、`100.2k`，1 位小数）。
- 右侧：`c/s`（见 §4 的 CPS 计量器，取最近 1s 滑动窗）+ `output`（最近一轮
  output_tokens，human 格式同左）。
- Hidden 模式不渲染统计行（整块已是折叠态）。

## 3. composer busy 工作带统计

busy 带（#37 的 3 行 band：转轮 + shimmer working + elapsed）右侧追加：

```
⠿ working 12.4s               84 char/s · input: 57.1k · output: 1.2k
```

- `char/s`：CPS 计量器（§4）。
- `input: xxk`：ContextUsage.input_tokens（最近一轮）。
- `output: xx.xk`：ContextUsage.output_tokens（最近一轮）。
- 无 usage 数据时只显示 char/s。

## 4. CPS 计量器 + callback 转轮（pixel_loader.rs）

### 4.1 CPS 计量器（TUI 本地，无 daemon 依赖）

流式路径（apply_snapshot 的文本增量）累计字节：滑动窗口 1s，记录窗口内字节数 →
`char/s`。实现：

```rust
struct CpsMeter {  // ui/stats.rs 新文件
    window: VecDeque<(Instant, usize)>,  // (时刻, 累计字节)
    // record(bytes) 每帧末调用；cps() 返回滑动 1s 窗内速率
}
```

回调：`record` 产出的 `cps` 每次变化超过阈值（如 ±10%）时回调
`PixelFrame::advance(cps)`。

### 4.2 转轮速度映射

- 基准速度（无流式输出，等工具调用）：~1 step / 250ms（现状视觉节奏）。
- 加速：`step_delay = clamp(base_delay / (1 + cps / 200), 20ms, base_delay)`——cps
  越高转得越快，20ms 封顶（防抖防转爆）；cps 回落（工具调用等待期）自然回落到基准。
- 帧驱动不变：转轮仍由 TUI 帧循环（spinner_frame tick）渲染，`advance(cps)` 只调整
  step 映射——不增加独立定时器，不增加每帧成本。

### 4.3 彩虹色序列 + 9 点旋转顺序（输入需求原文）

颜色：彩虹轮换（红橙黄绿青蓝紫粉），每步沿色环前进，9 点各自取偏移色形成
彩虹拖尾——pixel_loader 现有 HSV 轮转（`cell_color`）保留，色相推进改由
callback step 驱动。

点亮顺序（用户给定的 9 点旋转参考；编号 = 第一轮点亮次序，蛇形绕排）：

```text
第一轮（9 格全亮）       第二轮（旋转阵列，尾部 1 格灭）    第三轮（再灭 1 格，7 格亮）
1 2 3                    8 3 2                            8 9 5
6 5 4                    7 4 1                              1 4
7 8 9                    6 5                                2 3
```

以此类推：每轮把上一轮的点亮阵列旋转、亮格数递减（9 → 8 → 7 → …）。第三轮
的用户 ASCII 有错位（相对第二轮，9 号位重现、6/7 号位消失），不强行推导几何
规则——实现按「旋转阵列 + 亮格递减」语义落表，tmux capture 对照参考校准，
无法对齐时 ask_user 确认第三轮精确布局。

编码：pixel_loader 内每轮一张点亮次序表（行优先索引的 `[usize]` 数组，同
现有 `Orbit` 的 `ORDER` 写法），单测逐格钉死；顺序表决定 lit 波前，颜色由
HSV 偏移独立决定（顺序与颜色解耦）。现有 Drive/Dots/Orbit 三变体由统一
`RainbowSpinner` 收编：9 点顺序表替换 Drive/Dots 的 chevron 波前，Orbit 的
「尾部格子常灭」语义保留在第二、三轮的熄灭格里。

### 4.4 三处统一

- composer busy 带：现有 PixelFrame::render(tick) → 改为 shared 组件
  `pixel_loader::rainbow_frame(step, cps)`。
- thinking 块指示器：thinking 流式期间在统计行左侧渲染 3×3 迷你转轮（同组件）。
- dag subagents 指示器：sidebar/dag 渲染处的运行中指示（同组件，小尺寸变体）。

## 5. ctx 百分比修复 + 状态行

### 5.1 修复恒定 100%

根因：daemon `wire_snapshot` 的 `usage.total_tokens` 取 `cost.tokens.total_tokens`
（会话累计），TUI 用它除以 context_window → 一旦超过窗口恒 100%。

修法（daemon 侧最小改动）：`wire_snapshot` 的 usage 改为「最近一轮」——
取最后一条 assistant 消息的 usage（input/output/cache/total），不再用会话累计
cost。字段语义注释同步更新。TUI 端计算不变。

### 5.2 状态行移除 working / multiline

prompt_chrome 渲染时不再输出 working 标志（busy 已由工作带表达）与 multiline 标志；
`PromptChrome` 对应字段删除或置空（保留结构兼容，置空即可）。

## 6. composer 右上角 features

状态行/输入框顶栏右上角渲染激活特性标签。数据源已齐备，无新 wire 通道：

- `latest.sidebar.runtime`（wire 已有 `runtime: Vec<String>`，daemon 在
  wire_sidebar_snapshot 填 `panel_status.trigger_features`，即 ui_mode_panel.rs
  `active_trigger_features()` 的 dedup/cycle suppress/fire-once rules/inject-and-run）。
- 图形特性推导：`latest.dags` 中任一 run `kind == "dag"` → `graph engine`；
  `kind == "goal"` 或 `latest.goal.is_some()` → `goal`。
- 渲染：dim `·` 分隔串置于 chrome 顶行右端，无特性时不占位。

## 7. 加速度滚动 + composer 滚轮

### 7.1 加速度滚动

- 状态：`scroll_repeat: u32`（当前滚轮方向/键的连续 repeat 计数）+ 方向；
  键盘滚动事件（Press/Repeat）按相同方向连续到达时递增，方向改变或 Release 时复位。
- 步长：`step = SCROLL_STEP * mult`，`mult = min(1.0 + repeat * 0.1, 1.5)`——
  每多一次 repeat 加 0.1，1.5x 封顶（"不要加得太快"）。首按恒 1.0x。
- 鼠标滚轮滚动保持 SCROLL_STEP（不参与加速度）；鼠标滚轮连滚过快由终端事件
  频率天然限制，不做额外加速。
- 单测：mult 序列（1.0 → 1.1 → … → 1.5 封顶）、方向切换/Release 复位。

### 7.2 composer 滚轮浏览

- `handle_mouse_scroll`：鼠标位于 `last_text_area`（composer 输入框矩形）时，
  把 MouseEvent 转给 `textarea.input(Event::Mouse(...))`，由其滚动内部视图；
  仅当鼠标在 feed 区域才走 feed 滚动。Shift+滚轮由 textarea 处理横向。
- 边界：单行输入框（无折行）滚轮无操作，不抢事件；拖拽调高后矩形变化以
  `last_text_area` 实时值为准。
- 单测：滚轮命中 text_area → textarea 滚动、feed 不动；命中 feed → feed 滚动。

## 8. DAG 状态带（dag band）

### 8.1 位置与布局

- 仅当 `latest.dags` 非空渲染；位于 feed 与 busy 带之间（composer 状态栏之上）。
  多 run 时最多显示 2 组，超出 `… N more`。
- 每 run：1 个头行 + 节点行（可折行），总高上限 4 行（1 头 + 3 节点行）。

```
⠿ dag-2 · issue-38-tui-polish · 2/6 · c/s 84
  ✓ 1-blocks · ✗ 2-daemon-usage · ▶ 3-spinner · · 4-status · × 4-scroll
```

- 头行：`dag-{n}` 前缀取 run.id（引擎已编号），name 截断，进度 done/total
  （Succeeded+Skipped 数 / 节点数），右侧 run 级 `c/s`。
- 任一节点 running → 头行左侧渲染统一迷你转轮（pixel_loader 小尺寸变体，
  3-spinner 节点提供）。

### 8.2 节点状态样式

| 状态 | 符号 | 颜色 |
|---|---|---|
| pending | `·` | dark gray |
| ready | `▸` | yellow |
| running | `▶`（带转轮） | cyan |
| succeeded | `✓` | green |
| failed | `✗` | red |
| cancelled | `×` | dark gray + strikethrough |
| skipped | `↷` | gray |

节点按 wire 顺序（引擎定义序）渲染，` · ` 分隔；failed/cancelled 节点 error 摘要
截断至 20 字符附在节点后（dim）。

### 8.3 run 级 c/s 统计

- 复用 3-spinner 的 CpsMeter：每 run 一个实例（HashMap<run_id, CpsMeter>），
  每帧记录 `sum(node.output_tokens)`（快照 delta），1s 滑动窗 → `c/s`。
- 转轮速度映射与 busy 带同一套（cps 快转得快、封顶、回落）。

### 8.4 实现

- 新文件 `ui/dag_band.rs`：纯渲染函数 `render_dag_band(area, dags, meters, tick)`
  + 状态样式表 + 单测（构造 WireDagRunSnapshot 各状态断言符号/颜色/截断）。
- `ui/mod.rs` 只加调用点：render() 里 feed 段与 busy 段之间，`latest.dags`
  非空时先压缩 feed 面积再画 band；CpsMeter 集合放在 App 状态。

## 9. 迭代预算与工具 allowlist（编排层）

### 9.1 数据流（现状 + 改动点）

```
dag_plan 节点 JSON {maxIterations?, tools?}
        │  node_def_from_json()  [daemon/src/tools/dag_tools/utils.rs]
        ▼
DagNodeDef { max_iterations: Option<u32>, tools: Option<Vec<String>> }   ← 新增字段
        │  build_run()           [core/multiagent/graph/model/mod.rs]
        ▼
DagNode   { max_iterations, tools }                                     ← 新增字段
        │  NodeLauncherImpl::launch()  [core/multiagent/graph/node_launcher.rs]
        ├── launch.max_iterations = node.max_iterations.unwrap_or(spec 默认 300)
        └── tools = filter_tool_set(resolver(agent), node.tools)        ← 新增
        ▼
runner::run_agent(AgentRunOptions { launch, tools, … })                 ← 消费现有字段
```

subagent 工具路径对称：

```
subagent({subagent_type, max_iterations?, tools?, …})  [daemon/src/tools/subagent.rs]
        ├── launch.max_iterations = params.max_iterations.unwrap_or(spec 默认 300)
        └── tools = filter_tool_set(resolver(spec), params.tools)
        ▼
runner::run_agent(AgentRunOptions { … })
```

### 9.2 预算默认 300

`crates/theway-daemon/src/agent_specs.rs`：

```rust
pub const DEFAULT_MAX_ITERATIONS: u32 = 300;
```

- 全部 5 个普通 spec（explorer/planner/executor-coder/checker/general）用它；
  goal-evaluator 保持 `max_iterations: 1`。
- 注释写明预算策略：300 是 code-harness 预算（编译/修复循环需要）；短平快任务
  由 orchestrator 按提示词指引降到 4–32。
- 引擎（`theway-core`）不改：它只执行 `AgentRunParams.max_iterations`，默认值
  归属 app 层 spec 表。

### 9.3 覆盖语义

覆盖发生在「解析 launch 之后、run_agent 之前」，两个入口同一模式：

```rust
let mut launch = launch;                       // spec 默认 300
if let Some(n) = override { launch.max_iterations = n; }
```

- 覆盖值 0 的语义：`max_iterations: 0` 在 agent loop 里是「立即超限」——文档注明
  不要传 0（提示词写 4–32 范围）。
- DAG 节点路径在 `node_launcher.rs` 读取 `node.max_iterations`（`DagNode` 运行时
  状态随 `DagNodeDef` 持久化，restore 后覆盖仍生效）。

### 9.4 工具 allowlist

#### filter_tool_set（core，共享）

`crates/theway-core/src/multiagent/runner.rs`：

```rust
pub fn filter_tool_set(
    tools: Vec<Arc<dyn AgentTool>>,
    allow: &[String],
) -> Result<Vec<Arc<dyn AgentTool>>, String>
```

- `allow` 空 → 原样返回（默认全量语义）。
- 每个 allow 名字必须存在于 `tools`（按 `definition().name` 匹配）；任一未知 →
  `Err("unknown tool in allowlist: {name} (available: …)")`。
- 命中过滤后返回；顺序保持原集顺序（按定义序过滤）。

#### 默认工具集（不变）

`daemon/src/tools/assembly.rs::subagent_tools()` 现在就是「orchestrator 工具 -
subagent - dag_*」+ local tools（`subagent_engine_tools` = skill 家族 + memory，
再加 local）。本 change 不重构该集合，只在其结果上应用 allowlist。

#### 两个入口

- `subagent` 工具：执行时 `filter_tool_set(resolver(subagent_type), allow)`，
  Err → `AgentToolError::Message` 直接作为工具结果返回给 orchestrator
  （可见、可重试）。
- `dag_plan` 节点：`DagNodeDef.tools` + `DagNode.tools`；node_launcher 在 filter
  失败时 `on_node_completed(…, success: false, error: 消息)`（节点同步失败，
  orchestrator 通过 dag_inspect 看到原因；不 panic）。

### 9.5 提示词更新

- `subagent` 工具 description 加两段：
  > Budget: the subagent defaults to 300 LLM-turn attempts — the code-harness
  > budget (compile → fix loops need it). For short, fast tasks (a quick read, a
  > single check) lower max_iterations to a reasonable range like 4-32.
  > Tools: by default the subagent gets every orchestrator tool except dag_* and
  > subagent; pass tools: ["read", "bash"] to restrict it to specific tools
  > (unknown names fail the call).
  schema 加 `max_iterations`（number）与 `tools`（array of string）。
- `dag_plan` 工具 description 节点字段列表改为 `{id, agent, task, dependsOn?,
  timeout?, cwd?, model?, thinking?, maxIterations?, tools?}`，加两段：
  > Node budgets: every node's subagent defaults to 300 LLM-turn attempts
  > (code-harness budget — compile/fix loops need it); for short, fast tasks
  > (a quick read, a single check) set maxIterations to a smaller range like 4-32.
  > Node tools: by default a node's subagent gets the orchestrator tool set minus
  > dag_* and subagent; to restrict a node to specific tools set
  > tools: ["read", "bash"] (unknown tool names fail the node).
  schema 加 `maxIterations`（number）与 `tools`（array of string）。

### 9.6 持久化

- `DagNode` 新增两字段均 `#[serde(default)]`（旧持久化数据兼容）。
- `PersistedNode` 镜像两字段；`hydrate` 还原时写入 `DagNode`；
  `snapshot`（to_persisted）同步拷贝。Running 节点跨进程恢复后覆盖仍生效。

### 9.7 解析（dag_plan）

`node_def_from_json`（daemon/tools/dag_tools/utils.rs）：

```rust
max_iterations: n.get("maxIterations").and_then(|v| v.as_u64()).map(|v| v as u32),
tools: n.get("tools").and_then(|v| v.as_array()).map(|a| {
    a.iter().filter_map(|x| x.as_str().map(String::from)).collect()
}),
```

mermaid 形式不支持这两字段（标签只有 `agent: task`），构造处补 `None`。

### 9.8 边界与决策

- **不引入 per-spec 工具差异**：spec 表保持「同一默认集」，差异只在启动时由
  orchestrator 显式指定（保持 spec 概念简单）。
- **不把 300 写进引擎**：引擎没有默认；所有默认都在 app 层 spec 表，与现有
  「引擎只消费 launch 数据」分层一致。
- **goal-evaluator 不变**：单轮评估器保持 1。
- **不新增 wire/proto**：编排层内部变更，状态面无需暴露预算/工具面。

## 10. 验证

- 单测：块渲染（5 行折叠/展开）、统计行格式（human 数字 1 位小数）、CPS 滑窗、
  转轮速度映射（cps→delay 单调封顶）、usage 最近一轮、滚动加速度序列
  （1.0→1.5 封顶 + 复位）、滚轮命中区域路由、dag band（各状态符号/颜色/截断、
  c/s 计量）、filter_tool_set（空 allow 全量 / 命中过滤 / 未知名 Err / 顺序保持）、
  build_run 传递新字段、persist roundtrip 带新字段、node_launcher override 生效、
  node_def_from_json 解析 maxIterations/tools。
- e2e（tmux capture-pane）：长工具调用块折叠视觉、流式时 char/s 与转轮加速、
  工具等待期转轮回落、ctx% 随最近一轮变化、长按滚动键加速滚动、composer 滚轮
  浏览、composer 拖拽调高回归（#37 已有功能）、dag 运行中状态带渲染（节点状态
  着色 + c/s 跳动）。
- e2e（编排，可选）：真跑一个 `tools: ["bash"]` 的节点，断言工具面收窄
  （读文件失败、bash 可用）；一个 `maxIterations: 8` 的短任务正常完成。
- 门禁：`make check` / `make test` / `make lint` / `make fmt-check`。
- 选区：仅保留现有高亮（鼠标拖拽 + Ctrl+Space + Shift+方向键），不新增复制。
