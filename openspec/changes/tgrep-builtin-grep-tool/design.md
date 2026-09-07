## Context

tgrep 客户端查询链路（来自上游 `tgrep-cli/src/search.rs`，本设计依赖的既有行为）：

```
tgrep <pattern> <path>
  → canonicalize root → 默认索引目录 <root>/.tgrep
  → ServerInfo::load(.tgrep/serve.json) 命中 → TCP JSON-RPC search（服务器）
      连接失败 → 本地索引 search_local_index
  → 无索引 → brute-force 走树（stderr 警告，结果仍完整正确）
```

`tgrep serve` 特性：后台建索引（external merge，峰值内存有界，默认 50% CPU/内存上限）、建索引期间即时回答部分数据、notify 监听增量、每 50K 文件或 5 分钟落盘、每小时 reconcile 对账、多客户端并发。**关键正确性风险：建索引期间的查询只覆盖已索引子集**，agent 工具不能吞这个窗口。

## Goals / Non-Goals

- Goals：大仓 grep 从「每次走树」变「索引就绪后即时查询」；输出格式逐字节不变；无二进制/服务器死掉/索引未就绪时结果永远完整正确。
- Non-Goals：不把 tgrep 索引用于其他工具；不暴露配置；不改上游源码（逐字 vendor，升级=重拷贝）。

## Decisions

### D1. Vendor tgrep-cli 二进制 + daemon 托管 serve 进程（而非进程内索引）

备选：vendor `tgrep-core` 库，daemon 内实现索引管理（后台构建 + notify + 落盘 + 查询）。放弃原因：这些编排逻辑全在上游 `tgrep-cli/src/serve.rs`（530KB），进程内重写=复制上游大量硬化逻辑（watcher 队列溢出、reconcile 对账、落盘节流），维护成本高且必然漂移；daemon 本就有管理子进程的先例（MCP stdio 服务器、LSP supervisor）。选托管进程：上游逻辑零移植，升级=重拷贝两个 crate，客户端无服务器时自带走树回退兜底正确性。

### D2. 就绪门：索引完成前 walker 路径作答

`tgrep serve` 建索引期间回答部分数据。为避免 agent 拿到不完整结果：registry 轮询 JSON-RPC `status`（`{"indexing": bool, ...}`），只有 `indexing == false` 才标记 Ready。GrepTool 在 Ready 前（以及二进制缺失、spawn 失败、查询根不在 session cwd 下）走现有 walker 路径。索引完成状态只上升不下降（watcher 保持增量新鲜），registry 缓存 Ready，不重复轮询。server 死后 client 自己的走树回退兜底，registry 检测到子进程退出后清条目、下个查询重新 spawn。

### D3. 输出保真：JSON 流摄入 → 复用现有 formatter

tgrep `--json` 输出 ripgrep 兼容 NDJSON（`begin`/`match`/`context`/`end`/`summary`，`data.lines.text`、`data.line_number`、`data.submatches[].start/end` 字节偏移）。摄入层把记录转成现有 formatter 的输入（per-file line map + 匹配行字节区间），`format_content`/`format_counts`/`format_files_with_matches` 一字不动，walker 与 tgrep 两条路径共享同一 formatter，输出逐字节一致。max_results 上限在摄入层截断（达到上限即杀客户端进程，context 记录已缓冲的保留）。

### D4. Registry 形态：DaemonServices 字段 + Arc 共享

`TgrepServerRegistry`（`Arc<Mutex<HashMap<PathBuf, Entry>>>`）挂在 `DaemonServices.tgrep`（进程级共享，tgrep serve 本就多客户端）。key = canonical session cwd。spawn 用 `current_exe()` 同目录优先、PATH 兜底（与 TUI 找 thewayd 同策略）。serve 子进程 stdio null、unix 下新进程组。LRU 上限 3，淘汰即杀；registry Drop 杀全部。测试缝：构造函数可注入二进制路径。

### D5. GrepTool 只在 session cwd 下走 tgrep 路径

registry 按 session cwd 建服务器，客户端查询可 scoping 到子树（上游 `resolve_scope` 支持）。查询根不在 session cwd 下（外部绝对路径）→ walker 路径，不为此建服务器（避免索引任意目录）。

## Risks / Trade-offs

- **首次查询慢**：serve 冷启动建索引期间 grep 走 walker，与现状相同；索引完成后才加速。可接受（正确性优先）。
- **内存/CPU**：serve 建索引默认 50% CPU + 50% 内存上限（上游默认，仅构建期）；稳态只有文件监听与增量。多 session 同根目录共享一个服务器。
- **跨平台**：spawn/kill 逻辑沿用 daemon 既有子进程模式；`serve.json` 陈旧文件由上游客户端自行处理（连不上→回退）。
- **vendor 体积**：两个 crate 约 1.4MB 源码，不影响运行时二进制（tgrep 独立 bin）。
