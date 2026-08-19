# testing.md — 测试体系总纲

仓库完整测试体系分层模型、门禁矩阵与 bench 规范。**本文件是测试体系的入口总纲**，
与 [rust-test-files.md](rust-test-files.md)（文件布局与命名细则）互补，双向引用。

## 分层模型（5 层金字塔）

```
                    ┌──────────┐
                    │ L4 bench │ ← 分钟级, 架构假设+防回归
                    └┬────────┬┘
                     │        │
              ┌──────┴─┐  ┌──┴──────────┐
              │ L3 E2E │  │ L2 集成测试   │ ← 秒~分钟级, 独立二进制/进程内 lib pub API
              └───┬───┘  └──┬──────────┘
                  │         │
             ┌────┴─────────┴────┐
             │   L1 单元测试      │ ← 毫秒~秒级, src 内联 + tests/ 镜像桥接 (私有访问)
             └────────┬──────────┘
                      │
             ┌────────┴──────────┐
             │   L0 规范校验      │ ← 毫秒级, 格式/文档/feature 门控
             └───────────────────┘
```

---

### L0 — 规范校验（毫秒级）

| 维度 | 内容 |
|------|------|
| **目标** | 代码格式一致、feature 组合可编译、OpenSpec 文档变更合规 |
| **位置** | 不涉及测试文件 — 检查源码与 Cargo.toml |
| **命令** | `cargo fmt --all --check` · `cargo clippy --workspace --all-targets -- -D warnings` · `cargo check -p theway-core --no-default-features` · `openspec validate <change>` |
| **速度量级** | 毫秒~秒（clippy 略慢，但仍属秒级） |
| **CI 门禁** | **PR 必过**。任一失败阻止合并 |
| **失败责任** | 提交者修复。fmt 差异一条命令自动修复；clippy warning 按规则消除；feature 门控失败需检查条件编译 `#[cfg]` 与 default-features 声明 |

---

### L1 — 单元测试（毫秒~秒级）

| 维度 | 内容 |
|------|------|
| **目标** | 验证单个函数/方法的逻辑正确性，可访问被测模块私有项 |
| **位置** | (a) `src/` 内联 `#[cfg(test)] mod tests { }` — 轻量断言，贴近被测代码；(b) `tests/<镜像路径>/` 多文件模块 — 通过 `#[path]` 桥接以单元语义编译（可调用 `super::` 私有项）。详见 [rust-test-files.md §1-2](rust-test-files.md) |
| **命令** | `cargo test --workspace --lib`（仅 lib target）；`cargo test --workspace` 也包含 |
| **速度量级** | 毫秒~秒（无 I/O、无网络） |
| **CI 门禁** | **PR 必过**。所有单测必须 GREEN |
| **失败责任** | 提交者修复。新功能必须附带对应单测；修改行为需同步更新用例。旧测试按 [rust-test-files.md](rust-test-files.md) 规范逐步对齐 |

---

### L2 — 集成测试（秒级）

| 维度 | 内容 |
|------|------|
| **目标** | 验证模块间交互、public API 契约、多组件协同 |
| **位置** | `tests/<镜像路径>/` 内，通过 `use theway_core::...` 引用 pub API（不可访问私有项）。与 L1 共享镜像目录，但编译为独立的集成测试 target |
| **命令** | `cargo test --workspace`（lib + 所有集成 target） |
| **速度量级** | 秒级（进程内调用，无跨进程/网络开销） |
| **CI 门禁** | **PR 必过** |
| **失败责任** | 提交者修复。集成测试覆盖模块边界，失败通常意味着 pub API 不兼容或模块契约破损 — 优先回看 API 变更是否故意，是则同步更新测试 |

---

### L3 — E2E 测试（秒~分钟级）

| 维度 | 内容 |
|------|------|
| **目标** | 端到端验证 CLI 行为、传输协议、跨进程交互（真实外部依赖 mock） |
| **位置** | `tests/` 顶层独立二进制文件（如 `tests/cli.rs`、`tests/commands.rs`）；只能访问 pub API |
| **命令** | `cargo test --workspace`（独立 e2e target 随 `--workspace` 运行） |
| **速度量级** | 秒~分钟（可能启动子进程、绑定端口、模拟网络往返） |
| **CI 门禁** | **PR 必过**。e2e 用例需要稳定可重复 — 使用 port 0 自动分配、temp dir、mock 外部服务 |
| **失败责任** | 提交者修复。e2e 失败可能暴露集成测试未覆盖的真实交互问题；与环境相关的失败（网络/端口）需加 `#[ignore]` 或 feature gate 并记录原因 |

---

### L4 — 性能测试 / bench（分钟级）🆕

| 维度 | 内容 |
|------|------|
| **目标** | 验证架构硬约束（如同步回调 <1µs）、关键路径基线、防性能回归 |
| **位置** | `crates/<name>/benches/<object>.rs`，`Cargo.toml` 声明 `[[bench]] name = "..." harness = false` |
| **命令** | PR 编译验证: `cargo bench -p <crate> --bench <name> -- --test`（只编译不跑）；完整跑: `cargo bench -p <crate> --bench <name>` |
| **速度量级** | 分钟级（criterion 多次采样与统计分析） |
| **CI 门禁** | **PR 编译验证**（`--test`，确保 bench 可编译）；**完整跑不进 PR**（分钟级），手动触发或发布前跑 |
| **失败责任** | bench 编译失败 → 提交者修复。完整跑后关键 bench 均值超基线 N% 或超硬约束 → **架构审查**（不盲目接受"优化"或"回退"），由 reviewer 与提交者共同决策 |

