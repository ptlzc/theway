# Capability: proto-relocation-interface-unify

规范 proto 单一事实源位置与 gRPC / JSON-RPC / MCP 三接口的契约统一。

## ADDED Requirements

### Requirement: Proto lives in the transport crate

proto 文件 SHALL 位于 `crates/theway-transport/proto/`，作为 Rust 与 SDK 镜像的唯一事实源。
transport 与 probe 的 build.rs SHALL 从该目录编译；SDK 同步脚本 SHALL 从该目录拷贝到
`sdk/proto/`。

#### Scenario: Move source of truth

- **WHEN** 开发者修改 `crates/theway-transport/proto/session.proto`
- **THEN** transport/probe 构建重新生成，`scripts/sdk-sync.sh` 可同步到 `sdk/proto/`

#### Scenario: No stale root proto

- **WHEN** 检查仓库根目录
- **THEN** 不存在 `proto/` 目录（旧位置已迁移）

### Requirement: Three interfaces share one contract layer

gRPC、JSON-RPC 2.0、MCP 三种接口 SHALL 共享同一套 `WireCommand` / `WireStatus` /
proto 类型与 `TransportEndpoints` 事件循环。JSON-RPC 方法名 SHALL 与 proto service
方法对齐；MCP SHALL 复用同一 `AgentTool` 集合，不另造状态模型。

#### Scenario: JSON-RPC method mirrors proto

- **WHEN** proto 定义 `rpc SetSkillDirs(SetSkillDirsRequest) returns (CommandResult)`
- **THEN** JSON-RPC 提供对应方法（如 `session.set_skill_dirs`），载荷语义一致

#### Scenario: MCP reuses the tool surface

- **WHEN** daemon 以 `--mcp` 运行
- **THEN** MCP 暴露的工具来自同一 daemon `AgentTool` 集合，状态/事件走共享 transport

#### Scenario: No semantic break

- **WHEN** 执行现有 gRPC / HTTP / WS / MCP 测试
- **THEN** 全部通过，RPC 语义与 wire 字段不变
