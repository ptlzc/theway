# Tasks: grpc-service-domain-split

> 编排原则见 `openspec/config.yaml`。节点 ID = `<阶段数字>-<kebab-case 语义名>`；
> 每个非根节点必须声明 `[depends: ...]`；并行节点文件不相交。
>
> 并行组：P1 `2-server-dual ∥ 3-client-four-stubs ∥ 4-probe-four-stubs ∥ 5-sdk-four-stubs`。
> `6-tests-dual` 在 `2-server-dual` 完成后即可启动；`7-cutover` 等待全部迁移节点。
> 每个节点独立提交（Conventional Commits，引用 #60）。

## Phase 1 — proto 桥：领域 service 与旧 service 并存

 - [ ] **`1-proto-dual`** · agent: executor · [depends: -]
      `proto/commands.proto` 末尾新增 `service CommandService`（SendMessage / SetModel /
      Cancel / Approve）。`proto/session.proto` 新增 `import "commands.proto";` 与
      `service SessionService`（GetState / ListSessions / CreateSession / SwitchSession /
      RenameSession / DeleteSession）。`proto/graph_engine.proto` 新增
      `import "commands.proto";` 与 `service GraphEngineService`（GetNodeOutput /
      GraphCancel / GraphRetry / GraphSkip / GraphNodeInterrupt / GraphNodeSteer /
      GraphCheckpoint / GraphRestore / GraphList）。`proto/events.proto` 新增
      `import "commands.proto";` 与 `service EventService`（StreamEvents）。
      保留 `proto/theway_grpc.proto` 旧 `service ThewayGrpc` 作为迁移桥；四个领域文件的
      message 字段一律不动。`sdk/proto/` 与根 `proto/` 同步镜像。运行
      `cd sdk && npm run gen` 重新生成 `sdk/src/generated/*`（旧 `theway_grpc.ts` 仍保留），
      并提交生成文件。
      只改: `proto/{commands,session,graph_engine,events}.proto`，对应 `sdk/proto/*`，
      `sdk/src/generated/{commands,session,graph_engine,events}.ts`。
      验收: `cargo check --workspace --all-targets`；`cd sdk && npm run gen && npm run build`；
      生成文件出现 `CommandService` / `SessionService` / `GraphEngineService` / `EventService`
      四个 client/server 符号，旧 `ThewayGrpc` 符号仍在。

## Phase 2 — 并行迁移：server ∥ transport client ∥ probe ∥ SDK

 - [ ] **`2-server-dual`** · agent: executor · [depends: 1-proto-dual]
      `crates/theway-transport/src/grpc.rs` 用全路径为 `GrpcState` 实现四个新 tonic trait：
      `theway_grpc::command_service_server::CommandService`、
      `theway_grpc::session_service_server::SessionService`、
      `theway_grpc::graph_engine_service_server::GraphEngineService`、
      `theway_grpc::event_service_server::EventService`。新 trait 的方法体从旧
      `impl ThewayGrpc` 搬入；旧 impl 保留为薄委托（UFCS 显式指定新 trait，例如
      `<Self as theway_grpc::session_service_server::SessionService>::get_state(self, request)`）。
      桥接期不要把新 trait
      `use` 进模块作用域，避免 `state.send_message(...)` 测试调用出现同名方法歧义。
      `serve_grpc` 在保留 `ThewayGrpcServer` 的同时追加注册四个新 server
      （`GrpcState` 已 `Clone`，每个 `.add_service` 传 `state.clone()`）。
      只改: `crates/theway-transport/src/grpc.rs`。
      验收: `cargo check -p theway-transport --all-targets`；
      `cargo test -p theway-transport grpc` 通过（旧 trait 测试路径仍工作）。

 - [ ] **`3-client-four-stubs`** · agent: executor · [depends: 1-proto-dual]
      `crates/theway-transport/src/client.rs`：`GrpcClient` 改为持有四个生成客户端
      `SessionServiceClient` / `CommandServiceClient` / `GraphEngineServiceClient` /
      `EventServiceClient`。`connect` 先 `Channel::from_shared(format!("http://{addr}"))`
      建一次 channel，再 `clone()` 给四个 `*Client::new(...)`。各方法按领域调用对应 stub，
      方法签名、返回类型与错误文案不变。删除 `ThewayGrpcClient` 依赖。
      只改: `crates/theway-transport/src/client.rs`。
      验收: `cargo check -p theway-transport --all-targets`（server/client 桥接期并行，
      行为测试由 `6-tests-dual` 与终态验收覆盖）。

 - [ ] **`4-probe-four-stubs`** · agent: executor · [depends: 1-proto-dual]
      `crates/theway-probe/src/main.rs`：直接生成客户端改接新服务——
      `run_multi_session` 使用 `SessionServiceClient`（list/create/switch）与
      `CommandServiceClient`（send_message）；`run_get_state` 使用
      `SessionServiceClient`。更新相关错误文案
      （`ThewayGrpc::GetState` → `SessionService::GetState`）。
      只改: `crates/theway-probe/src/main.rs`。
      验收: `cargo check -p theway-probe`；`cargo build -p theway-probe` 通过。

 - [ ] **`5-sdk-four-stubs`** · agent: executor · [depends: 1-proto-dual]
      `sdk/src/client.ts`：`ThewayGrpcClient` 改为持有四个生成 stub
      （`SessionServiceClient` / `CommandServiceClient` / `GraphEngineServiceClient` /
      `EventServiceClient`），构造器逐一创建，方法按领域委托，`close()` 关闭四个。
      `sdk/src/index.ts` 确认 star export 覆盖四个领域生成模块；shared helper 仍从
      `./generated/commands.js` pin。旧 `./generated/theway_grpc.js` 的 star export
      暂保留（cutover 删除）。
      只改: `sdk/src/client.ts`, `sdk/src/index.ts`。
      验收: `cd sdk && npm run build`；用 `node --input-type=module` 导入
      `./dist/index.js`，断言 `ThewayGrpcClient` / `HealthClient` / `MessageMode` /
      `SessionState` / `protobufPackage` 仍可访问。

