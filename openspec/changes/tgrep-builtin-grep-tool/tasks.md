## 1. Vendor tgrep（上游 e2007b52，MIT）

- [x] 1.1 下载上游 tarball，逐字复制 `tgrep-core`/`tgrep-cli` 进 `crates/tgrep-core`/`crates/tgrep-cli`，各带 LICENSE 与 README 标注（上游仓库 + pinned commit + 升级流程）。
- [x] 1.2 根 `Cargo.toml`：members 增加两 crate；vendored 版本政策注释登记 tgrep（MIT）。
- [x] 1.3 `cargo check -p tgrep-cli` 通过；`cargo fmt --all --check` 与 `clippy -p tgrep-cli` 不报新增错误（vendored 目录豁免上游风格告警需在 clippy 配置登记）。

## 2. 二进制分发

- [x] 2.1 `scripts/install.sh`：与 thewayd 同批 `cargo install --path crates/tgrep-cli`，安装到同目录（thewayd 的同目录发现策略对 tgrep 同样适用）。
- [x] 2.2 `.github/workflows/release.yml`：平台构建二进制清单加入 `tgrep`（tar.gz/zip 资产、daemon 包同步）。

## 3. Daemon：TgrepServerRegistry

- [x] 3.1 新增 `crates/theway-daemon/src/tgrep_server.rs`：`TgrepServerRegistry`（`ensure_ready(root)`/`query_root(root)`/LRU/收割）。
- [x] 3.2 spawn `tgrep serve <root>`（`current_exe` 同目录 → PATH；stdio null；unix 进程组）；失败/早退记 tracing::warn 并标记 Missing。
- [x] 3.3 就绪轮询：读 `<root>/.tgrep/serve.json` 端口，TCP JSON-RPC `status`（短超时），`indexing=false` → Ready 缓存。
- [x] 3.4 LRU 上限 3：淘汰杀子进程；`Drop` 杀全部。
- [x] 3.5 `DaemonServices` 增 `tgrep: TgrepServerRegistry` 字段（Default 构造、测试可用注入二进制路径的构造器）。

## 4. GrepTool 双路径

- [x] 4.1 `grep.rs` 重构：把 `format_content` 的输入从 `Vec<RawMatch>` 收敛为「per-file line map + 匹配行字节区间」，walker 路径与 tgrep 路径共用 formatter。
- [x] 4.2 新增 tgrep 客户端执行路径：`tgrep -n -e <pattern> [-i] [-C n] [-g glob] --json --no-messages <path>`，NDJSON 摄入（begin/match/context/end），cancel token 杀进程，max_results 截断。
- [x] 4.3 路径选择：registry Ready 且查询根在 session cwd 下 → tgrep 路径；否则 walker 路径。
- [x] 4.4 `tools/mod.rs`：`local_tools_for_cwd_with_tgrep(executor, cwd, tgrep)`，`local_tools_for_cwd` 保持旧签名（None）；`session_tool_set_for_cwd`/`subagent_tool_sets_for_cwd`/`node_launcher` 接线 `services.tgrep`。

## 5. 测试

- [x] 5.1 单测：JSON 摄入（begin/match/context/end/submatches 字节偏移）与 walker 对同一 fixture 产出逐字节一致的三种 output_mode。
- [x] 5.2 单测：registry（注入假二进制路径）：spawn 失败/早退→Missing；就绪轮询（本地 TCP 假 status 服务器）；LRU 淘汰；Drop 收割。
- [x] 5.3 e2e（tests/tools/）：真实 `tgrep` 二进制（current_exe 同目录发现，缺失跳过）起 serve → grep 查询路径走通且格式正确。
- [x] 5.4 既有 grep/cwd/sandbox 测试保持全绿（walker 路径不受影响）。

## 6. 文档与收尾

- [x] 6.1 `docs/architecture.md` 工具表与直接 OS 工具段落补 tgrep 说明。
- [x] 6.2 `openspec validate --strict` 通过；`cargo test -p theway-daemon`、clippy、fmt 全绿。
- [x] 6.3 按任务小步提交（Conventional Commits 引用 #121），push main。
