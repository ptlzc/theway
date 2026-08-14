# Tasks: tui-grok-prompt-chrome

Issue: #28. 单节点实现（改动集中在 theway-tui 两个文件 + 一个新模块），
每步小 commit（Conventional Commits，引用 #28）。

graph TD
  1-prompt-chrome --> 2-adopt-render
  2-adopt-render --> 3-verify-closeout

## 1-prompt-chrome [executor] — 新建 chrome 模块

文件: 新建 `crates/theway-tui/src/ui/prompt_chrome.rs`。

- [ ] 1.1 `PromptChrome` 结构（focused / model_name / flags / multiline / title 可选）
- [ ] 1.2 圆角边框绘制（╭─╮ 顶 / │ 侧 / ╰─╯ 底 + info line），tokyonight 色值
- [ ] 1.3 `❯` 前缀 + placeholder（空输入且 unfocused 时）
- [ ] 1.4 info line 渲染（左: model · flags；右: multiline），照 grok
      render_info_line 的布局语义
- [ ] 1.5 单元测试：边框字符 / 前缀 / info 行布局

## 2-adopt-render [executor] [depends: 1-prompt-chrome] — 接入 App::render

文件: `crates/theway-tui/src/ui/mod.rs`。

- [ ] 2.1 render() 输入框段改为调用 prompt_chrome，textarea 渲染进返回的内容区
- [ ] 2.2 移除 `> ` 段落渲染与直角 Block；focused = 无 picker/control-prompt
- [ ] 2.3 info line 填充：模型名（latest.model）+ flags（busy→working,
      queued_count>0→N queued）+ multiline（输入含 \n）

## 3-verify-closeout [checker] [depends: 2-adopt-render] — 验证

- [ ] 3.1 cargo fmt / clippy（theway-tui）/ test 全绿
- [ ] 3.2 手动跑 theway 看输入框外观（圆角 + ❯ + info line）
- [ ] 3.3 提交合并 + push main，close #28
