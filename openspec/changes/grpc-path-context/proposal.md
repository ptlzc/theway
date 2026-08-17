# Proposal: grpc-path-context

## Why

#66 之后 daemon 的路径上下文收敛为 `DaemonPaths`（home / work_dir / base / extra_skill_dirs）
并在 CLI 边界一次解析。但 gRPC 协议层没有暴露这套上下文，也无法在运行中调整 skill 扫描根：
controller（web / headless / 未来客户端）只能通过 TUI 拉起 daemon 时传 `--home` / `--skills-dir`，
对已运行 daemon 的路径上下文既不可见也不可变。

## What Changes

- **proto**：新增 `PathContext`（home / base / work_dir / skills_dirs）与
  `SetSkillDirsRequest`；`SessionService` 增加 `GetPathContext` 与 `SetSkillDirs` RPC。
- **wire**：新增 `WirePathContext`（serde）；`WireCommand` 增加 `SetSkillDirs { dirs }`。
- **daemon**：`DaemonPaths.extra_skill_dirs` 改为 `Arc<RwLock<Vec<PathBuf>>>`（Clone 共享可变状态）；
  `skills::skills_dirs` 每次读取当前 extras；`TurnHost` 处理 `SetSkillDirs`：更新 extras →
  更新共享 path_context → 热重载 skill catalog（复用 `reload_skills_from_disk`）。
- **transport**：`TransportEndpoints` / `GrpcState` 增加共享 `path_context`；
  `GetPathContext` 返回当前上下文；`SetSkillDirs` 走事件循环应用；`GrpcClient` 增加两个方法。
- **语义**：`home` / `work_dir` / `base` 仍启动固定（不可改）；只有 `skills_dirs` 运行中可变。
  HTTP 不新增路由（WireCommand 已在共享层，后续可按需暴露）。

## Capabilities

### New Capabilities

- `grpc-path-context`: gRPC 暴露 daemon 路径上下文，并支持运行中动态更新 skill 扫描根。

### Modified Capabilities

- 无。`daemon-path-context` 的启动期固定语义保持不变，仅把 extras 变为可动态更新。

## Impact

- **proto**：`proto/session.proto`（消息 + 2 个 RPC）。
- **transport**：`wire.rs`（`WirePathContext`、`WireCommand::SetSkillDirs`）、
  `transport.rs`（`TransportEndpoints.path_context`）、`grpc.rs`（两个 RPC）、
  `client.rs`（两个方法）、`proto.rs`（转换）。
- **daemon**：`paths.rs`（extras 改 RwLock）、`skills.rs`（读当前 extras）、
  `turn/daemon.rs`（处理命令 + 热重载 + 共享 path_context）、`bin/thewayd.rs`（装配）。
- **不变**：`home` / `work_dir` / `base` 的启动固定语义；HTTP 路由；端口文件发现契约。
