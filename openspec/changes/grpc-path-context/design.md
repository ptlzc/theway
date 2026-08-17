# Design: grpc-path-context

## D1. 共享可变 path_context

`DaemonPaths.extra_skill_dirs` 从 `Vec<PathBuf>` 改为 `Arc<RwLock<Vec<PathBuf>>>`：
`DaemonPaths` 仍可 Clone，Clone 共享同一份 extras；`skills::skills_dirs` 每次调用读取当前值，
因此热重载/事件循环修改后立即生效。`home` / `work_dir` / `base` 保持不可变字段。

`WirePathContext { home, base, work_dir, skills_dirs }` 由 `TurnHost` 在
`transport_endpoints()` 构建一次，放进 `Arc<RwLock<WirePathContext>>`，同时挂在
`TransportEndpoints.path_context` 与 `TurnHost.path_context`。`GrpcState` 从 endpoints 持有
同一 Arc，`GetPathContext` 读它；`SetSkillDirs` RPC 先乐观更新该 Arc（与 SwitchSession 的
session_id 一致），再发 `WireCommand::SetSkillDirs` 到事件循环做权威应用 + 热重载。

## D2. 事件循环权威应用

`TurnHost::handle_web_command` 新增 `WireCommand::SetSkillDirs { dirs }`：

1. `dirs: Vec<String>` → `Vec<PathBuf>`；
2. 写 `self.paths.extra_skill_dirs`（共享 RwLock）；
3. 写 `self.path_context` 的 `skills_dirs`；
4. 若当前有 turn 在跑，先 `request_abort`（与 SwitchSession 一致，避免热重载与工具调用竞态）；
5. 调用 `self.kernel.harness().reload_skills_from_disk().await` 应用新 skill 目录；
   成功输出 loaded/diagnostics 摘要，失败走 error_line。

`reload_skills_from_disk` 闭包捕获的是同一个 `DaemonPaths` Clone（Arc 共享 extras），
所以会扫描到新目录。

## D3. gRPC 面

`proto/session.proto`：

```proto
message PathContext {
  string home = 1;
  string base = 2;
  string work_dir = 3;
  repeated string skills_dirs = 4;
}
message SetSkillDirsRequest { repeated string dirs = 1; }
service SessionService {
  rpc GetPathContext(Empty) returns (PathContext);
  rpc SetSkillDirs(SetSkillDirsRequest) returns (CommandResult);
}
```

`GrpcState` 实现：

- `GetPathContext`：读 `path_context` → `proto::PathContext`（复用 `proto.rs` 的 wire↔proto 转换）。
- `SetSkillDirs`：解析 dirs → 乐观写 `path_context.skills_dirs` → `commands.send(WireCommand::SetSkillDirs{dirs})` → 返回 `CommandResult{accepted}`；channel 关闭返回 unavailable。

`GrpcClient` 新增 `get_path_context() -> WirePathContext` 与 `set_skill_dirs(&[String]) -> bool`。

## D4. 兼容与边界

- `WireStatus` / `SessionState` **不加** path_context 字段：避免所有 fixture 改动；
  路径上下文走独立 `GetPathContext` RPC，职责单一。
- `home` / `work_dir` / `base` 不提供 setter；`SetSkillDirs` 只接受 `skills_dirs`。
- HTTP 不加路由；`WireCommand::SetSkillDirs` 在共享层存在，后续可按需暴露到 HTTP。
- `skills::skills_dirs` 的优先级语义不变，extras 仍最高且先扫描者胜。

## D5. 测试

- `paths.rs`：extras 可动态更新（write 后 skills_dirs 读出新值；Clone 共享同一份）。
- `skills.rs` / 现有 `cli_skills`：extras 动态变化后 `load_all` 使用新值。
- transport `proto`：`WirePathContext` ↔ proto round-trip。
- transport `grpc` 集成：`GetPathContext` 返回启动上下文；`SetSkillDirs` 后
  `GetPathContext` 反映新 dirs（走事件循环或乐观更新断言）。
- daemon 事件循环：`SetSkillDirs` 更新 extras 并触发 reload（可注入假 reload 或断言 harness
  reload 被调用）。
- 终态：workspace fmt/check/clippy/test + feature-gate 全绿。
