# tui-batch-polish

Issue: #45（umbrella；子 issue #39-#44、#46-#51）

## 范围整合

本轮 12 条输入收拢为一个 change、一个 DAG 实施链（同 #38 先例）——
之前为每条单独建的 change（tui-composer-features-only-graph-engine /
tui-composer-single-line-wrap / dag-node-graph-render /
tui-busy-snake-loader / tui-theme-interface）已并入本 change 并删除，
理由：涉及文件高度重叠（ui/mod.rs ×7、feed_render.rs ×3），按仓库规则
共享文件节点必须串行，拆开反而制造合并摩擦。

## 完整清单（12 条）

**1. composer 右上角 features 只保留 graph engine（#39）**

- `feature_labels(runtime, dags, has_goal)` → `feature_labels(dags)`：去掉
  runtime 透传（dedup / cycle suppress / fire-once rules / inject-and-run）
  与 goal 推导；dag-kind run 存在时只返回 `["graph engine"]`。
- trigger 面板 Runtime 区块不动（trigger features 的家）；prompt_chrome
  渲染逻辑不动（空列表本就不占格子）。
- 单测：删 runtime 透传 / goal 推导用例，收窄保留用例。

**2. composer 单行输入超宽字符换行（#40）**

- `composer_rows()` → `composer_rows(input_area_width)`：拖拽覆盖优先；
  否则用 textarea `desired_height(content_width)` 算视觉折行数，
  `content_width = input 区宽 − 5`（chrome pad 2+1 + ❯ 2），> 6 行时按
  scrollbar 预留 1 列复核，clamp 1..MAX_INPUT_ROWS(6)。
- `input_is_single_line()` 仍按逻辑行（无 `\n`）判定——折行不影响历史
  导航 / slash 补全 / Enter；textarea 内核不改。

**3. busy 状态轮改单行彩虹贪食蛇（#42）**

- 9 点保留，排成 1 行轨道，总高度 ≤ working 标签高度 125%，且与 working
  标签**同一行水平对齐**（pi 式固定位置：busy 带 3 行 → 1 行，与 idle
  同高、无布局跳动）。pi 参考（~/pi-src/extensions/working-indicator）
  是单格盲文转轮；我们保留 9 点因为盲文单格无法逐点彩虹。
- 新 `ui/snake_loader.rs`：`snake_frame(step, cps) -> SnakeFrame`——蛇头
  （最亮）带渐暗彩虹蛇尾（色相 40°/节渐变 + 15°/步整体推进），沿 9 点
  轨道左右往返（三角波，折返时蛇尾翻到运动方向背面）；尾长随吞吐
  2→8 节；字形 `●`，未点亮轨道点暗色底；`hsv_to_rgb` 从 pixel_loader
  转 `pub(crate)` 复用。
- ui/mod.rs：删除 BUSY_STATUS_ROWS（status 恒 1 行）；render_busy_status
  单行：蛇轨道（x+1）+ working/计时/队列/↑scrolled（蛇后 2 格）+ stats
  右对齐。
- 保留 `pixel_loader::rainbow_frame`（dag band mini spinner 继续用）；
  速率→速度映射曲线不改。

**4. thinking 统计行 c/s + in/out 接线（#44，bug）**

- 现状：`thinking_cps` / `thinking_output_tokens` 从未赋值（opts 组装
  `..Default::default()`）→ 右侧恒 `c/s: 0 · output: 0`，且无 input。
- ui/mod.rs 组装 opts：`thinking_cps = self.cps_meter.cps()`；
  `thinking_input_tokens` / `thinking_output_tokens` = `latest.usage`
  最近一轮 input/output（#38 已改最近一轮）。
- feed_render.rs：`FeedRenderOptions` 加 `thinking_input_tokens`；统计行
  右侧改 `c/s: N · in: X · out: Y`（human 格式）。
- **防缓存退化**：FeedRenderOptions 手写 PartialEq——只比较结构性开关
  （thinking_mode / tools_expanded / 主题色），每帧变化的 cps / in / out /
  spinner_phase 不参与相等；否则 feed_cache 每帧整体失效、#34/#35 的
  增量渲染退化为全量。流式尾块每帧重渲染路径（IncrementalWrap / stats
  rebuild）新值自然生效；冻结历史块显示冻结时值。

