# Tasks: consolidate-server-cli

## 1. server 回迁 (N1)

- [ ] 1.1 移动文件: `crates/server/src/{grpc,http,ws,proto}.rs` + `crates/server/src/http/tests/` → `crates/app/src/transport/`;`crates/server/build.rs` → `crates/app/build.rs`。
- [ ] 1.2 `crates/app/Cargo.toml`: 加 `server = ["dep:axum", "dep:tonic", "dep:tonic-prost", "dep:prost", "dep:tokio-stream"]` feature;dependencies 加 axum 0.8 (ws feature) / tonic 0.14 (server/channel/codegen) / tonic-prost 0.14 / prost 0.14 / tokio-stream (net/sync) (从 crates/server/Cargo.toml 复制版本);[build-dependencies] 加 protox 0.9 / tonic-prost-build 0.14。
- [ ] 1.3 `crates/app/src/lib.rs`: 加 `#[cfg(feature = "server")] pub mod transport;`。
- [ ] 1.4 引用适配 (transport 模块内): `theway::wire::` → `crate::wire::`;`theway::ui::` → `crate::ui::`;`theway::session_ops::` → `crate::session_ops::`;`theway::commands` / `theway::mentions` / `theway::readline` 等 → `crate::`;`theway_core::` 保持;`theway::transport::` → `crate::transport::`。
- [ ] 1.5 删除 `crates/server` 目录;根 `Cargo.toml` members 移除 `crates/server`。
- [ ] 1.6 验收: `cargo build --workspace --features tui,server` 通过;`cargo test -p theway --features server --lib transport::` 通过 (transport 测试随迁)。

## 2. cli 回迁 (N2)

- [ ] 2.1 移动: `crates/cli/src/main.rs` → `crates/app/src/main.rs`;`crates/cli/tests/cli_help.rs` → `crates/app/tests/`。
- [ ] 2.2 `crates/app/Cargo.toml`: 加 `[[bin]] name = "theway" path = "src/main.rs" required-features = ["tui", "server"]`;dependencies 补 main.rs 需要而 app 缺的 (clap 4 derive, tracing-subscriber 已有?按 use 反推,缺什么编译报错补什么)。
- [ ] 2.3 main.rs 引用适配: `theway_server::` → `theway::transport::` (仅 server 相关);其余 `theway::xxx` 保持 (bin 通过包名引用 lib)。
- [ ] 2.4 删除 `crates/cli` 目录;根 `Cargo.toml` members 移除 `crates/cli`。
- [ ] 2.5 验收: `cargo build --workspace --features tui,server` (产出 target/debug/theway.exe);`cargo test -p theway --features tui,server --test cli_help` 通过。

## 3. 收尾 (N3)

- [ ] 3.1 `Makefile`: `--features tui` → `--features tui,server` (build/check/test 目标)。
- [ ] 3.2 `.github/workflows/ci.yml`: 同样 feature 更新。
- [ ] 3.3 文档: `docs/issues/00-master.md` 分层描述 (theway-server/theway-cli 引用) 更新为 feature 结构;AGENTS.md 的 crate 结构描述更新 (如提及)。
- [ ] 3.4 复核: `grep -rn "theway_server\|crates/server\|crates/cli" Cargo.toml crates/ Makefile .github/ docs/` 结果为空 (openspec/ 除外)。
- [ ] 3.5 验收: `cargo build --workspace` (headless, 无 bin) 与 `cargo build --workspace --features tui,server` 均通过。

## 4. 验证 (N4)

- [ ] 4.1 `cargo test --workspace --features tui,server --no-fail-fast`: 新增 0 失败 (既有 8 个 Windows 环境失败除外)。
- [ ] 4.2 `cargo clippy --workspace --all-targets --features tui,server -- -D warnings` + `cargo fmt --all --check`。
- [ ] 4.3 冒烟: `--web` 起服 → `/healthz` 200、`/` 404、`/sessions` 列表、`POST /sessions` 创建+切换;`--grpc` 起服 → 引导日志含 health。
- [ ] 4.4 提交全部改动并推送。
