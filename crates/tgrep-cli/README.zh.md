# tgrep-cli（vendored）

[English](README.md) | 中文

[`tgrep-cli`](https://github.com/microsoft/tgrep) 的 vendored 副本 —— `tgrep` 二进制（带客户端/服务器架构的三字符组索引 grep）。**逐字**复制自上游 commit `e2007b52d2b8fe4176159d0da20c9ba4a46d5aab`（2026-09-07）。

- 许可证：MIT（见 `LICENSE`）
- 上游：<https://github.com/microsoft/tgrep>
- 为什么 vendored：未发布到 crates.io；theway 将其作为 workspace 成员构建，daemon 为内置 grep 工具（issue #121）spawn `tgrep serve` + `tgrep` 客户端查询。二进制与 `theway`/`thewayd` 并列安装。
- 升级流程：下载新 pinned commit 的上游 tarball，逐字替换 `src/`。`Cargo.toml` 并非逐字：theway workspace 没有 `[workspace.dependencies]`/可继承元数据，因此 manifest 自包含且与上游版本/依赖完全一致 —— 升级时仅同步上游变更的版本/依赖值。**不要**手改 vendored 源码 —— 有 bug 上报上游。