**5. TUI theme 接口：块背景/padding/对齐 + composer 样式（#43 + #49）**

- pi 参考（~/pi-src/config/themes/dark-theway.json）：`vars` + `colors`
  角色映射（toolPendingBg/toolSuccessBg/toolErrorBg/toolTitle/toolOutput、
  thinking 五级、markdown、syntax）+ settings.json 主题选择。
- v1：新 `ui/theme.rs`——`Theme` 结构（默认 = 现状硬编码色，无
  theme.toml 时视觉完全不变）；`Theme::load` 解析 `~/.theway/theme.toml`
  （未知角色/非法值 warn 回落，缺段/缺键 = 默认）。
- 角色（#43）：user_text/user_bg/assistant_text/assistant_prefix/
  tool_title/tool_args/tool_result/tool_error/tool_running_bg/
  tool_success_bg/tool_error_bg/thinking_text/thinking_bg。
- 块布局（#49）：
  ```toml
  [blocks.user]          # 每块类型一段；缺省 = 默认
  bg = "#343541"         # 背景色（可选）
  padding = 2            # 块内水平 padding 列数（可选，默认 1，0 允许）
  align = "right"        # left | right（可选，默认 left = 现状视觉）

  [blocks.assistant] [blocks.tool] [blocks.thinking]  # 同构
  ```
  背景 + 左右 padding 列均铺背景、铺满块宽（真"方块"）；align=right
  块内容右缘对齐（背景仍满宽）；非法值 warn + 回落。
- composer 样式（#49）：`[composer]` 表
  `border_focused/border_unfocused/prefix/text/bg/info_text`（默认 =
  prompt_chrome 现有 const）——prompt_chrome.rs 颜色改走 theme。
- feed_render.rs：const 迁移为 theme 角色默认值；Tool 块（标题 + args +
  result 展开/预览）与 Thinking 块（Full/Peek）设背景且**铺满块行宽**；
  颜色经 FeedRenderOptions 透传（feed_cache 指纹覆盖；启动一次性加载）。
- 热重载：#50 的 reload 联动（runtime_revision 变化 → 重读 theme.toml）。
- 非目标：dag band 配色、多主题目录 + settings 选择。

**6. DAG 节点渲染成框图（#41）**

- theway-markdown：`mermaid.rs` 的 `MermaidStyles` / `MermaidArt` /
  `render()` 转公开（`render_mermaid_art` + Default styles），lib.rs 导出。
- dag_band.rs：每个 run 合成 `graph {direction}` 源码（`id["{glyph} id"]`
  + depends_on 边）→ `render_mermaid_art(src, styles, Some(band_w))` 画
  盒子+箭头图；超宽/超大回退现有文本行；header 行与 `… N more` 不变；
  `band_rows` 按框图高度计。
