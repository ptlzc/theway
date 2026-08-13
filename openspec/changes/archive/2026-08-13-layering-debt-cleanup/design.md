# Design: layering-debt-cleanup

## Context

daemon-kernel-layers(#18)后的事实(已核实):

- `core/src/agent/hooks/mod.rs`:`HookRunner` 持有 `reqwest::Client`,`run_command` 用
  `tokio::process::Command` + `libc::setsid`/`killpg` 做整树 kill,`run_webhook` 用 reqwest。
  hooks 其余部分(规则解析 `HookRule`、事件编排 `listener`/`harness_listener`、payload 组装
  `HookPayload`、诊断)是纯逻辑。
- daemon 已有单点进程原语:`tools/exec.rs` 的 `run_with_kill_on_timeout_or_cancel`(bash 工具、
  exec_shell 家族、native env 三处已共享)。
- transport 现状:`src/` 19 个模块,其中 8 个是协议实现,11 个是 #18 从 sdk 吸收的共享面
  (auth/history/mentions/bug_report/images/commands/config/triggers/feed + client)。

## Goals / Non-Goals

- **Goals**:core 不再持有任何进程/webhook 副作用实现(interface-only 叙事兑现);两个行为
  缺陷修复;transport 两组语义在文档与结构上清晰区分,公共路径零破坏。
- **Non-Goals**:新 crate、运行时 executor 注入、协议改动、测试减脆。

## Decisions

### 1. hooks 副作用 seam:构造注入,函数对象优先

`HookRunner` 的构造改为接收两个可注入执行器:

```rust
/// 命令执行器: 起子进程、整树 kill、超时/取消语义。core 只定义 seam,
/// 实现由 daemon 提供 (进程组 kill 单点)。
pub type HookCommandRunner = Arc<dyn Fn(&str, &std::path::Path, Option<&std::path::Path>,
    u64) -> ... + Send + Sync>;

/// webhook 发送器: POST payload + headers, 返回状态码。
pub type HookWebhookSender = Arc<dyn Fn(&str, &str, &str) -> ... + Send + Sync>;
```

- 用类型别名 + `Arc<dyn Fn>` 而非新 trait:每个 seam 只有一个实现(daemon),trait 的抽象
  收益不抵成本;闭包可捕获 daemon 侧构造参数。
- 返回值形状与现实现对齐(命令:退出码 + 输出摘要;webhook:状态码),不放大接口。
- **未注入时的行为**:`HookRunner` 没有执行器时,命令/webhook 规则不执行并在 diagnostics
  记录一条"no side-effect executors injected"提示——core 裸用(不接 daemon)时 hooks 退化为
  纯诊断,不再拥有隐式的 OS 副作用能力。现有 harness_e2e hooks 测试核查后适配。

### 2. daemon 提供执行器,复用单点 kill 原语

`tools/exec.rs` 现有 `run_with_kill_on_timeout_or_cancel` 直接作为命令执行器的内核;
webhook 发送器用 daemon 已有的 reqwest 依赖(与 web_fetch 同构)。组装点:thewayd /
session_factory 的 hooks 加载处。

### 3. core 依赖收敛

移除 `libc`、`reqwest`、tokio `process` feature(逐项验证后删);`directories`(base_dir /
HookCwd)、`toml`(规则解析)、`sha2`(promotion 指纹)保留。

### 4. import 激活指引(不引入协议)

`SessionImportActivation` 分支输出:会话路径 + 未激活 id 列表(截断至前 N 个)+ 指引
`/triggers enable <id>` 与 `/cron enable <id>`。移除 `--activate-triggers=on` 的误导措辞。

### 5. sandbox 告警

skills/templates 的 `#[cfg(not(feature = "local"))]` 空实现改 `tracing::warn!`(含 "sandbox
build" 字样,一次性语义由调用方保证——组合根启动只调一次)。默认 local 构建零影响。

### 6. transport 分区(组织层,不改公共路径)

- `src/lib.rs` 分两组声明 + 组注释;crate 文档新增 "Module zones" 一节。
- shared 面模块(11 个)头部统一标注 `//! shared client contract (not protocol)`。
- 不重命名模块、不改 re-export,公共路径与依赖零变化。

## Risks / Mitigations

- **hooks 注入改变 core 裸用行为**:未注入时不执行副作用——核对 harness_e2e 的 hooks
  用例与 daemon 组装点,确保 daemon 路径全部注入;core 测试显式断言未注入时的诊断路径。
- **函数对象签名漂移**:命令执行器签名若需扩展(如 stdout 流式),本轮只包当前需求
  (退出码 + 输出摘要),扩面留真 sandbox 轮。
- **sandbox 告警刷屏**:warn 只打在组合根首次加载处,不在每会话重复。

## 迁移顺序(每步可编译)

1. core hooks seam(类型 + HookRunner 构造) + daemon 执行器实现 + 组装点注入;core 测试适配
2. core 依赖清理(libc/reqwest/process feature)
3. import 指引 + sandbox 告警(独立小修)
4. transport 分区文档
5. 终态验证(test/clippy/fmt/特性矩阵)
