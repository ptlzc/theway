# external-protocol-service Specification

规范设置契约的目录供给字段跨协议一致。

## ADDED Requirements

### Requirement: 设置契约的目录供给字段跨协议一致

共享设置契约 SHALL 在 gRPC（settings proto twin）与 JSON-RPC（wire）两个面上都携带 controller-provisioned 技能/模板目录字段（skills/templates，含正文），双向转换 SHALL 无数据丢失。gRPC 客户端推送的目录 SHALL 与 JSON-RPC 客户端推送产生相同的 daemon 应用结果。

#### Scenario: gRPC 供给技能与模板目录

- **WHEN** 外部客户端通过 gRPC `Configure` 推送 `skills` / `templates` 目录
- **THEN** daemon 应用的目录与等价的 JSON-RPC 推送一致
- **AND** `GetConfig` 经 proto 返回的目录与 wire 视图一致

#### Scenario: Proto twin 转换无丢失

- **WHEN** 目录条目含全部字段（name/description/content/file_path/source/disable 标志）
- **THEN** wire → proto → wire round-trip 逐字段相等