## Phase 3 — transport 生成客户端测试切换

 - [ ] **`6-tests-dual`** · agent: executor · [depends: 2-server-dual, 3-client-four-stubs]
      `crates/theway-transport/tests/grpc/mod.rs`：
      `grpc_server_over_transport_serves_client` 改为分别连接
      `SessionServiceClient`（GetState）与 `CommandServiceClient`（SendMessage），并新增断言
      `EventServiceClient::stream_events` 可建流、`GraphEngineServiceClient::graph_list`
      可调用；不依赖旧 `ThewayGrpcClient`。其余 `GrpcState` 直调测试保持旧 trait 路径
      （cutover 节点再统一切换 import）。
      只改: `crates/theway-transport/tests/grpc/mod.rs`。
      验收: `cargo test -p theway-transport grpc` 通过。

## Phase 4 — cutover：删除旧 service

 - [ ] **`7-cutover`** · agent: executor · [depends: 2-server-dual, 3-client-four-stubs, 4-probe-four-stubs, 5-sdk-four-stubs, 6-tests-dual]
      删除 `proto/theway_grpc.proto` 与 `sdk/proto/theway_grpc.proto`。
      `crates/theway-transport/build.rs` / `crates/theway-probe/build.rs` 改为编译
      `commands.proto` / `session.proto` / `graph_engine.proto` / `events.proto` +
      `health.proto`（可读目录收集，保留 rerun-if-changed 覆盖）。
      `crates/theway-transport/src/grpc.rs`：删除旧 `impl ThewayGrpc` 委托与
      `ThewayGrpcServer` 注册；把四个新 service trait `use` 进模块作用域；注册四个新
      server + `HealthServer`；启动日志改为四个 service 全路径。
      `crates/theway-transport/tests/grpc/mod.rs`：直调测试依赖新 trait 作用域，
      编译即验证无旧 trait 引用。
      `sdk/scripts/generate.sh` 输入改为四个领域 proto + `health.proto`；运行
      `npm run gen` 删除旧 `sdk/src/generated/theway_grpc.ts`。
      `sdk/src/index.ts` 删除 `./generated/theway_grpc.js` star export；更新
      `sdk/src/client.ts` JSDoc、`sdk/README.md`、`sdk/package.json` description、
      `crates/theway-transport/src/proto.rs` 与 `crates/theway-probe/src/main.rs` 的
      proto 引用说明。
      只改: `proto/theway_grpc.proto`（删）、`sdk/proto/theway_grpc.proto`（删）、
      `crates/theway-transport/{build.rs,src/grpc.rs,src/proto.rs,tests/grpc/mod.rs}`、
      `crates/theway-probe/{build.rs,src/main.rs}`、
      `sdk/{scripts/generate.sh,src/client.ts,src/index.ts,src/generated/*,README.md,package.json}`。
      验收: `grep -R "theway_grpc.proto\|ThewayGrpcServer\|theway_grpc_server" crates sdk proto`
      仅剩允许的历史注释/本地 Rust 模块路径（`crate::proto::theway_grpc` 与探针模块名）；
      `cargo check --workspace --all-targets`；`cd sdk && npm run gen && npm run build`；
      `cargo test -p theway-transport grpc`。

## Phase 5 — 终态验收

 - [ ] **`8-verify`** · agent: verify · [depends: 7-cutover]
      基于最新 HEAD 全量复核：`cargo test --workspace`、
      `cargo clippy --workspace --all-targets -- -D warnings`、
      `cargo fmt --all --check`；`cd sdk && npm run gen && npm run build`；
      冒烟 `theway --grpc` 后运行 probe（health-check / health-watch / multi-session /
      get-state），并用 transport 测试确认四个 service（GetState / SendMessage /
      StreamEvents / GraphList）均可访问。
      验收: 全绿；新增失败为 0；冒烟输出 service 名称为四个领域 service。

## Phase 6 — 收尾

 - [ ] **`9-docs-closeout`** · agent: writer · [depends: 8-verify]
      README / SDK README / docs 中 gRPC service 描述同步为四 service 结构；确认
      `openspec` 本 change 与实现一致；close issue #60。
      只改: `README.md`, `sdk/README.md`, `docs/*`（如需）。
      验收: `grep -R "theway.grpc.v1.ThewayGrpc" README.md sdk docs` 无残留
      （历史日志产物除外）；issue #60 closed。
