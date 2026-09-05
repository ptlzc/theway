## ADDED Requirements

### Requirement: 设置契约的 MCP 供给字段跨协议一致

设置契约的 `DaemonConfig` SHALL 在 gRPC 与 JSON-RPC 两个协议面携带相同的 MCP 服务器供给字段（`mcp_servers`，条目字段与 `mcp.toml` 的 `[[server]]` 表一一对应），两协议行为一致。

#### Scenario: gRPC 供给 MCP 服务器

- **WHEN** 外部客户端通过 gRPC `SettingsService.Configure` 推送 `mcp_servers`
- **THEN** daemon 连接行为与 JSON-RPC 供给完全一致
- **AND** `GetConfig` 配置视图回显已应用的 `mcp_servers`（含 `clear_fields` 语义）

#### Scenario: 供给字段的双向转换

- **WHEN** 任一协议面的 `DaemonConfig` 转换到另一协议面
- **THEN** `mcp_servers` 的每个条目字段（名称、类型、命令、参数、端点、认证引用、超时、重连、注入标志）保持一一对应
- **AND** 未设置字段以默认值表达，无数据丢失
