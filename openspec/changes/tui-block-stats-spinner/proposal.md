# TUI 打磨：工具块渲染、吞吐统计、彩虹转轮、状态修复

## 范围整合

本 change 合并两批任务的欠账为一份 openspec：

**近期清单（#37 之后，6 条）**：工具调用块化、thinking 统计行、working 吞吐统计、
彩虹 callback 转轮、ctx 100% 修复 + 去 working/multiline、右上角 features。

**本轮新增（2 条）**：滚动加速度（最多 1.5x）、composer 输入框滚轮浏览。

**#33 批次欠账（用户复核确认还欠的 2 条）**：工具调用平铺（= 近期 1）、ctx 用量
显示 bug（= 近期 6）。

**#33 批次已完成项（不在本计划）**：输入框光标、用户输入块渲染、mermaid 图、
Ctrl+O thinking 三状态（隐藏/查探/全显）、orchestrator thinking 结构化总结回填、
滚动 pinning（非底部时流式输出不打扰滚动位置）、文字选区高亮（鼠标拖拽 +
Ctrl+Space + Shift+方向键；用户明确只做选区，不做复制）。

## Why

#37 落地了 slash popup、粘贴对象、composer 拖拽和鼠标选区，但 feed 的工具调用仍然是
平铺单行，缺少块化结构（#33 遗留）；busy 转轮是固定 tick 驱动，与流式节奏脱节；
thinking 块没有字数/吞吐统计；右下角 ctx 百分比恒定 100%（daemon 发的是会话累计
token，除以 context window 后一旦超窗就永远 100%，#33 遗留）。

整合后的完整清单（9 条）：

1. tools 调用分块渲染：首行是指令，下面是 5 行输出，超过 5 行省略
2. thinking 块：一块内容 + 字数统计，右侧有 `c(har)/s`、`output: 1.2k`（最多 1 位小数）
3. composer 上面 working 右侧：`xx char/s`、`input: xxk`、`output: 100.2k`
4. 9 点转轮变彩虹色 + callback 机制转动（cps 快转得快 + 防抖，cps 慢回归正常速度）；
   composer / thinking / dag subagents 三处转轮统一使用
5. 右下角 100% cxt 有 bug（#33 遗留）；右下角不再显示 working 和 multiline
6. composer 右上角显示 feature（如 `graph engine | goal`）
7. 滚动支持加速度：按住滚动键逐渐加速，但不要加得太快，最多 1.5x 封顶
8. composer 输入框支持鼠标滚轮浏览（多行/折行内容滚轮查看）
9. dag 模式：在 composer 状态栏之上渲染 DAG 状态带——每个节点按运行/取消/成功/失败
   等状态着色，run 头显示 c/s 输出统计（与转轮统一 cps 驱动）

（选区只保留现有高亮，不做复制——用户确认。composer 顶部拖拽调高在 #37 已实现
[e4f1388]，不在本计划，但 verify 阶段回归验证。）

## What changes

**A. 工具调用块（feed_render）**：每个 tool call 渲染为一块——首行 `⏵ name args`，
下方最多 5 行 result 内容预览（尾部 `…` 省略）；`tools_expanded`（Ctrl+T）继续控制
全文展开。thinking 块头部加统计行：左侧字数（如 `1.2k char`），右侧 `c/s: xx` +
`output: 1.2k`。

**B. 吞吐测量（TUI 本地）**：流式增量计量器（每 token/文本 delta 累计字节 + 时间窗），
产出 `char/s`（CPS）。busy 工作带右侧显示 `xx char/s · input: xxk · output: 100.2k`，
input/output 取 daemon ContextUsage（当前/最近一轮，见 E）。

**C. 彩虹转轮（pixel_loader）**：PixelFrame 从固定 tick 改为 callback 驱动——
`advance(cps)` 由吞吐测量回调调用，旋转速度随 cps 加速、封顶（防抖防转爆）、
无流式输出时回落基准速度。颜色改为彩虹序列。composer busy 带、thinking 块指示器、
dag subagents 指示器（sidebar/dag 渲染处）统一走同一组件。

**D. 状态行修复**：右下角移除 working/multiline 标志（busy 已在工作带显示；
multiline 标志删除）；`xx% ctx` 改用最近一轮 token（daemon 需发 per-turn usage 或
TUI 用最后一条消息 usage 计算），修复恒定 100%。

**E. daemon 侧 usage 语义**：ContextUsage 已含 input/output/cache；现状取会话累计
cost（wire_snapshot 的 `cost.tokens`），改为「最近一轮」——取 agent state 最后一条
assistant 消息的 usage（`AgentMessage::Llm(Message::Assistant(a))` 的 `a.usage`），
不动 proto。

**F. composer 右上角 features**：状态行右侧（或 composer 顶栏右上）显示激活特性。
数据源已齐备无需新通道：`latest.sidebar.runtime`（wire 已有，daemon 填
ui_mode_panel 的 trigger 特性）+ 从 `latest.dags`（kind "dag" → "graph engine"、
kind "goal" → "goal"）与 `latest.goal`（Some → "goal"）推导的标签。渲染为 dim
`·` 分隔串置于 chrome 顶行右端。

**G. 加速度滚动（TUI）**：键盘滚动（Up/Down/PageUp/PageDown）连续按住时步长
逐渐加速——以按键 repeat 计数驱动，倍率从 1.0 渐增、1.5x 封顶（"不要加得太快"），
松开按键立即复位。鼠标滚轮与单次按键保持原步长（SCROLL_STEP）不变。

**H. composer 滚轮浏览（TUI）**：鼠标位于输入框区域时 ScrollUp/ScrollDown 转发给
ratatui-textarea（`input()` 处理滚轮滚动），多行/折行内容可滚轮浏览；滚轮事件不再
落到 feed 滚动。Shift+滚轮横向滚动（textarea 原生支持则透传）。拖拽调高已存在
（#37 e4f1388），仅回归验证。

**I. DAG 状态带（TUI，新文件 ui/dag_band.rs）**：`latest.dags` 非空时，在 feed 与
busy 带之间（composer 状态栏之上）渲染 DAG 状态带——每 run 一个头行
（`dag-2 · issue-38-tui-polish · 2/6 · c/s 84`，运行中带统一迷你转轮）+ 节点行
（状态符号 + id，按 wire 顺序、` · ` 分隔可折行）。节点状态样式：
pending `·` 暗灰 / ready `▸` 黄 / running `▶` 青（带转轮）/ succeeded `✓` 绿 /
failed `✗` 红 / cancelled `×` 暗灰删除线 / skipped `↷` 灰。run 级 c/s：各节点
output_tokens 之和在快照间的 delta / 1s 滑动窗（复用 C 的 CpsMeter，每 run 一个
实例），转轮速度同样由 cps 回调驱动。高度上限 4 行（1 头 + 3 节点行），多 run 截断
`…`。数据源 `WireStatus.dags`（wire 已带 per-node status/tokens/preview），无 proto
变更。

## Impact

- 纯 TUI + daemon usage 语义微调，无 proto 变更（ContextUsage 已够用）。
- 工具块默认折叠 5 行输出 → 长工具调用不再刷屏；Ctrl+T 展开行为不变。
- 转轮 callback 化后，无流式输出时转轮回落基准速度，不增加帧成本（仍是单帧渲染）。
- ctx 百分比修复后反映真实最近一轮占用。
- 选区保持现状（仅高亮），本 change 不引入复制路径。
