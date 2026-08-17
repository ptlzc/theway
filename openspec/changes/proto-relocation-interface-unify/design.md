# Design: proto-relocation-interface-unify

## D1. 目录与构建

- `git mv proto crates/theway-transport/proto`（保留历史）。
- `crates/theway-transport/build.rs`：`proto_dir = CARGO_MANIFEST_DIR.join("proto")`。
- `crates/theway-probe/build.rs`：同样改为 `CARGO_MANIFEST_DIR/../theway-transport/proto`
  或直接复用 transport 的生成产物（若 probe 可依赖 transport 的 proto 模块则删除自身 build.rs；
  否则最小改动只改路径）。先按最小改动处理：probe 保留独立 build.rs，路径指向
  `../theway-transport/proto`。
- `scripts/sdk-sync.sh`：`cp crates/theway-transport/proto/*.proto sdk/proto/`。
- `.githooks/pre-commit`：监听路径改为 `crates/theway-transport/proto sdk/proto`。
- 文档中所有 `proto/xxx.proto` 引用改为 `crates/theway-transport/proto/xxx.proto`
  （sdk 内相对引用除外）。

## D2. 三接口统一

统一不是合并协议，而是共享同一契约层：

1. **契约层**：`proto/*.proto` 定义消息与服务；`WireStatus`/`WireCommand` 是进程内共享模型；
   `transport::proto` 是二进制 gRPC 编解码；`transport::wire` 是 JSON/内部 serde 模型。
2. **gRPC**：直接由 proto 生成，tonic server。
3. **JSON-RPC**：HTTP `POST /rpc` + SSE `/events` + WS `/ws`。方法名与 proto service
   方法对齐（如 `session.list`、`session.switch`、`session.get_path_context`、
   `session.set_skill_dirs`）；载荷使用 `WireStatus`/`WireCommand` serde。
4. **MCP**：`--mcp` stdio server 暴露同一 `AgentTool` 集合；工具定义来自 daemon 组装，
   不另建状态模型；状态/事件仍走 `TransportEndpoints`。

## D3. 迁移检查

- `cargo check --workspace --all-targets` 通过。
- `scripts/sdk-sync.sh` 执行后 `sdk/proto/` 与 `crates/theway-transport/proto/` 一致，
  `sdk/src/generated` 重新生成无 diff（幂等）。
- `git status` 中旧 `proto/` 不再存在；`crates/theway-transport/proto/` 已跟踪。
- 三接口现有测试全绿（grpc/http/ws/mcp）。

## D4. 非目标

- 不改变 proto 包名、消息字段、RPC 签名。
- 不新增/删除任何 RPC（除后续 #70 P1+ 的配置类 RPC 另开 change）。
- 不做 daemon 配置剥离（P1）与本地文件操作外置（P2），本 change 只做搬迁与统一。
EOF