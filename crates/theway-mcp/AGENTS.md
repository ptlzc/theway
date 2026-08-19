# theway-mcp 修改规则

本文件适用于 `crates/theway-mcp/`，并补充仓库级规则 [`../../AGENTS.md`](../../AGENTS.md)。修改请求关联或传输行为前，先阅读 [crate 概览](README.md)和[客户端架构](docs/architecture.md)。

## 边界规则

- 本 crate 与 `theway-core`、daemon 配置、工具策略、trigger 投递和 UI 代码保持独立。
- MCP 到 `AgentTool` 的转换和 MCP server 行为保留在 [`theway-daemon`](../theway-daemon/README.md)。
- 只为客户端已实现操作，或解码其响应和 notification 所必需的内容添加协议记录。

## 生命周期与安全规则

- 收到响应、超时、取消、transport 关闭或 future drop 时，都要移除对应 in-flight 请求。
- 保持响应关联与 server notification 投递相互独立。
- Stdio 与 HTTP 在 `Transport` trait 层表现一致。
- HTTP body、SSE buffer、空闲等待、重连延迟和取消发送都必须有界。
- Debug 输出、诊断和错误不得泄漏 bearer 凭证。

## 测试与文档

- 使用本地子进程或 HTTP fixture；测试不得连接外部 MCP server。
- 相关路径变化时覆盖分片 SSE、heartbeat、直接 JSON 响应、重连/取消、响应不匹配、notification 投递和子进程关闭。
- 握手、请求生命周期、协议子集或 transport 行为变化时，更新 [`docs/architecture.md`](docs/architecture.md)。
- 运行 `cargo test -p theway-mcp` 和 `cargo doc -p theway-mcp --no-deps --document-private-items`。
