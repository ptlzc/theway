# tgrep-cli 架构

[English](architecture.md) | 中文

Vendored 上游：[microsoft/tgrep](https://github.com/microsoft/tgrep)，commit `e2007b52d2b8fe4176159d0da20c9ba4a46d5aab`。MIT。

## 这个 crate 是什么

`tgrep` 二进制 —— 带客户端/服务器架构的三字符组索引 grep：

- 客户端搜索回退链：运行中的服务器（`<root>/.tgrep/serve.json`）→ 磁盘索引 → 暴力走树。每条路径都返回完整结果，只有速度不同。
- `--json` 输出 ripgrep 兼容的 NDJSON 流（`begin`/`match`/`context`/`end`/`summary` 记录）—— theway grep 工具消费的集成契约。

架构图（与英文版一致，不翻译）：

```
tgrep <pattern> <path> ---TCP JSON-RPC---> tgrep serve <root> (multi-client)
   (client, per query)                      |  HybridIndex (disk mmap + live overlay)
                                            |  notify file watcher, background indexer,
                                            |  periodic flush, hourly reconcile
```

## 与 theway 的边界

- daemon 为内置 grep 工具 spawn `tgrep serve <root>` 并执行 `tgrep ... --json` 客户端查询。spawn/就绪/LRU/收割逻辑在 daemon 内（`crates/theway-daemon/src/tgrep_server.rs`，issue #121）；本 crate 对 theway 一无所知。
- vendored `Cargo.toml` 自包含（独立元数据）；`src/` 下源码与上游逐字节一致，必须保持如此。
