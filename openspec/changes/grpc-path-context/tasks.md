# Tasks: grpc-path-context

DAG: `issue-68-grpc-path-context`。节点文件集不相交：
1（proto/wire/transport field/client）完成后，2（daemon paths/event loop）与
3（grpc server）可并行；4 依赖 2+3。

## 1. [1-proto-wire] proto + wire + client 方法

- [ ] 1.1 `proto/session.proto`：新增 `PathContext`、`SetSkillDirsRequest`；
  `SessionService` 增加 `GetPathContext` / `SetSkillDirs` RPC。
- [ ] 1.2 `crates/theway-transport/src/wire.rs`：新增 `WirePathContext`（serde，
  home/base/work_dir/skills_dirs: Vec<String>）；`WireCommand` 增加 `SetSkillDirs { dirs: Vec<String> }`。
- [ ] 1.3 `crates/theway-transport/src/proto.rs`：`WirePathContext` ↔ proto `PathContext`
  转换函数；round-trip 单测。
- [ ] 1.4 `crates/theway-transport/src/transport.rs`：`TransportEndpoints` 增加
  `path_context: Arc<RwLock<WirePathContext>>`。
- [ ] 1.5 `crates/theway-transport/src/client.rs`：`GrpcClient::get_path_context()` 与
  `set_skill_dirs(&[String])`。
- [ ] 1.6 验收：`cargo check -p theway-transport --all-targets`；proto 生成编译通过。

## 2. [depends: none] [2-daemon-paths] daemon 动态 extras + 事件循环

- [ ] 2.1 `crates/theway-daemon/src/paths.rs`：`extra_skill_dirs` 改为
  `Arc<RwLock<Vec<PathBuf>>>`；`from_cli` 包 Arc；新增 `set_extra_skill_dirs` /
  `current_extra_skill_dirs`；单测覆盖动态更新与 Clone 共享。
- [ ] 2.2 `crates/theway-daemon/src/skills.rs`：`skills_dirs` 每次读取当前 extras
  （`paths.current_extra_skill_dirs()`），不再直接读 Vec 字段。
- [ ] 2.3 `crates/theway-daemon/src/turn/daemon.rs`：`DaemonConfig` 增加 `paths: DaemonPaths`；
  `TurnHost` 保存 `paths` 与共享 `path_context: Arc<RwLock<WirePathContext>>`；
  `transport_endpoints()` 构建并注入 `TransportEndpoints.path_context`；
  `handle_web_command` 处理 `SetSkillDirs`：更新 extras →
  更新 path_context → abort 当前 turn → `reload_skills_from_disk()`。
- [ ] 2.4 `crates/theway-daemon/src/bin/thewayd.rs`：装配时传 `paths`。
- [ ] 2.5 验收：`cargo check -p theway-daemon --all-targets`。

## 3. [depends: 1-proto-wire, 2-daemon-paths] [3-grpc-server] gRPC server

- [ ] 3.1 `crates/theway-transport/src/grpc.rs`：`GrpcState` 增加 `path_context`；
  实现 `GetPathContext`（读锁 → proto）与 `SetSkillDirs`（乐观写共享 state +
  `commands.send(WireCommand::SetSkillDirs{dirs})` → `CommandResult`）。
- [ ] 3.2 `serve_grpc` 组装处传入 endpoints.path_context。
- [ ] 3.3 验收：`cargo check -p theway-transport --all-targets` + gRPC 集成测试
  （GetPathContext / SetSkillDirs 往返）。

## 4. [depends: 3-grpc-server] [4-tests-docs] 测试与文档

- [ ] 4.1 transport tests：`grpc` fixture 增加 PathContext round-trip；
  `client`/`grpc` 集成覆盖 GetPathContext 与 SetSkillDirs。
- [ ] 4.2 daemon 事件循环测试：SetSkillDirs 更新 extras 并触发 reload。
- [ ] 4.3 `docs/architecture.md` / README 简述 gRPC path-context 与动态 skills_dirs。
- [ ] 4.4 `openspec/changes/grpc-path-context/tasks.md` 勾选完成。
- [ ] 4.5 验收：`cargo test -p theway-transport --all-targets`、
  `cargo test -p theway-daemon --all-targets`。

## 5. [depends: 4-tests-docs] [5-verify] 终态验收

- [ ] 5.1 `cargo fmt --all -- --check`、`cargo check --workspace --all-targets`、
  `cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace`。
- [ ] 5.2 feature-gate：sandbox-only check + `sandbox_tool_gate`。
- [ ] 5.3 grep：新增 RPC 在 proto/client/server 三处齐全；WireCommand 变体被 daemon 处理。
