# Phase 4 规划: theway-ratatui-inline（内嵌滚动流模式）

> 本阶段为**交互形态变更**，实施前必须经用户确认；openspec change
> `tui-grok-render-port` 的默认执行范围（Phase 0–3）**不包含**本阶段。

## 一、目标形态与用户价值

现状：`theway` 是传统全屏 REPL——`enter_tui()` 进 alternate screen，会话
结束后恢复原终端屏幕，feed 内容不留在终端的滚动历史里。

目标（Grok Build 的 inline 形态）：TUI 渲染在普通终端的滚动流中
（`xai-ratatui-inline` 的视口终端），退出后已输出的对话留在 scrollback，
可以像 `less -F` / 普通 CLI 输出一样回看、复制；不再独占屏幕。

价值：会话结束后内容可追溯；多会话/与 shell 交替使用时切换成本低；与
`theway --grpc` 等行输出模式共用同一输出面。

## 二、移植步骤

源：`/root/workspace/grok-build/crates/codegen/xai-ratatui-inline/`
（Apache-2.0, 源 revision `5d08d7e`；`terminal.rs` 派生自 ratatui 的
`Terminal`, 上游出处见其文件头与 Grok 仓库 `THIRD-PARTY-NOTICES`）。
目标 crate：`crates/theway-ratatui-inline/`（命名规则同 Phase 1–3：
`xai-` → `theway-`，`NOTICE` 注明捐赠方与源 revision，修改文件加
Apache §4(b) 变更声明）。

1. **依赖预检**：inline 依赖 ratatui 0.29 的 `unstable-backend-writer`
   feature（与 Phase 2 已引入的 `unstable-widget-ref` 同源）。确认
   theway 锁定的 ratatui 0.29.0 含该 feature（与 Grok 同版本，预计通过；
   不通过则本阶段整体冻结并回报）。
2. **机械移植**：拷贝 `src/`（terminal / scrollback / segment / resize /
   common + tests），包名与内部引用替换为 `theway-` 前缀；`NOTICE` 落盘；
   根 `Cargo.toml` members 追加。源测试一并拷贝，`cargo test -p
   theway-ratatui-inline` 全绿（同 Phase 2 预检模式）。
3. **TUI 侧切换（最小步）**：`ui/mod.rs` 的 `run()` 从
   `enter_tui()`/`leave_tui()`/`Terminal::new` 改为 inline 视口构造；
   `render()` 的 `terminal.draw` 改为视口内绘制；退出路径把已渲染行
   `emit_to_scrollback`。此步仅形态切换，不改布局逻辑。
4. **滚轮/鼠标语义重映射**：inline 模式下鼠标事件来自普通终端流而非
   alternate screen 捕获；`handle_mouse_scroll` 与选择逻辑按新事件源适配。
5. **OSC 8 输出接通**：Phase 3 已埋的 URL 检测（`underline_links`）升级为
   真实 OSC 8 序列输出（inline `Terminal::flush_with_links` 能力）。
6. **非 TTY / headless 降级**：`run_headless()` 路径保持不变；TTY 检测
   失败或环境不支持 inline 时回退全屏 REPL（feature gate）。

## 三、交互形态变更面

- **退出行为**：不再"恢复原屏"，已输出内容留在终端 scrollback。
- **鼠标/滚动**：feed 滚动与终端原生滚动语义叠加，需定义边界（TUI 滚动
  结束后的输入如何接管）。
- **daemon 帧节奏耦合**：inline 输出是追加流，快照 diff（Phase 0 的
  append 路径）与 emit_to_scrollback 需要节流对齐，避免终端被帧刷爆。
- **输入行定位**：inline 视口没有固定底部输入框锚点，输入行随输出流动；
  Grok 的 segment/scrollback 机制需要本地化适配。
- **兼容面**：Windows Terminal / conhost / 各终端仿真器的 inline 行为差异
  （Grok 的 `terminal.rs` 已处理部分，移植测试需覆盖）。

## 四、回滚策略

- **feature gate**：全屏 REPL 与 inline 模式由同一启动参数/feature 开关
  选择，默认全屏 REPL（当前形态），inline 为 opt-in。
- **降级检测**：运行期检测终端能力（alternate screen / 鼠标协议），
  异常时自动回退全屏模式并输出一行提示。
- **回滚路径**：每步独立 commit；若切换后体验不达预期，`git revert`
  接入 commit 即可回到 Phase 3 末状态，不影响已交付的渲染原语。

## 五、风险与开放问题

1. 终端的 inline 能力差异（尤其 Windows conhost）是主要质量风险；需要
   至少 Windows Terminal + iTerm2 + GNOME Terminal 三种环境的手工验证。
2. `emit_to_scrollback` 与 feed 增量 diff 的性能交互未验证（长会话的
   尾部追加是否仍高效）。
3. 输入行随流移动的交互模式是形态级变化，用户接受度未知——**必须先在
   独立分支试跑，经用户确认后才合入 main**。
4. syntect 语法高亮（Phase 3 裁剪掉的 syntax 模块）若用户需要，可与
   本阶段合并实施（依赖 `unstable-widget-ref` 已就绪）。

## 用户确认门槛

本阶段实施前必须获得用户明确确认（至少一次"确认进入 Phase 4"的指示），
确认内容包括：是否接受"退出后内容留在 scrollback"的形态变化、是否要求
先出体验分支、是否同时引入 syntect 语法高亮。未确认前，任何实现性
commit 不得进入 Phase 4 范围。
