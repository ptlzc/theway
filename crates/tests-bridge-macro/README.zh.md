# tests-bridge-macro

[English](README.md) | 中文

`tests-bridge-macro` 提供 `tests_bridge!` 过程宏，将镜像多文件测试套件挂载到其归属源码模块。它解决 `#[path]` 只能接受字面路径的问题，同时保留单元测试访问私有项的语义。

源码模块中的调用：

```rust,ignore
#[cfg(test)]
tests_bridge_macro::tests_bridge!("agent/session");
```

会在归属 crate 的单元测试 target 中生成以 `CARGO_MANIFEST_DIR` 为根的绝对 `#[path = "…/tests/agent/session/mod.rs"] mod tests;`。源码调用点负责 `#[cfg(test)]`，该宏通常作为 dev-dependency 使用。

同一源码被 integration-test crate 通过 path 引入时，宏比较 Cargo target 环境并不生成任何 token。这样可避免镜像套件针对不同 crate root 编译两次，或对进程全局测试状态产生竞争。

测试布局和 bridge 放置由 [`../../docs/rust-test-files.md`](../../docs/rust-test-files.md) 统一规定。展开机制见 [`docs/architecture.md`](docs/architecture.md)，修改规则见 [`AGENTS.md`](AGENTS.md)。

## 验证

```bash
cargo test -p tests-bridge-macro
cargo check --workspace --all-targets
```
