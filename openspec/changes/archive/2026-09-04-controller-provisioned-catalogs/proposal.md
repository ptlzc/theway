# Proposal: controller-provisioned-catalogs

## Why

#95 的第二版修复把 skill 目录供给改为 controller 全责：TUI 扫描本地根 → `WireDaemonConfig.skills` → daemon `Configure` 应用进 harness catalog + provision slot，controller 模式下 daemon 零 skill 文件 IO。该修复留下两个明确 caveat：

1. **templates 还留在 seam 里**：controller 模式下 `load_local_sources=false` 把 templates 一并关掉，但 TUI 没有对应供给通道 → 默认 TUI 流程中模板目录完全丢失（`/template <name>` 全部 miss）。
2. **gRPC proto 面无 skills/templates 字段**：`settings.proto` 的 `DaemonConfig` 是 `WireDaemonConfig` 的 twin，但新增的 `skills` 只存在于 serde JSON-RPC 面；外部客户端无法通过 gRPC 供给目录，违反 #59「新操作默认三协议可用」的方向。

本 change 收尾 controller 侧资源供给：templates 走与 skills 完全相同的「TUI 扫描 → wire 目录字段 → daemon 应用 + 保留」路径，并把两个目录字段补进 proto twin。

## What Changes

- **TUI 扫描 templates**：新增 `template_scan`（根目录平铺 `*.md`，project `.theway/templates` 覆盖 `base/templates`，frontmatter name/description），扫描结果进入 `WireDaemonConfig.templates`，reconcile 差量推送。规则镜像 daemon `crate::templates` 的加载器（不递归、根级 md、project-over-user）。
- **wire 契约**：`WireDaemonConfig` 新增 `templates: Vec<WireProvisionedTemplate>`（含 body），FIELDS/merge_from/clear 同步；`WireProvisionedTemplate` 形状对应 `PromptTemplate`（name/description/content/file_path）。
- **daemon**：`SessionProjectResources` 的 seam 重新覆盖 templates（controller 模式不扫盘，恢复 #95 之前的 seam 语义——但这次 templates 有供给通道，不会丢）；新增 `provisioned_templates` slot；reload 闭包在 controller 模式保留供给目录；`Configure` 应用 templates 分支（替换 harness template catalog + 写 slot + 配置视图回显）。
- **proto twin**：`settings.proto` `DaemonConfig` 增加 `repeated ProvisionedSkill skills` / `repeated ProvisionedTemplate templates`（message 与 wire 字段一一对应），`resources.rs` 双向转换 + round-trip 测试。

## Capabilities

### Modified Capabilities

- `session-resource-model`: 增加「controller-provisioned 技能与模板目录」需求——seam 语义、供给通道、reload 保留、清除语义。
- `external-protocol-service`: 增加「设置契约的目录供给字段跨协议一致」需求——settings twin 携带 skills/templates，gRPC 与 JSON-RPC 行为一致。

### New Capabilities

- 无。

## Impact

- `crates/theway-tui`: 新增 `template_scan` 模块；`config_payload` 组装与 reconcile 扩展。
- `crates/theway-transport`: wire 类型 + proto 消息 + 转换函数；serde 与 proto 两侧测试。
- `crates/theway-daemon`: `SessionProjectResources`（seam + slot + reload 闭包）、`Configure` applier；回归测试。
- `docs/architecture.md`：controller 供给边界描述同步（skills/templates 两个目录）。
- 参考 GitHub issues：#96（父）。

## Non-Goals

- 不改 `config.toml` schema；`/template` 命令交互不变。
- MCP `tools/list` 不需要目录字段（工具调用走共享服务），不扩 MCP 面。
- 独立 `thewayd`（无 controller）保持本地扫盘行为。
- 不触碰 skill overrides 的供给（仍为 daemon 本地状态，独立于本 change 的目录供给面）。
