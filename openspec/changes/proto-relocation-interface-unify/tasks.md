# Tasks: proto-relocation-interface-unify

DAG: `issue-70-p0-proto-relocation`。节点文件集不相交，串行为主：
1（搬迁+构建）→ 2（SDK/脚本/hooks）→ 3（JSON-RPC 方法对齐 + 文档）→ 4（验证）。

## 1. [1-move-proto] git mv + build.rs

- [x] 1.1 `git mv proto crates/theway-transport/proto`
- [x] 1.2 `crates/theway-transport/build.rs`：proto_dir 改为 `CARGO_MANIFEST_DIR/proto`
- [x] 1.3 `crates/theway-probe/build.rs`：proto_dir 指向 `../theway-transport/proto`
- [x] 1.4 验收：`cargo check -p theway-transport -p theway-probe --all-targets` 通过

## 2. [depends: 1-move-proto] [2-sdk-scripts] SDK 同步与 hooks

- [x] 2.1 `scripts/sdk-sync.sh`：拷贝源改为 `crates/theway-transport/proto/*.proto`
- [x] 2.2 `.githooks/pre-commit`：监听路径改为 `crates/theway-transport/proto sdk/proto`
- [x] 2.3 运行 `bash scripts/sdk-sync.sh`，确认 `sdk/proto/` 与 transport/proto 一致、
  `sdk/src/generated` 无额外 diff
- [x] 2.4 验收：sdk 生成幂等（重复执行无 diff）

## 3. [depends: 2-sdk-scripts] [3-jsonrpc-align] JSON-RPC 对齐 + 文档

- [x] 3.1 核对 `crates/theway-transport/src/http.rs` JSON-RPC 方法名与 proto service
  方法对齐；不一致处补别名/映射（不改 wire 字段）
- [x] 3.2 更新 `docs/architecture.md`、`sdk/README.md`、`README.md` 中 proto 路径引用
- [x] 3.3 删除根 `proto/` 残留引用（grep 确认无 `../../proto` / `proto/*.proto` 旧路径）
- [x] 3.4 验收：`cargo check --workspace --all-targets` 通过

## 4. [depends: 3-jsonrpc-align] [4-verify] 终态验收

- [x] 4.1 `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets -- -D warnings`、
  `cargo test --workspace`
- [x] 4.2 feature-gate：sandbox-only check + `sandbox_tool_gate`
- [x] 4.3 grep：无 `proto/` 根目录引用；SDK 同步脚本可运行且幂等
- [x] 4.4 三接口现有测试全绿（grpc/http/ws/mcp）
