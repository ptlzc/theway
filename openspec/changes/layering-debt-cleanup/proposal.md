# Proposal: layering-debt-cleanup

## Why

daemon-kernel-layers(#18)把分层收敛到 core=interface / daemon=唯一内核 / transport=唯一契约,
但交付评审留下三处债务:

- **"进程组 kill 语义单点在 daemon"没有兑现**。spec 宣称 bash/exec_shell/native env 三处共享
  daemon 内同一份原语、core 无副本,但 `core/src/agent/hooks/mod.rs`(426/498 行)有第四份
  `setsid`/`killpg`,且 core 至今仍直接依赖 `tokio::process`/`libc`/`reqwest`——全部只为 hooks
  服务。core=interface 的叙事在这里有一个洞。
- **transport 一个 crate 装了两类东西**:协议层(wire/grpc/http/ws/mcp/proto/client/host/
  transport/inbox)与共享工具抽屉(auth/history/mentions/bug_report/images/commands/config/
  triggers/feed)。两类语义混在同一 crate 名与扁平模块列表里,定位模糊。
- **两个行为缺陷**:
  - `/session import` 导入带自动化的会话时,daemon 提示 "re-import with
    --activate-triggers=on"(`app/daemon.rs:538-546`),但 `/session import` 没有这个 flag
    (那是 CLI 子命令的),指引不可执行。
  - sandbox 组合(`--no-default-features --features sandbox`)下 `skills::load_all` /
    `templates::load_all` 静默返回空——编译绿,但运行起来是一个无告警、无技能的 daemon。

## What Changes

- **hooks 副作用抽注入点**(核心):core 的 `HookRunner` 不再直接 `tokio::process` 起子进程、
  不再 `libc::setsid/killpg`、不再 `reqwest` 发 webhook;改由构造时注入两个执行器
  (命令执行器 / webhook 发送器)。core 保留 hooks 机制本体(规则解析、事件编排、payload
  组装、诊断),仅副作用执行外置。daemon 提供实现:命令执行器复用 daemon 内单点进程组
  kill 原语(`tools/exec.rs`),webhook 发送器用 daemon 的 reqwest。core 移除
  `libc`/`reqwest`/`tokio::process` 依赖后,spec 的"单点 kill 语义"才真正成立。
- **import 激活指引修正**:`SessionImportActivation` 分支改为可执行指引——列出未激活的
  trigger/cron id,提示 `/triggers enable <id>` / `/cron enable <id>`(或一次
  `/cron enable <id>` 逐个)。不引入新协议。
- **sandbox 组合显式告警**:sandbox build 的 skills/templates 空实现改为 `tracing::warn!`
  ("skill discovery unavailable in sandbox build" 级别),组合根首次调用即告警一次,
  不再静默。
- **transport 语义分区**:模块分两组——protocol(wire/grpc/http/ws/mcp/proto/client/host/
  transport/inbox/testing)与 shared(auth/bug_report/commands/config/feed/history/images/
  mentions/triggers)。公共路径不变(零破坏),lib.rs 分组声明 + crate 文档写明两组语义;
  共享面模块头部注释标注"shared client contract"。

## Impact

- **core 依赖收敛**:移除 `libc`、`reqwest`、`tokio` 的 `process` feature(hooks 迁出副作用后
  逐项验证);`directories`/`toml` 等仍为 hooks 规则解析所需,保留。
- **行为不变**:默认 local 构建下 hooks 命令/webhook 行为一致(执行器由 daemon 注入,
  语义与现实现相同);wire/gRPC/HTTP 协议不改。
- **文件**:core hooks ±1 文件(执行器 seam);daemon +1(hook 执行器实现或并入现有模块);
  transport 文档/注释整理。
- **验证**:`cargo test --workspace` + `clippy -D warnings` + `fmt --check` +
  daemon 特性矩阵三组合;hooks_e2e 现有测试保持全绿。

## 非目标

- 不拆新 crate(transport 保持单 crate;共享面 <10 文件,拆出是过度设计)。
- 不做 P1-3 测试 path-include 减脆(留后续轮)。
- 不做 local/sandbox 编译期 feature → 运行时 executor 注入(接真 sandbox 时再定)。
- 不改 wire/gRPC/HTTP 协议;不为 import 激活恢复交互卡片(控制平面化留后续)。
