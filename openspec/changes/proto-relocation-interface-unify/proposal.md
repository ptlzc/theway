# Proposal: proto-relocation-interface-unify

## Why

当前 proto 位于工作区根 `proto/`，由 transport 和 probe 两个 build.rs 引用，SDK 通过
`scripts/sdk-sync.sh` 镜像拷贝。三种接口（gRPC / JSON-RPC / MCP）各自实现，但共享的
契约模型（`WireCommand` / `WireStatus` / proto 类型）没有明确收敛到单一事实源。
在 daemon 无状态化（#70）之前，先把 proto 迁入 transport crate，并把三接口统一到
“一个契约层 + 三个薄适配器”。

## What Changes

- **proto relocation**：根 `proto/` 整体迁到 `crates/theway-transport/proto/`；
  更新 `crates/theway-transport/build.rs`、`crates/theway-probe/build.rs`、
  `scripts/sdk-sync.sh`、`.githooks/pre-commit`、相关文档。
- **interface unification**：
  - proto 作为唯一消息/服务契约源；
  - JSON-RPC 2.0 方法名与 proto service 方法对齐（HTTP `POST /rpc`、SSE `/events`、WS `/ws`）；
  - MCP server 复用同一 `AgentTool` 集合与 `TransportEndpoints`，不另造状态模型；
  - 三接口继续共享 `WireCommand` 事件循环与 `WireStatus`/proto 类型。
- 不改变现有 RPC 语义与 wire 字段（纯搬迁 + 适配层整理）。

## Capabilities

### New Capabilities

- `proto-relocation-interface-unify`: proto 单一事实源位于 transport crate，三接口共享同一契约层。

### Modified Capabilities

- 无（保持 gRPC/JSON-RPC/MCP 对外语义兼容）。

## Impact

- **move**：`proto/*.proto` → `crates/theway-transport/proto/*.proto`（git mv）。
- **build**：`crates/theway-transport/build.rs`、`crates/theway-probe/build.rs` 路径更新。
- **scripts**：`scripts/sdk-sync.sh`（拷贝源）、`.githooks/pre-commit`（监听路径）。
- **docs**：`docs/architecture.md`、`sdk/README.md` 等引用更新。
- **sdk**：`sdk/proto/` 仍为镜像，生成流程不变，只是源路径变化。
- **不变**：proto 包名/消息/服务/RPC 语义；`WireStatus`/`WireCommand` 形状；MCP 工具面。
EOF