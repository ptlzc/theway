# theway-extensions 架构

[English](architecture.md) | 中文

## 归属与依赖方向

`theway-extensions` 是一个叶子数据 crate：运行时零依赖，把官方扩展包源文件作为常量嵌入。daemon 依赖它；它不依赖任何其他 workspace crate，因此在分层顺序中位于靠近叶子的一端（发布 allowlist 中在 `theway-daemon` 之前）。

## 数据模型

- `EmbeddedPackage { id, files }` —— 一个 manifest id 加 `(相对路径, 内容)` 对构成的包目录。
- `TUI_DOCS` / `DEEPSEEK_ANCHOR` —— 两个官方包。包文件用 `include_str!("../packages/<id>/…")` 嵌入；磁盘上的 `packages/` 目录仍是唯一事实源，重新构建即拾取修改。
- `SHIPPED_PACKAGES` —— 装配进 managed 层；`ALL_PACKAGES` —— 分发包 + 参考包，供测试与工具使用。

## Managed 层装配

`ensure_managed_installed(base)` 把 `SHIPPED_PACKAGES` 写入 `<base>/extensions-managed/<id>/`：

1. 逐个比较嵌入文件与已安装副本；全部一致则跳过该包。
2. 把整个包先写入 `extensions-managed/.<id>-staging`，再 rename 覆盖目标——目录级原子，catalog 永远不会看到写了一半的包。
3. 失败只告警（`tracing::warn`）；缺少 managed 副本退化为"指针包不存在"，绝不影响启动。

daemon 在 `SessionExtensionResources::new` 里、`ExtensionRegistry::discover` 之前调用它，因此每次启动时 catalog 的 managed 层都能看到嵌入的包。Managed 包免信任记录、按声明权限授予（平台随发行、用户只读），project / user 层同名包仍然会遮蔽它们。

## 验证

单元测试断言 manifest 合法性（JSON 可解析、`id` 与常量名一致、entry 文件已嵌入）、缺失包落盘、幂等性、过期内容刷新、整目录替换与 staging 清理。
