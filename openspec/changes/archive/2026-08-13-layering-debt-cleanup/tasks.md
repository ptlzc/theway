# Tasks: layering-debt-cleanup

每项完成后 workspace 编译通过;逐项提交(引用 issue #19)。

## 1. hooks 副作用 seam

- [x] 1.1 core: `HookRunner` 构造改为注入命令执行器 + webhook 发送器
      (类型别名 Arc<dyn Fn>,未注入时诊断跳过);`run_command`/`run_webhook` 改经执行器
- [x] 1.2 daemon: 实现两个执行器 —— 命令执行器复用 `tools/exec.rs` 单点 kill 原语,
      webhook 发送器用 reqwest;thewayd / session_factory 组装点注入
- [x] 1.3 core 测试适配: 未注入路径显式断言诊断;harness_e2e hooks 用例跑通
- [x] 1.4 core 依赖清理: 移除 libc / reqwest / tokio process feature(逐项验证)

## 2. 行为缺陷修复

- [x] 2.1 `/session import` 激活指引改为 id 列表 + `/triggers enable` / `/cron enable`
- [x] 2.2 sandbox 组合 skills/templates 空实现改 tracing::warn!(组合根一次性)

## 3. transport 语义分区

- [x] 3.1 lib.rs 分组声明(protocol / shared)+ crate 文档 "Module zones" 一节
- [x] 3.2 共享面模块头部标注 shared client contract(公共路径不变)

## 4. 终态验证

- [x] 4.1 `cargo test --workspace --no-fail-fast` 全绿
- [x] 4.2 `cargo clippy --workspace --all-targets -- -D warnings` + `cargo fmt --all --check`
- [x] 4.3 daemon 特性矩阵: default / --no-default-features --features sandbox / --all-features
- [x] 4.4 归档变更(openspec archive)与 issue #19 收口
