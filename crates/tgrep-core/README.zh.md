# tgrep-core（vendored）

[English](README.md) | 中文

[`tgrep-core`](https://github.com/microsoft/tgrep) 的 vendored 副本 —— `tgrep` CLI 背后的三字符组索引库。**逐字**复制自上游 commit `e2007b52d2b8fe4176159d0da20c9ba4a46d5aab`（2026-09-07）。

- 许可证：MIT（见 `LICENSE`）
- 上游：<https://github.com/microsoft/tgrep>
- 为什么 vendored：未发布到 crates.io；theway 将其放在 `crates/` 下与 `tgrep-cli` 并列，使 `tgrep` 二进制从同一 pinned 源码构建。
- 工作区边界：根 `Cargo.toml` 将该 crate 列入 `[workspace.exclude]`，因此它不是根工作区成员，并声明自己的 `[workspace]`。`tgrep-cli` 通过 path 依赖 `tgrep-core = { path = "../tgrep-core" }` 消费它；用 `cargo test --manifest-path crates/tgrep-core/Cargo.toml` 或 `make vendored-test` 运行本 crate 的检查。
- 升级流程：下载新 pinned commit 的上游 tarball，逐字替换 `src/`。`Cargo.toml` 并非逐字：theway workspace 没有 `[workspace.dependencies]`/可继承元数据，因此 manifest 自包含且与上游版本/依赖完全一致 —— 升级时仅同步上游变更的版本/依赖值。**不要**手改 vendored 源码 —— 有 bug 上报上游。
