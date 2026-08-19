# theway-mcp 架构

[English](architecture.md) | 中文

## 范围

`theway-mcp` 实现 theway 所需的 Model Context Protocol 客户端部分：通过 stdio 或 Streamable HTTP 完成 initialize、工具发现与调用、请求取消和 server notification。Sampling、resource subscription 和 MCP server 行为不属于本 crate。

公开 API 不出现运行时引擎类型。`theway-daemon` 把 `McpTool` 定义包装成面向模型的工具，并决定 notification 如何进入 trigger 处理。

## 协议记录

[`protocol.rs`](../src/protocol.rs) 定义协商协议版本、初始化记录、工具定义与结果、支持的内容变体、JSON-RPC 请求/错误/notification 和请求 builder。未知或不支持的 server 载荷通过 [`McpError`](../src/errors.rs) 失败，不在这里转换成 agent 运行时错误。

## 客户端生命周期

[`client.rs`](../src/client.rs) 持有一个 `Arc<dyn Transport>`、单调递增请求标识、in-flight 响应表、初始化状态、缓存工具和 server notification channel。

客户端读取循环把每行 JSON 分类为响应或 notification。响应完成匹配的 in-flight 请求；notification 转发给 `take_notifications` 获取的 receiver。In-flight guard 被 drop 时移除 pending 表条目，因此超时或取消调用不会泄漏关联状态。

`initialize` 记录 server info 与能力，然后发送 initialized notification。`tools_list` 刷新目录。`tools_call` 支持可选取消 token；取消会在有界发送预算内发送 `notifications/cancelled`，并以对应错误结束本地调用。

`close` 关闭 transport 并终止 pending 活动。请求超时属于客户端配置，并在各 transport 实现间保持一致。

## Transport 实现

[`transport.rs`](../src/transport.rs) 定义面向换行帧的 `send_line`、`recv_line` 和 `close`。Framing 位于 `McpClient` 下方，JSON-RPC 关联位于其上方。

[`stdio.rs`](../src/stdio.rs) 启动 stdin/stdout 管道子进程，每行写入一个 JSON 文档、逐行读取 stdout，并在 close 时终止子进程。

[`http.rs`](../src/http.rs) 实现 Streamable HTTP。出站 JSON-RPC 消息 POST 到配置端点；响应 body 可以是直接 JSON 或 SSE 流。SSE parser 忽略 heartbeat，并把 data 字段作为 JSON 行转发。Body 上限、请求超时、SSE 空闲超时、重连退避、last-event-id 处理和取消共同限制网络资源。`HttpMcpAuth` 的 `Debug` 实现会遮蔽 bearer token。

## 不变量

- 一个请求标识只对应一个 in-flight 响应，并在完成、超时、取消或 drop 时移除。
- Server notification 不会完成 pending 请求，响应也不会进入 notification 流。
- 两种 transport 向 `McpClient` 提供相同的换行分隔 JSON 行为。
- HTTP 响应和 SSE buffer 保持有界，空闲流可以取消。
- Debug 格式和错误消息不输出认证值。
- Agent 工具适配、notification 投递策略和 MCP server 模式由 daemon 负责。
