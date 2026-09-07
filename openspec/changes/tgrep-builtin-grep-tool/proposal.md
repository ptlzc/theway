## Why

内置 `grep` 工具（`crates/theway-daemon/src/tools/grep.rs`）每次查询都用 `ignore::WalkBuilder` 全量走树 + `regex` 逐行匹配——每次查询 O(总字节数)。在大仓（10w+ 文件）里一次 grep 要数秒到数十秒，而 agent 工具循环里 grep 是最高频操作之一。

[microsoft/tgrep](https://github.com/microsoft/tgrep)（MIT，Copilot CLI 同源）用三字符组（trigram）索引把查询压到只碰候选文件：`tgrep index` 预建索引、`tgrep serve` 常驻服务器（notify 文件监听 + 后台增量索引 + 定期落盘）、客户端查询自动连接服务器，无服务器/无索引时优雅回退为全量走树。在 gecko-dev（38.8w 文件）上平均比 ripgrep 快 52x。本 change 把 tgrep 接入 theway 内置 grep 工具：daemon 托管 `tgrep serve` 进程，grep 工具在索引就绪后走客户端路径，输出格式与现行为逐字节一致。

## What Changes

- **Vendor** `tgrep-core` 与 `tgrep-cli`（上游 commit `e2007b52`）进 `crates/`，沿用 `mermaid-rs-parser` 的 vendored 仓惯例（独立版本/许可证、根 Cargo.toml 注释登记）。
- **二进制分发**：`scripts/install.sh` 同步安装 `tgrep` 到 theway/thewayd 同目录；`.github/workflows/release.yml` 的平台构建把 `tgrep` 加进二进制清单。
- **daemon 托管服务器**：新增 `TgrepServerRegistry`（挂 `DaemonServices`，进程级共享）：按 canonical 根目录惰性 spawn `tgrep serve <root>`，经 `serve.json` + JSON-RPC `status` 轮询索引完成状态，LRU 上限（默认 3 个），daemon 退出时收割全部子进程。
- **GrepTool 双路径**：索引就绪的查询根 → `tgrep` 客户端 `--json` 输出经 ripgrep 兼容 JSON 流摄入，复用现有 formatter（content/files_with_matches/count、截断预览、`--` 分段、max_results 上限）；未就绪/无二进制/根外路径 → 现有 walker 路径（行为不变，永不返回半索引结果）。

## Impact

- `crates/tgrep-core`、`crates/tgrep-cli`：新 vendored 成员（上游源码逐字复制）。
- `Cargo.toml`：members + vendored 注释。
- `scripts/install.sh`、`.github/workflows/release.yml`：安装/发布 `tgrep` 二进制。
- `crates/theway-daemon/src/tgrep_server.rs`（新）：`TgrepServerRegistry`。
- `crates/theway-daemon/src/orchestration/services.rs`：`DaemonServices.tgrep`。
- `crates/theway-daemon/src/tools/grep.rs`：tgrep 客户端路径 + JSON 摄入；walker 路径保留为回退。
- `crates/theway-daemon/src/tools/mod.rs`：`local_tools_for_cwd_with_tgrep` 工厂，把 registry 注入 GrepTool。
- `crates/theway-daemon/src/tools/assembly.rs` / `node_launcher` 接线。
- 测试：JSON 摄入单测、registry 单测（注入二进制路径）、真实二进制 e2e（缺失时跳过）。
- `docs/architecture.md` 工具表。

## Out of Scope

- tgrep 上游功能改动（任何 bug 直接回报上游，vendored 目录只做逐字升级）。
- `find` / `ls` / `read` 等其他工具接入索引。
- 配置项（如禁掉 tgrep 路径、LRU 大小）先不暴露 config.toml，常量即可。
