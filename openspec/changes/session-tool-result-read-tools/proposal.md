# session-tool-result-read-tools

Issue: #50

Depends on: #49（session-tool-result-virtualization）

## Problem

长工具结果全量入库、上线时被占位符替换后（#49），模型需要**按需读取**
完整结果的手段 —— 否则只能重跑工具或靠猜。占位符是自描述的
（含 tail preview），模型据此决定是否读取、读哪一段。

## What changes

两个 session-tree 读取工具，协议为"先 grep 定位，再分页精读"：

1. **`session_tool_result(tool_call_id, offset?, max_lines?)`** —— 分页读取
   存储的工具结果（offset/limit 语义与 `read` 一致）：
   `{ tool_name, total_lines, chunk, has_more }`
2. **`session_tool_result_grep(tool_call_id, pattern)`** —— 在结果内定位
   （语义与 `grep` 一致）：
   `{ matches: [{line_no, text}], truncated }`

工具描述里写明建议流程：看 tail preview → grep 定位 → 分页读相关片段。

## Design notes

- 按 `tool_call_id` 查找（session tree 中 toolCall/toolResult 消息 id 配对；
  daemon 通过 `get_entry` / tool_result 索引解析）
- 单次读取上限与 `read` 一致（2000 行 / 256 KiB），服务端分块
- 只有超过阈值的结果才被虚拟化，这两个工具是低频路径 —— 描述保持精炼

## Out of scope

- TUI 浏览工具结果的界面（后续 UI issue）
- subagent/DAG job 的 tool result（在 job registry 而非 session tree）

## Acceptance

- `session_tool_result` 对存储结果返回分页 chunk + `has_more`
- `session_tool_result_grep` 返回行号 + 匹配行
- 未知 `tool_call_id` → 明确报错，不 panic
- `make test` + `make lint` 全绿
