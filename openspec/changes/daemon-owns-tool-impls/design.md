# Design: daemon-owns-tool-impls

## Context

历史偏差两条(均已归档):

- `tools-into-core`:把 26 个工具文件从 server 迁入 core,理由是"引擎自包含"。本次变更**逆转该决策**:引擎自包含的边界收窄为 *interface + runtime*(trait + 消费注入的 `Vec<Arc<dyn AgentTool>>`),工具本体属于产品组装层(daemon)。
- `sdk-split-local-sandbox`:把 executor 的 *trait* 放 core(正确,保留)、*参考实现* 放 sdk(本次修正:实现随 daemon)。

现状证据(core 内对 `src/tools/` 的引用为 0,harness 以 `Vec<Arc<dyn AgentTool>>` 注入消费;sdk/tui/transport 不引用 `theway_core::tools`;sdk 生产代码不引用 LocalExecutor——只有 daemon 用)。因此移动是机械的,不涉及行为改造。

## Goals / Non-Goals

- **Goals**:三层职责归位;daemon 双 feature(local/sandbox)可注入切换;executor/进程原语/锁在 daemon 单点维护。
- **Non-Goals**:sandbox gRPC executor 实装;工具语义变更;core runtime 自身移动。

## Decisions

### 1. 工具本体归属 daemon(逆转 tools-into-core)

core 保留 `AgentTool` trait 族与 executor/`ExecutionEnv` trait;`src/tools/**` 全部迁入 daemon。daemon 内的装配(`default_tools`/`session_tool_set`/`subagent_tool_sets`)与工具本体同层,不再跨 crate 引用 core 装配函数。core 的 harness 只面向 trait 与注入集合。

*理由*:core=interface+引擎 runtime;工具是产品能力,依赖 daemon 环境(local fs / sandbox gRPC),引擎不得反向依赖产品层。

### 2. executor 实现完全搬出 sdk

`LocalExecutor`(含 `atomic_write`)、`SandboxExecutor`(stub)、`FileLock` 从 sdk 迁入 daemon;`ToolExecutor` trait 留在 core(维持 core 自包含、wasm/嵌入消费者可自实现)。sdk 生产代码零引用(已核实),迁移无客户端面影响。

*理由*:executor 是 fs 访问策略 = daemon 的运行时资产;sdk 是客户端契约,不应携带运行时实现。

### 3. daemon local/sandbox 双 feature(同一 crate)

- `local`(default):直触 fs + 进程表(LocalExecutor + NativeEnv)
- `sandbox`:注入 sandbox executor(stub,`ExecutorKind::Sandbox` 显式报错);未来接 gRPC 实现

两个 feature 共享同一套工具本体与装配;组合根按 feature 选择 executor。`--no-default-features --features sandbox` 必须可编译(特性矩阵进入 CI 验证)。

### 4. NativeEnv 实现与 exec.rs 进程原语迁入 daemon

`agent/env/native.rs`(setsid/killpg 整树 kill 语义)从 core 迁入 daemon,挂 `local` feature;core 移除 `native-env` feature,只留 `ExecutionEnv` trait。daemon 内 `templates.rs`/`skills.rs` 的 `NativeEnv::new` 注入点改为 `crate::` 路径。

*理由*:native env 是实现类代码,属 core=interface-only 原则的清理范围;其进程原语与 bash 工具、exec_shell 家族共享,daemon 内单点维护,避免三处复制整树 kill 语义。

### 5. sdk 三文件夹语义固化

- `common/`:客户端共享契约(wire/session/config/feed/commands 框架)—— 不变
- `local/`:客户端本地辅助(auth store 路径、history、images、mentions、session repo 包装、离线命令)—— 删除 executor 后余部不变
- `sandbox/`:沙箱客户端契约占位(删除 executor stub 后,保留模块与契约注释,未来放 gRPC 客户端类型)

### 6. 测试随实现迁移

工具测试(tests/tools/*、tests_bridge 镜像)随本体迁 daemon;`tests/executor.rs` 随 LocalExecutor 迁 daemon;FileLock 内联测试随迁。core 只保留 trait/harness 测试(不含已迁工具的用例)。

### 7. 依赖收敛(core 瘦身)

迁移完成后逐项验证 core 的 `reqwest`/`tree-sitter`/`theway-mcp` 等是否只被已迁出的工具使用,是则从 core `Cargo.toml` 移除,由 daemon 承接;`Cargo.lock` 不新增包(全部为既存依赖)。

## Risks / Mitigations

- **大范围路径适配**(`theway_core::tools::` → `crate::tools::` 数十处):按模块分批迁移,每批 workspace 编译绿后再下一批(见 tasks.md)。
- **core 公共 API 破坏**:仓库内消费者只有 daemon(已核实);对外以 commit 说明 + CHANGELOG 标注为准。
- **特性矩阵回归**:新增 `--no-default-features --features sandbox` 编译检查任务,防 feature 拆分漏门控。
- **测试迁移遗漏**:迁移每批同步移动 tests_bridge 镜像目录,终态以 `cargo test --workspace` 全绿兜底。

## Migration order(保持每步可编译)

1. daemon feature 骨架(local/sandbox,无行为变化)
2. sdk executor/FileLock → daemon(引用适配 + tests/executor.rs 迁移)
3. core 工具族 → daemon(assembly/subagent/dag_tools → skill 族/memory/mcp_adapter → exec/exec_shell),core lib.rs 清理
4. agent/env/native.rs → daemon,core 移除 native-env feature
5. sdk 目录清理 + 三夹语义文档
6. 依赖收敛 + 特性矩阵 + 全量验证
