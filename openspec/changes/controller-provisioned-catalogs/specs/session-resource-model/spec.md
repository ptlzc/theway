# Session Resource Model

规范 controller-provisioned 技能/模板目录的供给语义（seam、供给通道、reload 保留）。

## ADDED Requirements

### Requirement: Controller-provisioned 技能与模板目录

`SessionProjectResources` SHALL 仅在 standalone 模式（`load_local_sources = true`）从本地根扫描 skills 与 templates。controller-provisioned 会话（`load_local_sources = false`）SHALL 通过设置契约（`WireDaemonConfig.skills` / `WireDaemonConfig.templates`）接收完整目录（含正文），daemon 在该模式下 SHALL NOT 读取任何 skill/template 文件。供给目录 SHALL 写入共享 provision slot，reload 闭包在该模式下 SHALL 以 slot 快照重载，不得因磁盘扫描而清空供给目录。

#### Scenario: Controller 模式零文件 IO

- **WHEN** daemon 以 controller-provisioned 模式构建 session 资源
- **THEN** skills 与 templates 初始目录来自设置契约而非磁盘
- **AND** daemon 不执行任何 skill/template 文件的读取

#### Scenario: 供给目录进入 harness 目录

- **WHEN** controller 通过 `Configure` 推送 `skills` / `templates` 字段
- **THEN** harness 的技能/模板目录以供给内容替换（builtin skills 保留）
- **AND** 配置视图与 provision slot 同步回显该目录

#### Scenario: Reload 保留供给目录

- **WHEN** controller 模式下触发 `/reload` 或 `SetSkillDirs`
- **THEN** 重载结果包含当前供给目录
- **AND** 供给目录不因跳过磁盘扫描而被清空

#### Scenario: 清除供给目录

- **WHEN** controller 推送 `clear_fields: ["skills"]`（或 `["templates"]`）
- **THEN** harness 对应目录中非 builtin 条目被移除
- **AND** provision slot 与配置视图同步清空

#### Scenario: Standalone 模式保持本地扫描

- **WHEN** daemon 以 standalone 模式构建 session 资源
- **THEN** skills 与 templates 照常从本地根扫描加载
- **AND** reload 闭包重新扫描磁盘
