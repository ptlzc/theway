# Tasks: tui-grok-render-port

Issue: #24. DAG 编排（openspec/config.yaml 规范）：节点按文件归属串/并，
每个节点完成即小步 commit（Conventional Commits，引用 #24）。
agent 映射：executor = executor-coder，verify = checker，writer = executor-writer。

graph TD
  0-event-feed --> 1-adopt-feed-render
  1-port-markdown --> 1-adopt-feed-render
  1-adopt-feed-render --> 1-verify
  1-verify --> 2-port-textarea
  2-port-textarea --> 2-adopt-input
  2-adopt-input --> 2-verify
  2-verify --> 3-port-render
  3-port-render --> 3-adopt-primitives
  3-adopt-primitives --> 3-verify
  3-verify --> 4-inline-plan
  4-inline-plan --> final-verify

## 0-event-feed [executor] — TUI 接入事件面 + feed 增量（纯 theway 代码, 治卡）

文件: `crates/theway-tui/src/ui/mod.rs`（+ 其 tests）。

- [x] 0.1 `apply_frame`: `StreamEvent` 不再丢弃——feed 相关事件映射为增量
      patch; 先盘点 `StreamEvent` 六种 kind 中哪些 TUI 目前全靠 snapshot 重
      刷（subagent/graph 面），把能增量的改为增量, 其余保留 snapshot 路径
- [x] 0.2 feed 增量: `apply_snapshot` 由整表 `replace_blocks` 改为按
      `feed_blocks` 尾部 diff 追加/替换（保持现有 `feed_changed` 短路语义）;
      feed 布局（换行/高度）按脏块重算而非全量
- [x] 0.3 渲染节流: 事件帧合并到下一个 100ms tick 统一 draw, 避免每事件一
      帧; spinner 帧只在有变化时重绘
- [x] 0.4 测试: 事件帧增量不丢块、diff 追加与替换边界、节流后帧数下降;
      `cargo test -p theway-tui` 全绿
- [x] 0.5 commit `perf(#24): tui applies stream events incrementally`

## 1-port-markdown [executor] — 移植 theway-markdown-core（与 0-event-feed 并行）

文件: 新建 `crates/theway-markdown-core/*`, 根 `Cargo.toml`
（members + workspace.dependencies 增 pulldown-cmark）。

- [x] 1.1 复制 `xai-grok-markdown-core` src → `crates/theway-markdown-core/src/`,
      包名/前缀 `xai-`→`theway-`, license Apache-2.0, edition/rust-version 走
      workspace 继承; `NOTICE` 注明捐赠方 SpaceXAI + 源 revision `5d08d7e`
- [x] 1.2 复制其测试; `cargo test -p theway-markdown-core` 全绿（移植=机械
      拷贝, 不加行为改动）
- [x] 1.3 commit `feat(#24): port theway-markdown-core from Grok Build`

## 1-adopt-feed-render [executor] — feed 渲染换 markdown 分析 [depends: 0-event-feed, 1-port-markdown]

文件: `crates/theway-tui/Cargo.toml`, `crates/theway-tui/src/feed_render.rs`。

- [x] 1.4 feed 块文本经 `theway-markdown-core` 分析（GFM/删除线/math/任务
      列表口径）后渲染, 保留现渲染样式; 删除手搓段落切分逻辑
- [x] 1.5 快照/增量路径共用同一渲染入口; 更新 `ui/tests.rs` 断言
- [x] 1.6 commit `feat(#24): feed rendering uses theway-markdown-core`

## 1-verify [checker] [depends: 1-adopt-feed-render]

- [x] 1.7 `cargo test --workspace` 全绿; clippy `-D warnings`; fmt-check;
      对照 Grok 行为抽查 markdown 边界（代码块/表格/单波浪线不删除线）
- [x] 1.8 commit 仅在有修正时产生 (`fix(#24): ...`)

## 2-port-textarea [executor] [depends: 1-verify]

文件: 新建 `crates/theway-ratatui-textarea/*`, 根 `Cargo.toml`
（members + workspace.dependencies 增 textwrap / unicode-segmentation /
tui-scrollbar）。