- feed_render.rs：ToolResult 展开时检测 ```mermaid 围栏 → 走现有 mermaid
  渲染路径成图（dag_plan / dag_status 输出直接受益）。
- 非目标：PNG/SVG、mmdc CLI、高级图形、节点状态分色边框（v1 状态符号进
  标签）。

**7. slash command 补全弹层自动翻页（#46）**

- 现状：弹层渲染固定窗口 `completions[0..COMPLETION_POPUP_MAX(8)]`，
  高亮下标循环任意项——Down 越过第 8 项后高亮跑出窗口（不可见）。
- 新增 `completion_scroll`（窗口首项下标）：selection 移动时保持
  `idx ∈ [scroll, scroll+MAX-1]`——越过上边界 `scroll = idx`，越过
  下边界 `scroll = idx - MAX + 1`；render_completions 渲染
  `completions[scroll..scroll+MAX]` 并按绝对下标匹配高亮。
- refresh_completions / clear_input / accept_completion 重置 scroll = 0；
  Up/Down/Tab 环绕语义不变。
- 涉及 ui/mod.rs（render_completions + App 字段）、ui/app_input.rs、
  ui/tests.rs。

**8. slash 弹层显示 skill:: 与 mcp: 条目（#47）**

- `collect_slash_commands` 追加两类条目（沿用 `/` 前缀存储与既有过滤
  机制）：每个已启用 skill → `skill::<name>`（WireSkillSnapshot.name
  原样）；每个 MCP 工具 → `mcp:<tool_name>`（sidebar.mcp.tool_names
  原样，server 定义名不改写）。
- 现有 `/skillname` 快捷与其它条目保留；提交后未知 slash 命令按 #37
  语义回落 user message（不报错）——这些是引用信息形态。
- 列表超长自动翻页由 #46 的 completion_scroll 承接（本节点在其后）。
- 涉及 ui/mod.rs（collect_slash_commands + 调用点）、ui/tests.rs。

**9. 工具名统一小写+下划线（#48，daemon-only，与主链并行）**

- 14 处改名（Tool.name + label()）：Skill → skill、SkillBuilder →
  skill_builder、InstallSkill → install_skill、RemoveSkill →
  remove_skill、SetSkillState → set_skill_state、NewCronJob →
  new_cron_job、ListCronJobs → list_cron_jobs、RemoveCronJob →
  remove_cron_job、SetCronJobState → set_cron_job_state、NewTrigger →
  new_trigger、ListTriggers → list_triggers、RemoveTrigger →
  remove_trigger、SetTriggerState → set_trigger_state、Exec → exec
  （exec_shell 仅 label，name 已是 exec）。
- 连带：turn/listener.rs 的 `tool_name == "Skill"` 匹配与 `Skill(...)`
  显示串、system_prompt.rs 自然语言提及、skill_builder description 内
  InstallSkill 自引用、daemon 测试（tests/tools/*、commands_e2e、
  dynamic_trigger_e2e、e2e_llm）名字断言、代码注释。
- 非目标：MCP 工具名（server 定义不改写）、Rust struct 名、历史会话
  transcript 旧名。

**10. reload 工具：LLM 一个 reload 入口 + TUI 联动（#50）**

- daemon 新 AgentTool `reload`（snake_case）：包装现有
  `reload_everything`（skills/config/commands/triggers 重扫），执行后
  递增 `runtime_revision`；description 写明"安装 skill / 修改配置后
  调用以生效"。与 /reload 命令共用逻辑（不复制）。
- transport：wire snapshot 加 `runtime_revision: u64`（serde default 0）。
- TUI 联动：App 缓存 revision，检测变化 → 重读 `~/.theway/theme.toml`
  + TUI 侧配置 → 更新 App.theme（TUI 自身 reload；runtime 侧 daemon 已
  完成）。LLM 只看到一个 reload。
- 涉及：daemon tools/reload.rs（新）+ assembly.rs 注册 + turn/daemon.rs、
  transport wire.rs、ui/mod.rs、ui/tests.rs。

**11. AGENTS.md 明确 daemon 定位（#51，文档，并行小节点）**

- daemon = 会话/工具/触发/编排的运行时服务，面向**协议层**（transport
  的 gRPC + HTTP/SSE/WS）；对客户端形态无概念，不区分 TUI/web/headless
  脚本/其他程序，不携带 UI 概念（颜色/布局/按键）。
- 边界规则：客户端专属外观与交互归 theway-tui；跨端新功能先定 wire
  契约，daemon 只做协议侧；需要客户端配合的行为用 snapshot 字段/事件
  表达（例：runtime_revision 通知客户端重读本地资源），不得假设单一
  客户端类型。
- 涉及 AGENTS.md（Workspace layout / Layering 附近）。

## Out of scope

- #39-#44 为 theway-tui / theway-markdown 纯展示（除 #50/#51 的 wire
  增量外，其余不碰协议/daemon）。
- dag band mini spinner、速率映射、textarea 内核。
- prompt chrome 配色 v1 由 #49 覆盖；多主题目录 + settings 选择 + 热
  切换不做（reload 后重读单文件即 v1 热重载语义）。

## Acceptance

- 12 条各自单测 + 编译 + 视觉断言（tmux e2e 截图）通过；
  make check / test / lint / fmt-check 全绿。
- 逐条 close #39-#44、#46-#51，证据贴 #45 后 close #45。
