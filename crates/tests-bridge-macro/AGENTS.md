# Test bridge 修改规则

本文件适用于 `crates/tests-bridge-macro/`。同时遵循 [`../../AGENTS.md`](../../AGENTS.md) 和测试布局事实来源 [`../../docs/rust-test-files.md`](../../docs/rust-test-files.md)。

## 展开契约

- 调用点继续负责 `#[cfg(test)]`，使宏可作为 dev-dependency 使用。
- 保留归属 library/binary target 检测，integration target 不得再次编译镜像套件。
- 生成路径以调用 crate 的 `CARGO_MANIFEST_DIR/tests` 为根，并为 Rust path literal 规范化 separator。
- 发出 token 前拒绝不安全或含糊输入。输入解析、路径 containment、target 检测或诊断变化时添加编译展开测试。
- 除非全仓库 bridge 约定同步变化，否则生成模块名保持 `tests`。

## 边界

- 本过程宏不添加运行时依赖、test runner 行为、fixture 加载或源码模块策略。
- 标准套件布局与 inline test 例外只在 [`../../docs/rust-test-files.md`](../../docs/rust-test-files.md) 定义，本文件只引用它。
- 展开语义变化时至少验证一个真实消费者。

## 验证

运行 `cargo test -p tests-bridge-macro` 和 `cargo check --workspace --all-targets`。Mirror 或 target 行为变化时运行对应消费者测试。
