# rust-test-files.md — 测试文件管理规范

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
| 多文件模块测试 | `tests/<镜像路径>/` | 需要私有访问的较大测试套件, src 模块末尾一行桥接 (见 §2) |
| 独立集成测试 / e2e (独立二进制 target) | `tests/` 顶层 `<name>.rs` | 只能访问 pub API, 经进程/网络/CLI 驱动 |

> 为什么 src 里还能看到一行桥接? 测试文件物理在 `tests/` 下, 但需要以**单元
> 测试语义** (可访问被测模块私有项) 编译 — Rust 的 `#[path]` 属性是唯一机制。
> 而 `#[path]` 只接受字符串字面量 (属性求值早于宏展开, `concat!`/`env!` 不可用,
> 相关 RFC 2320 已关闭), 所以用 `tests-bridge-macro` 的 proc-macro 在展开阶段
> 生成绝对路径 — 这是语言限制下唯一可行的"顶层锚定"方案。**那**一行桥接调用
> 是必要的, 测试文件本身不在 src 下。

## 2. 1:1 镜像: tests/ 内部目录镜像 src/ 路径

多文件模块测试的目录结构**完全镜像**被测模块的 src 路径:

```
crates/theway-core/src/runtime/graph_engineering/engine.rs   ←→  crates/theway-core/tests/runtime/graph_engineering/engine/
crates/theway-core/src/tools/dag_tools.rs                    ←→  crates/theway-core/tests/tools/dag_tools/
crates/theway-daemon/src/tools/bash.rs                       ←→  crates/theway-daemon/tests/tools/bash/
crates/theway-transport/src/http.rs                          ←→  crates/theway-transport/tests/http/
```

桥接声明 (src 模块末尾, 用 `tests-bridge-macro` 的 proc-macro — 展开时以
`CARGO_MANIFEST_DIR` 为锚点生成绝对路径, 等价 TS `@/` 顶层锚定):

```rust
#[cfg(test)]
tests_bridge_macro::tests_bridge!("tools/dag_tools");  // 路径相对 crate 根, 不含 ../../
```

- **`#[cfg(test)]` 前缀必须保留**: 桥接宏是 dev-dependency, 普通 `cargo build`
  时不可用; `cfg(test)` 让非测试构建直接跳过展开。
- 参数是镜像路径 (**相对 crate 根**, 不含 `tests/` 前缀和 `/mod.rs` 后缀), 禁止 `..`。
- 镜像目录内的 `mod.rs` 声明子模块 (`mod plan;` 等), 每个子模块文件对应一个
  测试面。**禁止**在 src 下新建 `tests/` 目录。

> cargo 只把 `tests/` **顶层** `.rs` 自动编译为集成测试 target; 镜像子目录
> 下的文件不会成为独立 target (仅被宏生成的 `#[path]` 引用), 因此可以安全
> 存放依赖 `super::` 私有项的测试代码。

### e2e 引用被测代码: 优先 lib crate, include 仅限同 crate

- e2e 优先通过 **lib crate 路径**引用被测代码 (`theway_core::tools::dag_tools`),
  只用 pub API — 不要在 e2e 里 `#[path]` include 其他 crate 的源码。
- 仅在必须访问 `cfg(test)`-only 接口时 (如 `tests/commands.rs` 需要
  `clear_for_tests`) 才 `#[path]` include **同 crate** 源码。注意桥接宏的
  `CARGO_MANIFEST_DIR` 是**编译上下文 crate** — include 同 crate 源码路径正确,
  include 其他 crate 源码会生成错误路径 (这是 e2e 用 lib 路径的硬理由)。

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
- 测试总数以 **crate 级 lib target** 为准 (单测在 `cargo test -p <crate> --lib`
  中跑); e2e target 只含自身用例 — 不要用"总数不变"校验迁移, 而是核对每个
  target 的计数与迁移前一致 (e2e 曾因 include 源码重复跑单测, 已消除)。

## 6. 性能测试 (bench)

> bench 在整个测试体系中的定位与完整细则见 [testing.md §L4](testing.md)。
> 本章仅覆盖 bench 独有的**文件布局与命令规范**，其余 (分层模型/门禁矩阵/基线管理/防回归判据) 以上游 testing.md 为准。

### 位置

```
crates/<name>/benches/<object>.rs   # bench 源码
crates/<name>/Cargo.toml            # [dev-dependencies] criterion = "0.5"
                                    # [[bench]] name = "<object>"  harness = false
```

bench 文件与 `src/`、`tests/` 平级，不参与 1:1 镜像规则 — bench 是对架构级路径的性能验证，不与被测模块的目录结构耦合。

### 配置

每个 bench 文件必须在 crate 的 `Cargo.toml` 中声明：

```toml
[[bench]]
name = "dispatch"      # 对应 crates/<name>/benches/dispatch.rs
harness = false        # criterion 提供自己的 main(), 不使用 Rust 默认 bench harness
```

- `harness = false` **必须设置** — criterion 通过 `criterion_main!` 宏生成入口。
- criterion 放在 `[dev-dependencies]`，不进入生产构建。

### 命名

| 层级 | 规范 | 示例 |
|------|------|------|
| bench 文件 | `<被测对象>.rs` (snake_case) | `dispatch.rs` |
| bench 函数 | `bench_<被测对象>_<场景>` | `bench_emit_sync_only`、`bench_broadcast_10_receivers` |
| criterion group | 按文件聚合 | `benches`（含该文件所有 bench 函数，由 `criterion_group!` 声明） |

### 内容规范

1. **优先黑盒公开 API** — bench 通过 crate pub API 触发被测路径，不侵入私有实现。
2. **需复刻内部结构时注释标注来源** — 如 `// Replicates the three-segment dispatch from \`crate::agent::run_loop::utils::emit\`.`，确保后续维护者能追溯。
3. **每个 bench 函数文件头注释声明验证对象** — 硬约束 / 基线 / 对比项。

### 基线命令速查

```bash
# 保存基线
cargo bench -p theway-core --bench dispatch -- --save-baseline v0.1

# 对比基线
cargo bench -p theway-core --bench dispatch -- --load-baseline v0.1

# PR 编译验证 (不实际跑)
cargo bench -p theway-core --bench dispatch -- --test
```

基线存储在 `target/criterion/` (不提交到 Git)。

### CI 策略

| 触发 | 命令 | 目的 |
|------|------|------|
| PR | `cargo bench -p theway-core --bench dispatch -- --test` | 仅编译验证，确保 bench 代码不腐烂 |
| 手动 / 发布前 | `cargo bench -p theway-core --bench dispatch` | 完整跑，对比基线 |

> bench 编译验证已纳入 PR 门禁矩阵，见 [testing.md 门禁矩阵](testing.md)。
