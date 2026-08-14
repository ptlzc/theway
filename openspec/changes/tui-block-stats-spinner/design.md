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

### 4.3 彩虹色序列

现状 9 点顺序（第二轮起旋转阵列）保留，颜色改为彩虹轮换（红橙黄绿青蓝紫粉）：
每步颜色沿色环前进，9 点各自取偏移色，形成彩虹拖尾。

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

状态行/输入框顶栏右上角渲染激活特性标签，来源 daemon sidebar runtime 特性：

- daemon 已发布 `trigger_features`（sidebar.runtime）；补一个明确的
  `features: Vec<String>`（如 `graph engine`、`goal`）或直接映射现有
  runtime 特性。为最小改动：TUI 从 sidebar.runtime + 会话配置推导
  `["graph engine", "goal"]` 等标签，渲染为 dim `·` 分隔串，置于 chrome 顶行右端。

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

## 8. 验证

- 单测：块渲染（5 行折叠/展开）、统计行格式（human 数字 1 位小数）、CPS 滑窗、
  转轮速度映射（cps→delay 单调封顶）、usage 最近一轮、滚动加速度序列
  （1.0→1.5 封顶 + 复位）、滚轮命中区域路由。
- e2e（tmux capture-pane）：长工具调用块折叠视觉、流式时 char/s 与转轮加速、
  工具等待期转轮回落、ctx% 随最近一轮变化、长按滚动键加速滚动、composer 滚轮
  浏览、composer 拖拽调高回归（#37 已有功能）。
- 门禁：`make check` / `make test` / `make lint` / `make fmt-check`。
- 选区：仅保留现有高亮（鼠标拖拽 + Ctrl+Space + Shift+方向键），不新增复制。
