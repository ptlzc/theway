# RUST_TEST_FILES.md — 测试文件管理规范

仓库测试文件布局与命名规范。参考 .NET 解决方案惯例 (src/test 分离、1:1 镜像、
命名三段式、AAA 模式) 的 Rust 适配版。**本规范是事实源** — 新测试必须遵循,
旧测试逐步对齐。

## 1. 目录分离: src 与 tests 不混放

每个 crate 的测试分两类, **物理位置**严格分离:

```
crates/<name>/src/      # 仅生产代码
crates/<name>/tests/    # 所有测试文件 (多文件模块测试 + 独立集成测试)
```

| 测试类型 | 位置 | 说明 |
|----------|------|------|
| 内联单测 (`mod tests { }` 同文件) | `src/` 内联 | Rust 惯例, 轻量断言贴近被测代码; 超过 ~30 行或需要 fixture 时**必须**拆出 |
| 多文件模块测试 (`mod tests;` 引 `tests/` 子目录) | `tests/<镜像路径>/` | 需要私有访问的较大测试套件, 经 `#[path]` 从 src 引入 (见 §2) |
| 独立集成测试 / e2e (独立二进制 target) | `tests/` 顶层 `<name>.rs` | 只能访问 pub API, 经进程/网络/CLI 驱动 |

> 为什么 src 里还能看到 `#[path] mod tests` 一行? Rust 中只有 `#[path]` 能
> 让 `tests/` 下的文件以**单元测试语义** (可访问私有项) 编译。那**一行桥接声明**
> 是语言要求, 测试文件本身不在 src 下。

## 2. 1:1 镜像: tests/ 内部目录镜像 src/ 路径

多文件模块测试的目录结构**完全镜像**被测模块的 src 路径:

```
crates/core/src/runtime/graph_engineering/engine.rs   ←→  crates/core/tests/runtime/graph_engineering/engine/
crates/core/src/tools/dag_tools.rs                    ←→  crates/core/tests/tools/dag_tools/
crates/server/src/tools/shell.rs                      ←→  crates/server/tests/tools/shell/
crates/server/src/transport/http.rs                   ←→  crates/server/tests/transport/http/
```

桥接声明 (src 模块末尾):

```rust
#[cfg(test)]
#[path = "../../tests/tools/dag_tools/mod.rs"]  // 相对本文件所在目录
mod tests;
```

镜像目录内的 `mod.rs` 声明子模块 (`mod plan;` 等), 每个子模块文件对应一个
测试面。**禁止**在 src 下新建 `tests/` 目录。

> cargo 只把 `tests/` **顶层** `.rs` 自动编译为集成测试 target; 镜像子目录
> 下的文件不会成为独立 target (仅被 `#[path]` 引用), 因此可以安全存放
> 依赖 `super::` 私有项的测试代码。

## 3. 命名规范

### 文件命名

- 镜像模块测试: 目录镜像被测路径; `mod.rs` 内子模块按**被测场景/功能面**命名
  (snake_case, 与源模块的 public 面一一对应): `plan.rs` → 测 `dag_plan`, `wait.rs` → 测 `dag_wait`。
- 独立集成测试: `<被测模块>_e2e.rs` (跨进程/网络/CLI 真实验证) 或
  `<被测模块>.rs` (同 crate 内驱动, 如 `tools.rs`)。禁止无意义后缀 (如 `_test.rs`)。

### 测试函数命名 (三段式)

`被测方法_测试场景_预期结果` — 看到名字即知业务逻辑与断言方向:

```rust
#[tokio::test]
async fn plan_param_errors() {}               // 被测: dag_plan; 场景: 参数错误
#[test]
fn wait_times_out_on_stuck_run() {}          // 被测: dag_wait; 场景: 卡死 run; 结果: 超时
#[tokio::test]
async fn with_retry_skips_cancelled_downstream() {}
```

坏例子: `test_plan`, `check`, `verify_something` (不含被测方法/场景/预期)。

## 4. 测试代码结构: AAA 模式

每个测试函数内部严格 `Arrange → Act → Assert` 三段, 用注释标记 (复杂测试)
或空行分隔 (简单测试):

```rust
#[test]
fn resolve_spec_rejects_unknown_agent() {
    // Arrange: 准备输入
    let name = "no-such-agent";

    // Act: 调用被测
    let resolved = resolve_spec(name);

    // Assert: 验证结果
    assert!(resolved.is_none());
}
```

- Arrange 不与被测逻辑混写; Act 只调用一个被测入口; Assert 只做验证。
- fixture 构造重复时抽到 `mod.rs` 的 `pub(super) fn` helper (如
  `engine_with_launcher()`), 不复制。
- 平台差异 (Windows/Unix) 用 `#[cfg(windows)]` / `#[cfg(not(windows))]` 块,
  在 helper 中集中, 不在用例内散落。

## 5. 验证

- 迁移/新增后: `cargo build --workspace` + `cargo test --workspace --no-fail-fast` +
  `cargo clippy --workspace --all-targets -- -D warnings` + `cargo fmt --all --check`。
- 测试数量不因迁移减少 (git mv 保留历史, 迁移前后 `test result` 计数一致)。
