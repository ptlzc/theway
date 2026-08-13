# Spec delta: layering (layering-debt-cleanup)

对 `openspec/specs/layering/spec.md` 的变更。

## MODIFIED Requirements

### Requirement: Core is interface-only

原要求保持;强化 scenario:

#### Scenario: No OS side effects in core
- **WHEN** 开发者查找进程执行(子进程 spawn、进程组 kill)或 HTTP webhook 的实现
- **THEN** core 只有注入 seam(hooks 副作用执行器),实现位于 theway-daemon;
  core 不依赖 libc/reqwest/tokio process

### Requirement: Single source of process-group kill semantics

原 scenario 覆盖 bash 工具、exec_shell 家族、native env 三处;修正为四处:

#### Scenario: Single source of process-group kill semantics
- **WHEN** bash 工具、exec_shell 家族、native env 或 **hook 命令执行器** 需要整树 kill
- **THEN** 四处共享 daemon 内同一份 setsid/killpg 原语,core 无副本
  (hooks 经注入的命令执行器消费同一原语)

## ADDED Requirements

### Requirement: Sandbox builds never degrade silently

sandbox 组合下不可用的运行时能力必须显式告警,不得静默返回空。

#### Scenario: Sandbox skill loading warns
- **WHEN** 以 `--no-default-features --features sandbox` 构建的 daemon 加载技能/模板
- **THEN** 组合根发出明确的 warn 日志("unavailable in sandbox build"),
  不静默表现为空目录

### Requirement: Session import activation guidance is actionable

导入带自动化会话时,未激活项的指引必须可直接执行。

#### Scenario: Import with disabled automation
- **WHEN** `/session import` 导入的会话含有源端已启用的 trigger/cron
- **THEN** daemon 输出未激活项 id 与 `/triggers enable <id>` / `/cron enable <id>`
  指引;不引用不存在的 flag(如 --activate-triggers=on)

### Requirement: Transport module zoning

theway-transport 内的模块分两组语义,文档与 lib.rs 结构清晰,公共路径不变。

#### Scenario: Zone lookup
- **WHEN** 开发者判断某模块属于协议实现还是共享契约
- **THEN** lib.rs 分组声明与 crate 文档的 "Module zones" 一节给出明确归属;
  共享面模块头部标注 shared client contract
