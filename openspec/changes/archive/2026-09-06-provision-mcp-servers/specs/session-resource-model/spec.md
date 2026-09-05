## ADDED Requirements

### Requirement: Controller-provisioned MCP 服务器

controller 模式（TUI 带 `--storage-service-addr` spawn 的 daemon）下，MCP 服务器目录 SHALL 由 controller 通过设置契约供给（`WireDaemonConfig.mcp_servers`），daemon 不再读取本地 `mcp.toml`。独立 `thewayd`（无 controller）SHALL 保持本地扫盘加载语义。

#### Scenario: controller 供给 MCP 服务器

- **WHEN** TUI 在 controller 模式下扫描 `~/.theway/mcp.toml` 与项目 `.theway/mcp.toml` 并通过设置契约供给服务器列表
- **THEN** daemon 连接全部供给的服务器
- **AND** 连接成功的服务器工具进入会话工具集
- **AND** 服务器推送通知钩子注册进会话触发器执行器
- **AND** 连接失败的服务器以 `(name, error)` 形式进入会话快照 `McpSnapshot.errors`

#### Scenario: 设置契约替换 MCP 服务器集合

- **WHEN** controller 推送新的服务器列表（增删改任一）
- **THEN** daemon 断开不在新列表中的服务器并回收其工具与客户端
- **AND** 只连接新增/变更的服务器
- **AND** 新集合整体生效，无部分应用

#### Scenario: 清除 MCP 服务器供给

- **WHEN** controller 以 `clear_fields: ["mcp_servers"]` 清除供给
- **THEN** daemon 断开全部供给的 MCP 服务器
- **AND** 会话快照中 MCP 计数与服务器名归零

#### Scenario: 凭据不出设置契约

- **WHEN** 服务器配置含 bearer 认证
- **THEN** 设置契约 SHALL 只携带 `token_keychain_ref` 引用名
- **AND** token 内容由 daemon 从本地 `auth.json` 解析
- **AND** 配置视图回显 SHALL 不包含任何 token 明文

#### Scenario: 重载重连供给的 MCP 服务器

- **WHEN** controller 模式下触发 `/reload`
- **THEN** daemon 用供给的服务器配置重连 MCP（连接替换、快照刷新）
- **AND** 独立 thewayd 的 `/reload` 保持扫盘语义

### Requirement: MCP 失败在会话快照中可见

任何 MCP 服务器连接失败或配置文件解析失败 SHALL 以结构化 `(name, message)` 形式出现在会话快照的 `McpSnapshot.errors`，客户端据此展示启动提示与侧面板错误行。

#### Scenario: 供给的服务器连接失败

- **WHEN** 供给的某个服务器连接失败（网络超时、认证拒绝、spawn 失败）
- **THEN** `McpSnapshot.errors` 包含该服务器的 `name` 与失败原因
- **AND** 其余服务器不受影响

#### Scenario: mcp.toml 解析失败（本地扫描模式）

- **WHEN** 独立 thewayd 的 `mcp.toml` 无法解析
- **THEN** `McpSnapshot.errors` 包含文件标签与解析错误
- **AND** controller 模式下 TUI 扫描失败 SHALL 通过 provision notes 呈现