- [x] 2.1 依赖预检: 确认 `ratatui` 0.29 具备 `unstable-widget-ref` feature
      且 `ratatui-core` 0.1 与 workspace ratatui 版本一致; 不满足则本节点
      只产出预检报告, 暂停 2-adopt-input 并回报用户（fallback: 保留
      tui-textarea 0.7）
- [x] 2.2 复制 `xai-ratatui-textarea` src + tests → 新 crate, 命名/前缀替换,
      `NOTICE`（tui-textarea fork 上游出处）; `cargo test -p
      theway-ratatui-textarea` 全绿
- [x] 2.3 commit `feat(#24): port theway-ratatui-textarea from Grok Build`

## 2-adopt-input [executor] [depends: 2-port-textarea]

文件: `crates/theway-tui/Cargo.toml`, `crates/theway-tui/src/ui/mod.rs`
（输入构造处）, `crates/theway-tui/src/ui/app_input.rs`。

- [x] 2.4 输入组件切换: `tui-textarea` 依赖替换为 `theway-ratatui-textarea`,
      保留现有按键绑定语义（历史/补全/多行高度≤6）; 编辑能力（undo/word
      样式）默认不启用新行为
- [x] 2.5 `cargo test -p theway-tui` 全绿; 更新输入相关测试
- [x] 2.6 commit `feat(#24): input uses theway-ratatui-textarea`

## 2-verify [checker] [depends: 2-adopt-input]

- [x] 2.7 workspace 全绿 + clippy/fmt; 手工核对输入行为无回归（IME/中文/
      粘贴/历史）
- [x] 2.8 commit 仅在有修正时产生

## 3-port-render [executor] [depends: 2-verify]

文件: 新建 `crates/theway-pager-render/*`, 根 `Cargo.toml`
（members + workspace.dependencies 按所选模块）。

- [x] 3.1 选取独立原语模块: syntax（syntect 依赖需确认）、osc8、
      scrollbar、highlight、theme、line_utils; 不搬 scrollback 布局/block
      模型; 每模块带 `NOTICE` 条目
- [x] 3.2 复制所选模块 src + tests, 命名/前缀替换; `cargo test -p
      theway-pager-render` 全绿
- [x] 3.3 commit `feat(#24): port theway-pager-render primitives from Grok Build`

## 3-adopt-primitives [executor] [depends: 3-port-render]

文件: `crates/theway-tui/Cargo.toml` + feed 渲染相关文件。

- [x] 3.4 接入 OSC8 链接（工具输出/feed 中的 URL）与滚动条原语; 语法高亮
      仅当 syntect 依赖经确认后接入（否则留 TODO 说明缺失）
- [x] 3.5 `cargo test -p theway-tui` 全绿; 新增渲染测试
- [x] 3.6 commit `feat(#24): feed uses theway-pager-render primitives`

## 3-verify [checker] [depends: 3-adopt-primitives]

- [x] 3.7 workspace 全绿 + clippy/fmt; 视觉抽查（链接可点、滚动条、配色）
- [x] 3.8 commit 仅在有修正时产生

## 4-inline-plan [writer] [depends: 3-verify] — 规划 only, 不实现

文件: `openspec/changes/tui-grok-render-port/phase4-inline.md`。

- [x] 4.1 写 Phase 4 规划文档: `theway-ratatui-inline` 移植步骤、交互形态
      变更面（alternate screen → 内嵌滚动流）、回滚策略、风险（与 daemon
      帧节奏耦合、非 TTY 降级路径）; 文档结尾明确
      **实施前必须经用户确认, 本 change 不自动执行**
- [x] 4.2 commit `docs(#24): phase 4 inline-mode plan (gated on user confirmation)`

## final-verify [checker] [depends: 4-inline-plan]

- [x] F.1 `make check` / `make lint` / `make test` 全绿; 每个新 crate 的
      NOTICE 与许可证头抽查; grep 确认无 `xai-` 前缀残留（除 NOTICE 中
      提及捐赠方）
- [x] F.2 汇总各节点 commit, 更新 issue #24 完成状态
