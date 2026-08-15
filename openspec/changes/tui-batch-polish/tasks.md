# Tasks

```mermaid
graph TD
  A["1-features: composer 右上角只留 graph engine (#39)"] --> B["2-wrap: composer 单行超宽折行 (#40)"]
  B --> C["3-snake: 单行彩虹贪食蛇 busy 带 (#42)"]
  C --> D["4-thinking-stats: c/s + in/out 接线 (#44)"]
  D --> E["5-theme: theme.toml 块背景/padding/对齐 + composer (#43,#49)"]
  E --> F["6-dag-graph: DAG 框图渲染 (#41)"]
  F --> G["7-completion-scroll: 补全弹层自动翻页 (#46)"]
  G --> H["8-slash-catalog: 弹层 skill:: 与 mcp: 条目 (#47)"]
  H --> I["9-new-session: /new 命令 create+switch (#52)"]
  I --> J["10-text-selection: 字符级文本选中 + 复制 (#53)"]
  J --> K["11-status-panel: 面板拖拽调宽 + /status-panel 菜单 (#54)"]
  K --> L["12-fork-picker: TUI fork 补全/交互选择/可切换 (#55)"]
  L --> M["13-fresh-attach: 重进默认 new session + /resume (#56)"]
  M --> N["14-reload: reload tool + revision + TUI 热重载 (#50)"]
  O["15-tool-rename: 工具名统一 snake_case (#48)"] --> Q["17-verify"]
  P["16-agents-doc: daemon 定位写入 AGENTS.md (#51)"] --> Q["17-verify"]
  N --> Q["17-verify: make ci + tmux e2e + close #39-#56"]
```

