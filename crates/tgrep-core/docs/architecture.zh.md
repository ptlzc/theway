# tgrep-core 架构

[English](architecture.md) | 中文

Vendored 上游：[microsoft/tgrep](https://github.com/microsoft/tgrep)，commit `e2007b52d2b8fe4176159d0da20c9ba4a46d5aab`。MIT。

## 这个 crate 是什么

`tgrep` CLI 背后的三字符组索引库 —— 对仓库文本文件做全文索引，让正则查询只触碰候选文件而无需走树：

- `builder` —— 走树（gitignore 感知）、提取三字符组、写磁盘索引（`<root>/.tgrep`）；external merge 策略约束超大仓库的峰值内存。
- `reader` / `ondisk` —— mmap 索引读取器，三字符组查找表二分查找。
- `live` / `hybrid` —— 内存覆盖层（服务器启动后变更的文件）叠加在磁盘索引之上；覆盖层优先。
- `query` —— 三字符组查询规划（有序 posting-list 求交/并）与候选文件正则搜索。
- `walker` —— 文件走查：二进制拒绝、体积上限、gitignore / `core.ignorecase` 处理。
- `path_index` —— `--files` 合并用的文件名索引。

## 与 theway 的边界

- theway **不链接**本 crate。daemon 通过 shell 调用 `crates/tgrep-cli` 构建出的 `tgrep` 二进制；服务器生命周期（spawn/就绪/LRU/收割）在 daemon 内实现（`crates/theway-daemon/src/tgrep_server.rs`，issue #121）。
- vendored `Cargo.toml` 自包含（独立元数据）；`src/` 下源码与上游逐字节一致，必须保持如此。
