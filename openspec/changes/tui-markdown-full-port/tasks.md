# Tasks: tui-markdown-full-port

Issue: #25. DAG 编排（openspec/config.yaml 规范）：节点按文件归属串/并，
每个节点完成即小步 commit（Conventional Commits，引用 #25）。
agent 映射：executor = executor-coder，verify = checker，writer = executor-writer。

graph TD
  0-preflight --> 1-port-markdown
  1-port-markdown --> 2-adopt-feed
  2-adopt-feed --> 3-verify
  3-verify --> 4-docs-closeout

## 0-preflight [executor] — 依赖预检

文件: 无改动（只读检查 + 报告写入节点输出）。

- [ ] 0.1 确认捐赠源存在且 revision 匹配：`/root/workspace/grok-build/crates/codegen/xai-grok-markdown/`
      （git rev-parse 得到 revision，记录进 NOTICE 文案）
- [ ] 0.2 依赖预检：syntect 5.3 / two-face 0.4 (default-features=false,
      syntect-fancy) / anstyle-syntect 1.0.4 / anstyle-lossy 1.1.4 /
      html-escape 0.2 / supports-color 3.0 的 MSRV ≤ 1.85 且 edition 2024
      可编译；在临时目录 `cargo add` 这些依赖并 `cargo check` 验证解析
      （不得改主工作树文件）
- [ ] 0.3 检查捐赠 crate 对 workspace 的依赖：`grep -n "xai_grok_markdown_core"
      src/`（仅 parse.rs 一处）; 确认 bin/ 与 playground feature 不参与 lib 编译
- [ ] 0.4 报告结论（依赖版本 / MSRV / 风险）; 若预检失败本节点 FAIL 并回报
      （fallback: 停在此处, 人工决策 pin 旧版本或裁剪模块）

## 1-port-markdown [executor] [depends: 0-preflight] — 机械移植

文件: 新建 `crates/theway-markdown/*`（src 全量 + assets/grok-night.tmTheme +
NOTICE + Cargo.toml）, 根 `Cargo.toml`（members 追加）。

- [ ] 1.1 复制 `xai-grok-markdown/src/` → `crates/theway-markdown/src/`；
      前缀替换 `xai_grok_markdown` → `theway_markdown`、
      `xai-grok-markdown-core` → `theway-markdown-core`（crate 引用与
      Cargo 依赖名）; 不搬 `bin/`、不建 `playground` feature、不搬
      `benches/`（criterion 依赖不引入）
- [ ] 1.2 复制 `xai-grok-pager-render/assets/grok-night.tmTheme` →
      `crates/theway-markdown/assets/grok-night.tmTheme`；写 NOTICE
      （捐赠方 SpaceXAI, Apache-2.0, 源 revision `<0-preflight 记录>`,
      ratatui/syntect/two-face/pulldown-cmark 上游出处, tmTheme 资产出处）
- [ ] 1.3 Cargo.toml：license Apache-2.0, edition/rust-version 走 workspace
      继承; 依赖版本对齐 grok-build workspace（见 proposal Impact）;
      `lints.workspace = true`
- [ ] 1.4 验收：`cargo test -p theway-markdown` 全绿;
      `cargo clippy -p theway-markdown --all-targets -- -D warnings` 全绿;
      grep 无 `xai-`/`xai_grok` 残留（NOTICE 中提及捐赠方除外）
- [ ] 1.5 commit `feat(#25): port theway-markdown from Grok Build`

## 2-adopt-feed [executor] [depends: 1-port-markdown] — TUI 接入

文件: `crates/theway-tui/Cargo.toml`, `crates/theway-tui/src/feed_render.rs`,
`crates/theway-tui/src/ui/tests.rs`。

- [ ] 2.1 TUI 依赖 `theway-markdown = { path = "../theway-markdown" }`;
      主题构造（`Syntect::new(include_bytes!(...grok-night.tmTheme))`）
      放 theway-tui（once_cell 惰性单例），MarkdownStyle 用
      `Default::default().adapt()`
- [ ] 2.2 `feed_render.rs`: assistant 块改走
      `render_markdown_ratatui_full`（pretty=true）; `ai ▸` 前缀加到首行;
      非代码行用现有 `wrap_str` 按宽度折行，代码块行（`code_blocks`
      的 `output_line_range`）与表格行保持原样; 链接下划线改用返回的
      `hyperlinks`（line_index/column_range）替代现有 regex 扫描路径
      （其它块保留原扫描）; 删除 `push_markdown_paragraphs` 与
      theway-markdown-core 直连（如已无引用则移除依赖）
- [ ] 2.3 更新 `ui/tests.rs`：翻转 `markdown_single_tilde_pair_stays_literal`
      为对齐 Grok 口径（单波浪线删除线行为以渲染器实际输出为准）；新增
      parity 断言：**bold**→BOLD、*em*→ITALIC、`# h`→标题样式、`code`
      行内、表格边框行、`[text](url)` 下划线、围栏代码语法高亮 span 非空
- [ ] 2.4 验收：`cargo test -p theway-tui` 全绿; 人工渲染样例（含
      mermaid 围栏与 `$x^2$` latex）输出与 Grok 视觉对齐
- [ ] 2.5 commit `feat(#25): feed renders markdown via theway-markdown`

## 3-verify [checker] [depends: 2-adopt-feed]

- [ ] 3.1 `make check` / `make test` / `make lint` / `make fmt-check` 全绿
- [ ] 3.2 抽查：无 `xai-` 残留; NOTICE/license 头; Cargo.lock 无多余依赖;
      feed 边界（空文本 / 仅代码块 / 超宽代码行不折行 / 前缀 + 标题首行）
- [ ] 3.3 commit 仅在有修正时产生 (`fix(#25): ...`)

## 4-docs-closeout [writer] [depends: 3-verify]

文件: `README.md`, `openspec/changes/tui-markdown-full-port/`（归档标记）,
issue #25 关闭。

- [ ] 4.1 README 增补一行 markdown 渲染说明（assistant 输出经
      theway-markdown 渲染：样式/表格/语法高亮/mermaid/latex）;
      AGENTS.md 如需要则补例外说明（大文件 diff-compat 例外沿用
      mermaid-parser 先例，无需新增条目）
- [ ] 4.2 `gh issue close 25`; 变更文档标注完成
- [ ] 4.3 commit `docs(#25): markdown rendering in README + close #25`
