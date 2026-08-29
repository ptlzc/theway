# Tasks: session-tool-result-read-tools

Issue: #50. 依赖 #49 的全量入库。按需读取工具（分页 + grep 定位）。
每步小 commit（Conventional Commits，引用 #50）。

graph TD
  1-session-tool-result --> 2-session-tool-result-grep
  2-session-tool-result-grep --> 3-verify

## 1-session-tool-result [executor] — 分页读取工具

- [ ] 1.1 daemon tools 新增 `session_tool_result`：按 tool_call_id 从
      session tree 解析 toolResult（get_entry / tool_result 索引），
      offset/max_lines 分页（上限 2000 行 / 256 KiB，同 read），
      返回 { tool_name, total_lines, chunk, has_more }
- [ ] 1.2 未知 tool_call_id → 明确错误，不 panic
- [ ] 1.3 单测：分页边界、has_more、UTF-8 安全、未知 id
- [ ] 1.4 验收：`cargo test -p theway-daemon` + `make lint` 绿

## 2-session-tool-result-grep [executor] [depends: 1-session-tool-result] — grep 定位工具

- [ ] 2.1 新增 `session_tool_result_grep`：复用 1 的解析 helper，
      正则匹配返回 { matches: [{line_no, text}], truncated }，
      单行 500 字符上限（同 grep）
- [ ] 2.2 工具描述写明协议：tail preview → grep 定位 → 分页精读
- [ ] 2.3 单测：命中/未命中、行号、长行截断、未知 id
- [ ] 2.4 验收：`cargo test -p theway-daemon` + `make lint` 绿

## 3-verify [checker] [depends: 2-session-tool-result-grep] — 验证

- [ ] 3.1 `make test` + `make lint` + `make fmt-check` 全绿
- [ ] 3.2 e2e：虚拟化占位符场景下模型 grep → 分页读取完整结果
      （#49 验收场景衔接）
- [ ] 3.3 push + close #50
