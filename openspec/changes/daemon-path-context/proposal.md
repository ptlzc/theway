# Proposal: daemon-path-context

## Why

daemon 当前从**调用时刻的进程环境**推导路径上下文，而不是启动参数：

1. **kernel 内散读环境变量**：`theway_contract::config::base_dir()` 每次调用读
   `THEWAY_DIR`/`HOME`（config readers、skills install 根、skill overrides、
   ts_extensions、set_skill_state 都在用）；`skills::user_config_root()` 读 `$HOME`；
   `file_commands.rs` 读 `$HOME`。daemon 行为依赖任意调用时刻的环境。
2. **skill install/load 根不一致**（#37 多目录扫描遗留回归）：`/skills install`、
   `skill_builder`、`remove_skill` 写 `<base>/skills`（默认 `~/.theway/skills`），但 loader
   扫 `$HOME/skills`、`$HOME/.agents/skills`、`$HOME/.codex/skills`、`$HOME/.claude/skills`
   —— **install 目标不在扫描列表里**，装完即隐身。
3. **cwd 是 daemon 级 ambient 状态，session 绑定隐式**：`Session.cwd` 仅是元数据，
   `switch_session` 不做任何校验，没有显式的 session work_dir 概念。
4. **controller 无法传入扫描目录**：`spawn_daemon` 只透传 `--cwd`。

## What Changes

- **`DaemonPaths` 上下文，启动时构建一次**：`thewayd` 新增 `--home <dir>` 与
  `--skills-dir <dir>`（可重复）旗标；`DaemonPaths { base, home, work_dir, extra_skill_dirs }`
  在 CLI 边界一次性解析（环境变量只在此边界读），显式线程化到 kernel 各模块。
  端口文件发现（`<base>/daemon-port-<fnv64(cwd)>`）保持 env 契约 —— 它是同机
  client/daemon 共享的发现机制，不属于内容路径。
- **统一 skill 根**（修复不一致）：`skills_dirs(paths)` 优先级 =
  `--skills-dir` extras > `<work_dir>/{.agents,.theway,.codex,.claude}/skills`（project）
  > `<base>/skills`（theway 原生 install 目标，**现在纳入扫描**） > `<home>` 下四个 user 根。
  first-wins 去重不变；install/builder/remove 继续写 `<base>/skills`。
- **session work_dir 绑定**：`Session.cwd` 语义明确为 session 的 work_dir（wire 字段名不变）；
  `SessionHarnessFactory::build` 打开目标 session 后校验其 work_dir 与 daemon work_dir
  一致（canonicalize 后比较），不一致拒绝并报两边路径；无 cwd 元数据的旧 session 放行（兼容）。
- **TUI 透传**：TUI CLI 增加 `--home` / `--skills-dir`（可重复），`daemon_launch_args`
  逐项转发给被拉起的 `thewayd`；attach 已运行 daemon 的路径不受影响（cwd 键控发现不变）。

## Capabilities

### New Capabilities

- `daemon-path-context`: 显式路径上下文（base/home/work_dir/extra skill dirs）、
  统一 skill 根优先级、session work_dir 绑定与 switch 校验、controller 透传。

### Modified Capabilities

- 无。`layering` 既有场景（含 "Sandbox skill loading warns"）语义保持 —— sandbox stub
  仍显式 warn，仅签名随 `DaemonPaths` 调整。

## Impact

- **daemon**：新增 `src/paths.rs`；`bin/thewayd.rs`（旗标 + 组装）、`skills.rs`（签名
  `load_all(paths)`）、`file_commands.rs` / `ts_extensions/mod.rs`（显式参数）、
  `tools/{install_skill,skill_builder,remove_skill,set_skill_state}`（显式 base/root）、
  `turn/session_factory.rs`（work_dir 校验）。
- **core**：`SkillSource` 文档措辞、`Session.cwd` 语义注释（无枚举/wire 变更）。
- **tui**：`cli/mod.rs`（`--home`/`--skills-dir`）、`startup/mod.rs`（`daemon_launch_args`）。
- **不变**：`theway_contract::config::base_dir()` 本体（端口发现/inbox 的 env 契约）、
  transport wire 字段、`local|sandbox` feature 语义与 fail-closed 门控。