主链全串行：ui/mod.rs 被 1/2/3/4/5/7/8/9/10/11/12/13/14 修改，feed_render.rs 被
4/5/6/10 修改，按仓库规则共享文件节点串行。15-tool-rename 只碰 daemon
文件（tool 定义 + 测试 + listener + system_prompt）、16-agents-doc 只碰
AGENTS.md，两者与主链文件不相交，并行跑。每节点小步 commit
（feat(#子issue)），17-verify 基于最新 HEAD 复核并逐条 close。

- [x] 1-features — `crates/theway-tui/src/ui/mod.rs` + `ui/tests.rs`：
  - `feature_labels(runtime, dags, has_goal)` → `feature_labels(dags)`：
    删 runtime 透传与 goal 推导；dag-kind run 存在 → `["graph engine"]`，
    否则空。调用点只传 `&self.latest.dags`。
  - 测试：删 `feature_labels_passes_runtime_features_through` /
    `feature_labels_goal_from_run_or_active_goal_once` /
    `feature_labels_combined_order`；`chrome_top_divider_shows_feature_labels`
    fixture 去 runtime/goal 只留 dags。
  - 验收：`cargo check -p theway-tui`；`cargo test -p theway-tui` 全绿；
    trigger 面板 Runtime 区块不受影响（测试未删）。
- [x] 2-wrap — `crates/theway-tui/src/ui/mod.rs` + `ui/tests.rs`：
  - `composer_rows(input_area_width: u16)`：拖拽覆盖优先；否则
    `content_width = input_area_width.saturating_sub(5)`，
    `rows = self.input.desired_height(content_width)`；`rows > 6` 时用
    `content_width - 1` 复核（scrollbar 预留），clamp 1..=6；render 调用点
    传 `frame.area().width`。
  - 测试：超宽单行（200 字符）`composer_rows` ≥ 2 且 ≤ 6；超长封顶 6；
    `input_is_single_line()` 仍 true；拖拽覆盖优先。
  - 验收：`cargo check -p theway-tui`；`cargo test -p theway-tui` 全绿。
- [x] 3-snake — 新 `crates/theway-tui/src/ui/snake_loader.rs` +
  `ui/mod.rs` + `ui/tests.rs`：
  - `snake_frame(step, cps) -> SnakeFrame`（轨道固定 9 格；蛇头三角波
    0→8→0 折返；蛇尾节 i 跟随头历史位置、越界暗色；lit 随 trail 衰减，
    trail 2→8 随 cps；色相 `step*15° + i*40°` 经 hsv_to_rgb；字形 `●`）。
  - pixel_loader.rs：`hsv_to_rgb` 转 `pub(crate)`。
  - ui/mod.rs：删 BUSY_STATUS_ROWS（status 恒 1 行）；render_busy_status
    单行——蛇轨道 x+1 起 9 格、标签 x+12 起、stats 右对齐同行。
  - 测试：蛇折返轨迹 / 蛇尾跟随与翻面 / 彩虹渐变 / trail 封顶 / 暗色
    轨道点恒在；`busy_status_shows_pixel_loader_with_elapsed` 改 1 行带
    断言（composer 顶边框 = 标签行 + 1）。
  - 验收：`cargo check -p theway-tui`；`cargo test -p theway-tui` 全绿；
    dag band mini spinner 测试不回归。
- [x] 4-thinking-stats — `ui/mod.rs` + `feed_render.rs` + `feed_cache.rs` +
  `ui/tests.rs`：
  - ui/mod.rs 组装 opts：`thinking_cps = self.cps_meter.cps()`；
    `thinking_input_tokens` / `thinking_output_tokens` =
    `latest.usage.input_tokens/output_tokens`（>0 才有效）。
  - feed_render.rs：`FeedRenderOptions` 加 `thinking_input_tokens`；
    `thinking_stats_line` 右侧 `c/s: N · in: X · out: Y`；**手写 PartialEq**
    只比较 thinking_mode / tools_expanded / 主题色（cps/in/out/
    spinner_phase 不参与，防 feed_cache 每帧失效）。
  - feed_cache.rs：无改动（stats 行 rebuild 路径已在，新值随流式生效）；
    跑现有测试确认增量语义不回归。
  - 测试：thinking_stats_line 格式断言（in/out 出现）；opts 相等语义
    测试（cps 变化仍相等、mode 变化不等）。
  - 验收：`cargo check -p theway-tui`；`cargo test -p theway-tui` 全绿。
- [x] 5-theme — 新 `crates/theway-tui/src/theme.rs` + `feed_render.rs` +
  `prompt_chrome.rs` + `ui/mod.rs` + `ui/tests.rs`（#43 + #49）：
  - `Theme`（颜色角色 + 块布局 + composer 样式，Default = 现状 const 色）
    + `Theme::load`（`[colors]` 表 + `[blocks.*]` 段 + `[composer]` 表
    解析；未知键/非法 hex/未知 align warn 回落、无文件默认）。
  - 块布局：`[blocks.{user,assistant,tool,thinking}]` 段
    `bg`（色）/`padding`（列数，默认 1，0 允许）/`align`（left|right，
    默认 left）；背景 + 左右 padding 列铺满块宽，align=right 内容右缘
    对齐。
  - composer：`[composer]` 表 `border_focused/border_unfocused/prefix/
    text/bg/info_text`；prompt_chrome.rs 颜色 const 改走 theme。
  - feed_render.rs：const 迁为默认值来源；FeedRenderOptions 加主题色/
    布局字段（结构，参与 PartialEq）；Tool 块（标题+args）、ToolResult
    （展开/预览）、Thinking（Full/Peek + stats 行）设背景 + padding +
    对齐（背景铺满块行宽，文本宽度归一后补 bg span，空行纯 bg）。
  - ui/mod.rs：启动 `Theme::load(~/.theway/theme.toml)` 一次存
    `App.theme`，组装 opts 填入。
  - 测试：Theme 解析（默认/覆盖/未知/非法/缺失）；自定义主题下 tool /
    thinking 块 buffer bg + padding + 右对齐断言；composer 换色断言；
    默认主题 feed_render / prompt_chrome 现有测试全绿。
  - 验收：`cargo check -p theway-tui`；`cargo test -p theway-tui` 全绿。
- [x] 6-dag-graph — `crates/theway-markdown` + `ui/dag_band.rs` +
  `ui/feed_render.rs`：
  - markdown：`mermaid.rs` 的 `MermaidStyles`/`MermaidArt` 转 `pub`、
    `MermaidStyles` 加 Default、`render()` 转 `pub render_mermaid_art`；
    lib.rs 导出。
  - dag_band.rs：`synthesize_mermaid(run)`（`graph {direction}` +
    `id["{glyph} {id}"]` + depends_on 边）；render 每 run 先试
    `render_mermaid_art(.., Some(band_w))`，成功且行数 ≤ 高度预算画框图，
    否则回退文本行；`band_rows` 按框图高度计；header / `… N more` 不变。
  - feed_render.rs：ToolResult 展开分支检测 ```mermaid 围栏（逐行扫
    fence 起止）→ 复用 markdown mermaid 渲染路径成图，非围栏行现状。
  - 测试：合成源码快照；回退分支；band_rows 框图高度；tool result 含
    mermaid 围栏 → `┌─┐` 盒子断言。
  - 验收：`cargo check --workspace`；`cargo test -p theway-markdown -p
    theway-tui` 全绿。
- [x] 7-completion-scroll — `ui/mod.rs` + `ui/app_input.rs` + `ui/tests.rs`：
  - `App` 加 `completion_scroll: usize`；`completion_prev` / `completion_next`
    / `cycle_completion` 移动后调整窗口：idx < scroll → scroll = idx；
    idx ≥ scroll + COMPLETION_POPUP_MAX → scroll = idx - MAX + 1。
  - `render_completions`：渲染 `completions[scroll..scroll+MAX]`，高亮按
    绝对下标匹配（`i == scroll 偏移`）。
  - `refresh_completions` / `clear_input` / `accept_completion` 重置
    scroll = 0；Up/Down/Tab 环绕不变。
  - 测试：20+ 项列表 Down 连按越过 8 项后高亮仍在窗口内（buffer 断言
    高亮行可见）；Up 回翻跟随；刷新重置到顶部。
  - 验收：`cargo check -p theway-tui`；`cargo test -p theway-tui` 全绿。
- [ ] 8-slash-catalog — `ui/mod.rs` + `ui/tests.rs`：
  - `collect_slash_commands` 增 `mcp_tool_names: &[String]` 参数（调用点
    传 `latest.sidebar.mcp.tool_names`）；追加条目：每个已启用 skill →
    `skill::{name}`（name 原样）；每个 mcp 工具 → `mcp:{tool}`。
  - 现有 `/skillname` 快捷与全部既有条目保留；SlashCompleter 去重排序
    不变（from_commands 已处理）。
  - 测试：collect_slash_commands 断言 skill:: / mcp: 条目存在；
    弹层过滤（/skill:: 只剩 skill 条目）。
  - 验收：`cargo check -p theway-tui`；`cargo test -p theway-tui` 全绿。
- [ ] 9-new-session — `ui/app_turns.rs` + `ui/mod.rs` + `ui/tests.rs`（#52）：
  - dispatch_slash 加 `"/new"` 分支：`self.client.create_session(None).await`
    → 取 summary.session_id → `self.switch_session(id).await`（已有，
    app_goal.rs）；失败 error_line；成功 system_line 提示新 id。
  - busy 不加额外防护（与 /session switch 一致：切换中止当前 turn）。
  - collect_slash_commands 加 `LOCAL_COMMANDS: &["new"]`（TUI 本地命令，
    不进 DAEMON_COMMANDS）；/help 文案本地清单加 /new。
  - 测试：slash_new 触发 create_session + SwitchSession{id} 命令；补全
    列表含 /new；失败路径 error_line。
  - 验收：`cargo check -p theway-tui`；`cargo test -p theway-tui` 全绿。
- [ ] 10-text-selection — 新 `ui/selection.rs` + `feed_render.rs` +
  `ui/mod.rs` + `ui/app_input.rs` + `ui/tests.rs`（#53）：
  - `FeedSelection { anchor: (usize, usize), head: (usize, usize) }`
    （uncapped 行 + 显示列）；`ordered()` 归一方向；列钳制按行文本
    宽度（unicode-width）；`extract_text(lines)` 从渲染行 span 拼纯文本
    （首/尾行按列截断，行间 \n）；`highlight_cols(buf, line, c1, c2)` 按
    显示列切分 span 只涂 [c1,c2) 背景（BAND_STYLE）。
  - feed_render.rs：render_lines_window 选区参数 2D 化（整行涂色路径
    替换为列区间涂色；删除 highlight_line 整行压平逻辑或转 selection
    内部）。
  - ui/mod.rs：FeedSelection 2D；mouse down/drag 传列（col = 行内显示
    列，超行宽钳行尾）；Ctrl+Space 选可视页 (view.top,0)→(view.bottom,
    末行宽)；mouse-up 触发复制（system line `copied N chars · M lines`）。
  - ui/app_input.rs：Shift+Left/Right 按字符（钳行宽）、Shift+Up/Down
    保持列、Shift+PgUp/PgDn 按页；Ctrl+Shift+C 显式复制；Esc 清除不变。
  - 复制：`clipboard_image` 模块（或 selection 内）加 write 路径——
    arboard set_text（spawn_blocking）优先，失败回 OSC52。
  - 测试：列钳制、文本提取（首尾列截断 + 多行）、列区间涂色（buffer
    bg 断言）、mouse 列映射、键盘扩展、复制调用（mock）。
  - 验收：`cargo check -p theway-tui`；`cargo test -p theway-tui` 全绿。
- [ ] 11-status-panel — `ui/mod.rs` + `ui/app_turns.rs` + `ui/app_input.rs`
  + `ui/tests.rs`（#54）：
  - `enum SidePanelMode { Auto, Shown(u16), Hidden }`（App 字段，默认
    Auto）；渲染 split：mode → Option<width>（Auto = should_show_side_panel()
    && 宽 ≥100 时 Some(36)；Hidden = None；Shown(w) = w clamp
    [24, 内容宽-40] 且宽 ≥100 门槛共用），记录 `last_panel_area`。
  - 拖动：handle_mouse_down 在 feed 拖选分支**之前**加面板左边界命中
    （last_panel_area.x 单列、整面板高）→ `panel_drag =
    Some(PanelDrag { start_col, start_width })` 且 mode 转 Shown；drag 中
    `width = start_width + (start_col - col)` clamp [24,60]；**col ≥
    面板右缘（拖到最右边）或 width < 24 → Hidden**；mouse_up 结束拖动。
  - `/status-panel`（TUI 本地）：dispatch_slash 分支打开
    `status_panel_menu: Option<usize>`（二级菜单，选项 show/hide/auto）；
    handle_key 前置分支 Up/Down 移动、Enter 应用（Shown(36)/Hidden/Auto）、
    Esc 取消；render 居中弹层菜单（标题 "status panel"）。
  - LOCAL_COMMANDS 加 `status-panel`；/help 文案同步。
  - 测试：mode 渲染解析（Auto 有/无内容、Hidden、Shown clamp）、拖拽
    宽度计算与拖到最右关闭、菜单导航/应用/取消、/status-panel 打开菜单。
  - 验收：`cargo check -p theway-tui`；`cargo test -p theway-tui` 全绿。
- [ ] 12-fork-picker — `ui/mod.rs` + `ui/app_turns.rs` + `ui/app_input.rs`
  + `ui/tests.rs` + daemon `commands/session.rs`（#55）：
  - DAEMON_COMMANDS 加 "fork"；核对 daemon `Registry::with_daemon_commands()`
    全量命令名与 TUI 列表一致（diff 补齐其它缺失项）。
  - TUI 本地拦截 `/fork`：无参数 → 打开 `fork_picker` 弹层（当前会话
    `latest.feed_blocks` 的 User 块倒序：编号 + ≤60 字符预览；与 daemon
    编号序一致）；有参数直接转发 `/fork <n>`（现状路径）。
  - app_input.rs：弹层 Up/Down 移动、Enter 转发 `/fork <选中编号>` 并
    关闭、Esc 取消；render 居中弹层（列表 + "fork" 标题，复用
    completion 弹层样式）。
  - daemon ForkCommand 成功提示：`forked session {完整id} — /session
    switch {short} to continue there`（保留 CLI resume 提示行）；不自动
    切换。
  - 测试：补全含 /fork；弹层列表 = feed User 块倒序（fixture 断言）；
    Enter 转发文本为 `/fork <n>`；Esc 取消；daemon 提示文本断言。
  - 验收：`cargo check --workspace`；`cargo test -p theway-tui -p
    theway-daemon` 全绿。
- [ ] 13-fresh-attach — `startup/mod.rs` + `ui/mod.rs` + `ui/app_turns.rs`
  + `ui/app_input.rs` + `ui/tests.rs`（#56）：
  - startup `connect_or_spawn`：discover 复用路径返回标记 `reused:
    bool`；reused 且无 `--resume/--resume-id/--continue` 时 attach 后
    `client.create_session(None)` + `client.switch_session(id)`（/new
    同路径）；自己 spawn 的 daemon 不重复建（返回前标记 spawn）。
  - `/resume`（TUI 本地）：dispatch_slash 拦截 → `resume_picker:
    Option<ResumePickerState>`（client.list_sessions() 的
    (sessions, current_id)）；弹层行 = 短 id + name + busy/graph 标记
    （current 行标注）；Up/Down/Enter → switch_session 并关闭、Esc 取消；
    空列表 system line "no sessions to resume"。
  - ui/mod.rs：resume_picker 状态 + 渲染（复用 completion 弹层样式）；
    LOCAL_COMMANDS 加 `resume`；/help 文案同步。
  - busy 时 switch 排队：快照到达后自动呈现新会话（apply_snapshot 已有
    session_id 更新路径，验证即可）。
  - 测试：reused 路径触发 create+switch（mock client 断言命令序列）、
    spawn 路径不触发；/resume 弹层列表、Enter 发 SwitchSession{id}、
    Esc 取消、空列表提示；--resume-id 启动无回归。
  - 验收：`cargo check -p theway-tui`；`cargo test -p theway-tui` 全绿。
- [ ] 14-reload — daemon + transport + tui（#50）：
  - daemon 新 `tools/reload.rs`：AgentTool `reload`（snake_case），
    包装现有 `reload_everything`（skills/config/commands/triggers 重扫，
    与 /reload 命令共用逻辑不复制），执行成功 → 递增 runtime_revision；
    description 写明"安装 skill / 修改配置后调用以生效"。
  - `tools/assembly.rs` 注册；`turn/daemon.rs`：revision 计数器 +
    wire snapshot 携带。
  - transport `wire.rs`：WireSidebarSnapshot 加 `runtime_revision: u64`
    （serde default 0，向后兼容）。
  - tui `ui/mod.rs`：App 缓存 `last_runtime_revision`，apply_snapshot
    检测变化 → `Theme::load(~/.theway/theme.toml)` 重读 → 更新
    `App.theme`（TUI 自身 reload；runtime 侧 daemon 已完成）。
  - 测试：reload tool 调用 + revision 递增（daemon）；TUI revision
    变化触发 theme 重载断言（ui/tests.rs）。
  - 验收：`cargo check --workspace`；`cargo test -p theway-daemon -p
    theway-tui` 全绿。
- [x] 15-tool-rename — daemon-only（与主链文件不相交，并行）：
  - `tools/skill.rs`、`tools/skill_builder.rs`、`tools/install_skill/
    mod.rs`、`tools/remove_skill.rs`、`tools/set_skill_state.rs`、
    `triggers/cron/tools.rs`、`triggers/dynamic/tools.rs`：Tool.name +
    label() 按映射改 snake_case（Skill→skill、SkillBuilder→skill_builder、
    InstallSkill→install_skill、RemoveSkill→remove_skill、SetSkillState→
    set_skill_state、NewCronJob→new_cron_job、ListCronJobs→list_cron_jobs、
    RemoveCronJob→remove_cron_job、SetCronJobState→set_cron_job_state、
    NewTrigger→new_trigger、ListTriggers→list_triggers、RemoveTrigger→
    remove_trigger、SetTriggerState→set_trigger_state）；
    `tools/exec_shell.rs` label "Exec" → "exec"（name 已是 exec）。
  - `turn/listener.rs`：`tool_name == "Skill"` → `"skill"`；
    `Skill(...)` 显示串 → `skill(...)`。
  - `system_prompt.rs`：自然语言提及（call ListCronJobs、SkillBuilder、
    InstallSkill…）改新名。
  - description 自引用（skill_builder 描述里 InstallSkill 等）+ 代码
    注释提及同步。
  - daemon 测试名字断言（tests/tools/*、commands_e2e、
    dynamic_trigger_e2e、e2e_llm.rs）更新。
  - 验收：`cargo check -p theway-daemon`；`cargo test -p theway-daemon`
    全绿；grep 确认 `name: "[A-Z]` 在 Tool 定义中无残留（除 test Faux）。
- [x] 16-agents-doc — `AGENTS.md`（#51，只碰文档，并行）：
  - Workspace layout / Layering 附近加"daemon 定位"小节：daemon =
    会话/工具/触发/编排的运行时服务，面向协议层（transport 的 gRPC +
    HTTP/SSE/WS）；对客户端形态无概念（不区分 TUI/web/headless 脚本/
    其他程序），不携带 UI 概念（颜色/布局/按键）。
  - 边界规则：客户端专属外观与交互归 theway-tui；跨端新功能先定 wire
    契约，daemon 只做协议侧语义；需要客户端配合的行为用 snapshot 字段/
    事件表达（例：runtime_revision 通知客户端重读本地资源）。
  - 验收：`git diff AGENTS.md` 内容符合上述语义；无其它文件改动。
- [ ] 17-verify — `make check` + `make test`（workspace 全量）+ `make lint` +
  `make fmt-check`；tmux e2e 逐条：①composer 右上角仅 graph engine（无
  dag run 无标签，trigger 面板 Runtime 仍在）②长文本折行显示行首 + Up/Down
  历史/拖拽调高回归 ③busy 时彩虹蛇在 9 点轨道左右跑 + 折返 + 与 working
  同行（带 1 行、idle 无跳动）④流式 thinking 统计行 c/s 非零 + in/out
  随流更新 ⑤theme.toml 定制 tool/thinking 背景/padding/对齐铺满块宽 +
  composer 换色 + 无主题视觉回归 ⑥dag 状态带盒子+箭头图 + dag_status
  mermaid 围栏在 feed 成图 + 超宽回退 ⑦补全弹层 Down/Up 越界自动翻页、
  高亮始终可见 ⑧弹层含 skill:: 与 mcp: 条目、前缀过滤生效 ⑨工具调用块
  显示 snake_case 名（skill/trigger/cron 工具调用正常）⑩装 skill / 改
  theme.toml 后 LLM 调 reload → 新 skill 可 invoke + TUI 主题即时生效
  ⑪AGENTS.md 定位小节在库 ⑫/new 新 session：feed 清空、系统行提示、
  下一条消息进新 session、/sessions 可见新旧两个 ⑬鼠标拖选只高亮字符
  （跨行首尾按列截断）、释放后剪贴板含选区文本、Shift+方向键字符/行/页
  扩展、Ctrl+Shift+C 复制、Esc 清除 ⑭面板左边界拖动调宽（24-60 clamp）、
  拖到最右关闭、/status-panel 菜单 show/hide/auto 生效 ⑮/fork 补全可见、
  无参数弹层列出 user messages、选 #k 转发 /fork k → 新 session 完整 id +
  /session switch 提示、切换成功、Esc 取消弹层 ⑯daemon 存活时退出再进 →
  新 session（system line 新 id、feed 空）、/resume 弹层选旧会话 → feed
  恢复、--resume-id 启动无回归、spawn 新 daemon 无多余空 session；
  `gh issue close 39 40 41 42 43 44 46 47 48 49 50 51 52 53 54 55 56`，
  证据贴 #45 后 `gh issue close 45`。
