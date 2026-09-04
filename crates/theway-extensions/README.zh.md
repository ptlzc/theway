# theway-extensions

[English](README.md) | 中文

`theway-extensions` 把官方 theway 运行时扩展包作为构建期数据打包：`packages/<extension-id>/` 下的包源文件通过 `include_str!` 嵌入并暴露为常量，daemon 在启动时把它们装配到 managed 扩展层（`<base>/extensions-managed/`）。本 crate 无运行时依赖。

插件 ABI 无版本号（见 `docs/extensions.md`），扩展包必须与匹配的 daemon 一起发布；把它们嵌入本 crate 使二者通过 workspace 版本耦合在一起。

| 常量 | 包 | 角色 |
|---|---|---|
| `TUI_DOCS` | `tui-docs` | 指向随二进制分发的 theway 配置指南的 prompt-section 指针；在 `SHIPPED_PACKAGES` 中。 |
| `DEEPSEEK_ANCHOR` | `deepseek-anchor` | 扩展文档的参考包；默认惰性（`zeroAnchor: true`），不分发。 |

`ensure_managed_installed(base)` 幂等地把 `SHIPPED_PACKAGES` 落盘到 `<base>/extensions-managed/`：每个包目录先写 staging 再原子 rename，仅内容变化时刷新。daemon 在扩展发现之前调用它。

## 文档

- [架构](docs/architecture.md)

## 验证

```bash
cargo test -p theway-extensions
cargo doc -p theway-extensions --no-deps --document-private-items
make layering-check
```
