# Tasks: session-tool-result-virtualization

Issue: #49. 翻转工具结果管线：工具全量返回 → 全量入库 → 上下文构建时
确定性占位符替换。每步小 commit（Conventional Commits，引用 #49）。

graph TD
  1-tools-full-output --> 2-context-virtualize
  2-context-virtualize --> 3-verify

## 1-tools-full-output [executor] — 工具层翻转

- [ ] 1.1 bash.rs：去掉 truncate_tail 截断（保留 10 MB 保险上限），
      完整 stdout/stderr 返回并入库；`[truncated: ...]` 注记仅用于
      >10 MB 保险上限场景
- [ ] 1.2 read.rs：limit 语义保留（默认 2000 行）但去掉 256 KiB
      字节截断 —— 调用方明确指定 limit 时按行数返回全量
- [ ] 1.3 git.rs / grep.rs：同样去掉字节硬截断，保留结果数/行数上限
      （grep 100 条结果不变，这是结果集上限不是字节截断）
- [ ] 1.4 回归：现有 truncate 单测适配（truncate.rs 保留给保险上限）；
      TUI feed 展示层（compact_tool_output_lines）不动，确认 display-only
- [ ] 1.5 验收：`cargo test -p theway-daemon tools` + `make check` 绿；
      sqlite 直查确认 > 256 KiB 的 bash 输出完整入库

## 2-context-virtualize [executor] [depends: 1-tools-full-output] — 上下文虚拟化

- [ ] 2.1 core 新增确定性占位符替换：阈值 4 KiB，自描述格式
      `[tool_result <tool> <call_id>: <size> / <lines>, exit <code>; tail: …]`
      （tail preview 5 行、单行 200 字符），保留 toolResult 角色 +
      tool_call_id（API toolCall/toolResult 配对要求）
- [ ] 2.2 挂到 build_context / transform_context（daemon 侧传入钩子，
      run_loop/llm.rs 已预留）；小结果内联不变
- [ ] 2.3 单测：大小/行数/tail 边界、UTF-8 安全截断、
      call_id 配对保持、两轮历史不变时上下文前缀字节级一致
- [ ] 2.4 验收：`cargo test -p theway-core` + `make lint` 绿

## 3-verify [checker] [depends: 2-context-virtualize] — 验证

- [ ] 3.1 `make test` + `make lint` + `make fmt-check` 全绿
- [ ] 3.2 e2e：> 4 KiB 输出一轮对话 → sqlite 全量在库、
      上下文占位符、模型可基于 tail preview 继续；小结果内联
- [ ] 3.3 push + close #49