#### bench 层细则（L4）

**位置与配置**：

```
crates/<name>/benches/<object>.rs   # bench 源码
crates/<name>/Cargo.toml            # [dev-dependencies] criterion = "0.5"
                                    # [[bench]] name = "<object>"  harness = false
```

- `harness = false` 必须设置 — criterion 提供自己的 `main()`，不使用 Rust 默认 bench harness。
- criterion 是 dev-dependency，不影响生产构建。

**命名规范**：

| 层级 | 规范 | 示例 |
|------|------|------|
| bench 文件 | `<被测对象>.rs` | `dispatch.rs` — 事件分发架构 bench |
| bench 函数 | `bench_<被测对象>_<场景>` | `bench_emit_sync_only`、`bench_broadcast_10_receivers` |
| criterion group | 按文件聚合 | `benches`（含该文件所有 bench 函数） |

**内容规范**：

1. **优先黑盒公开 API**：bench 应通过 crate pub API 触发被测路径（如 `registry_emit` 通过 `AgentJobRegistry::register` + `finish` 触发内部 emit），不侵入私有实现。
2. **需复刻内部结构时标注来源**：当 pub API 无法精确隔离被测段时（如 `emit_three_segment` 需要拆分 sync/await/broadcast 三段），在注释中标注复刻来源：
   ```rust
   // Replicates the three-segment dispatch from `crate::agent::run_loop::utils::emit`.
   ```
3. **每个 bench 函数文件头注释声明验证对象**（硬约束 / 基线 / 对比项）。

**验证对象分类**：

| 类型 | 说明 | 示例 |
|------|------|------|
| 架构硬约束 | 绝对值不可逾越 | `emit_sync_only` — 同步回调 <1µs |
| 关键路径基线 | 建立历史基线防回归 | `emit_three_segment` — 三段分发总耗时 |
| 架构对比 | 新旧范式对比验证 | `emit_three_segment` vs `emit_legacy_for_await` |
| 扩展性 | 参数缩放验证 | `broadcast_1_receiver` vs `broadcast_10_receivers` |

**基线管理**：

```bash
# 首次建立基线
cargo bench -p theway-core --bench dispatch -- --save-baseline v0.1

# 后续对比
cargo bench -p theway-core --bench dispatch -- --load-baseline v0.1

# 列出已有基线
ls target/criterion/
```

基线存储在 `target/criterion/`（不提交到 Git）。

**防回归判据**：

- 关键 bench 均值超出基线 **N%**（如 20%，由 reviewer 根据变更范围判定）→ 触发架构审查，不自动接受。
- 硬约束 bench（如 `emit_sync_only`）**超过预设绝对值** → 阻止合并，必须回退或重新设计。

**CI 策略**：

| 触发 | 命令 | 目的 |
|------|------|------|
| PR | `cargo bench -p theway-core --bench dispatch -- --test` | 仅编译验证，确保 bench 代码不腐烂 |
| 手动 / 发布前 | `cargo bench -p theway-core --bench dispatch` | 完整跑，对比基线 |

---

## 门禁矩阵（PR 必过）

| # | 检查 | 命令 | 层 | 说明 |
|---|------|------|----|------|
| 1 | rustfmt | `cargo fmt --all --check` | L0 | 代码格式一致，差异 `cargo fmt --all` 修复 |
| 2 | clippy | `cargo clippy --workspace --all-targets -- -D warnings` | L0 | lint 全部 target（lib/bin/test/bench） |
| 3 | feature 门控 | `make feature-gate` | L0 | bare core/provider、harness、Bedrock、全 provider 与 daemon backend 矩阵均可编译 |
| 4 | 单元+集成 | `cargo test --workspace` | L1+L2 | 所有 lib + 集成 target 全 GREEN |
| 5 | E2E | `cargo test --workspace` | L3 | e2e target 随 `--workspace` 运行 |
| 6 | bench 编译 | `cargo bench -p theway-core --bench dispatch -- --test` | L4 | bench 可编译，不实际跑 |
| 7 | OpenSpec | `openspec validate <change>` (本地) | L0 | 变更文档合规（有 openspec change 时） |

> 完整 CI 检查命令（本地 pre-push 推荐）：
> ```bash
> cargo fmt --all --check \
>   && cargo clippy --workspace --all-targets -- -D warnings \
>   && make feature-gate \
>   && cargo test --workspace --no-fail-fast \
>   && cargo bench -p theway-core --bench dispatch -- --test
> ```

---

## 与 rust-test-files.md 的关系

| 维度 | testing.md（本文件） | [rust-test-files.md](rust-test-files.md) |
|------|----------------------|------------------------------------------|
| 定位 | **体系总纲** — 分层模型、门禁矩阵、bench 规范 | **文件布局与命名细则** — 目录分离、镜像、命名三段式、AAA |
| bench | L4 完整细则（目标/命名/基线/CI） | §6 位置/命名/harness 配置/命令速查（指向本文件 L4） |
| 引用 | 多处引用 rust-test-files.md 具体规范 | §6 引用本文件 L4 细则 |

**阅读路径**：新贡献者先读本文件理解测试分层与门禁 → 写测试时查 [rust-test-files.md](rust-test-files.md) 按文件布局与命名规范落代码。
