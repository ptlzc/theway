# Test bridge 架构

[English](architecture.md) | 中文

## 职责

`tests-bridge-macro` 在宏展开期间把镜像测试路径转换为以 crate 根为锚点的模块声明。它只提供路径锚定；归属源码模块决定测试是否编译，[`docs/rust-test-files.md`](../../../docs/rust-test-files.md) 决定套件存放位置。

## 展开流程

[`tests_bridge`](../src/lib.rs) 执行以下操作：

1. 读取 `CARGO_CRATE_NAME`、`CARGO_PKG_NAME` 和 `CARGO_BIN_NAME`，把 package 与 binary 名中的连字符规范化为下划线。
2. 活动 target 不是 package 自身 library 或 binary 单元测试 target 时，返回空 token stream。
3. 将输入 token stream 转成字符串、去掉外围引号字符，并拒绝空 mirror 或包含 `..` 的 mirror。
4. 拼接 `CARGO_MANIFEST_DIR`、`tests`、mirror 和 `mod.rs`，将 separator 规范化为正斜杠，然后生成 `#[path = "<absolute path>"] mod tests;`。

调用契约是 `"agent/session"` 这样的带引号相对 mirror。上面逐项列出宏实际执行的校验；修改输入解析或路径 containment 需要专门的编译展开覆盖。

## Target 过滤

Integration test 可以在启用 `cfg(test)` 时通过 path 引入源码模块。此时 `CARGO_CRATE_NAME` 指向 integration target，而不是归属 package 或 binary，因此宏展开为空。原始套件只在 library/binary crate root 下编译一次，并保留单元测试可见性。

## 边界与不变量

- 源码调用点保留 `#[cfg(test)]`；生产编译不需要该 dev-dependency。
- 生成模块名固定为 `tests`，target 文件固定为 mirror 下的 `mod.rs`。
- 展开只依赖编译期 Cargo 环境，不执行运行时工作。
- 宏没有非 std 依赖，也不包含测试发现或执行策略。
