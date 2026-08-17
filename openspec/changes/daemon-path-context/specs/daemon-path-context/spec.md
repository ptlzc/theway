# Capability: daemon-path-context

规范 daemon 的路径上下文来源、skill 根统一契约、session work_dir 绑定与
controller 透传语义。

## ADDED Requirements

### Requirement: Path context resolved once at the CLI boundary

daemon 的内容路径上下文（base / home / work_dir / 额外 skill 根）SHALL 在启动时由
`DaemonPaths` 一次性解析并显式传递；`THEWAY_DIR` / `HOME` 环境变量 SHALL 只在
CLI 边界解析函数内读取。同机 client↔daemon 的端口文件发现（`<base>/daemon-port-*`）
不受此约束（双方各自从进程 env 推导同一 `<base>` 的共享发现契约）。

#### Scenario: Kernel module lookup

- **WHEN** 开发者在 `crates/theway-daemon/src` 内（`paths.rs` 边界解析除外）查找
  `HOME` / `THEWAY_DIR` 的读取
- **THEN** 找不到；kernel 模块收到的都是显式传入的路径参数

#### Scenario: Controller-supplied home

- **WHEN** `thewayd` 以 `--home /data/home --skills-dir /a --skills-dir /b` 启动
- **THEN** `DaemonPaths.home = /data/home`，`extra_skill_dirs = [/a, /b]`（保持顺序），
  且 `base` = `$THEWAY_DIR`（若设置）否则 `/data/home/.theway`

### Requirement: Unified skill scan roots

skill 扫描根 SHALL 按以下优先级排序（先扫描者胜，同名 first-wins）：

1. `--skills-dir` extras（控制器显式，最高）
2. `<work_dir>/{.agents,.theway,.codex,.claude}/skills`（project）
3. `<base>/skills`（theway 原生 install 目标）
4. `<home>/{skills,.agents/skills,.codex/skills,.claude/skills}`（user）

`install_skill` / `skill_builder` / `remove_skill` 的目标目录 SHALL 为 `<base>/skills`
且该目录 SHALL 在扫描列表内。

#### Scenario: Installed skill is discoverable

- **WHEN** `/skills install` 把 `<name>` 写入 `<base>/skills/<name>/SKILL.md` 后 reload
- **THEN** `skills::load_all` 能发现该 skill（来源 User）

#### Scenario: Extra dir wins on collision

- **WHEN** `--skills-dir /extra` 与 project 根存在同名 skill
- **THEN** extra 根的版本生效（extras 优先级最高）

#### Scenario: Sandbox-only warns

- **WHEN** 以 `--no-default-features --features sandbox` 构建的 daemon 执行 skill 发现
- **THEN** 发出明确的 warn 日志并返回空目录，不静默表现为健康空目录

### Requirement: Session work_dir binding

`Session.cwd` 语义 SHALL 为该 session 的 work_dir（创建时继承 daemon work_dir；
wire 字段名不变）。切换 session 时 SHALL 校验目标 session 的 work_dir 与 daemon
work_dir 一致（canonicalize 后比较）；不一致 SHALL 拒绝并报告两边路径。目标 session
无 cwd 元数据（历史数据）SHALL 放行并记录 debug 日志。

#### Scenario: Cross work_dir switch rejected

- **WHEN** daemon（work_dir=/repo-a）尝试 switch 到 work_dir=/repo-b 的 session
- **THEN** 返回错误，错误文案包含目标 session 的 work_dir 与当前 daemon 的 work_dir，
  当前绑定不变

#### Scenario: Legacy session without cwd passes

- **WHEN** 目标 session 元数据缺失 cwd（旧版本创建）
- **THEN** switch 正常进行，并记录 debug 日志

### Requirement: Controller passthrough

TUI（controller）SHALL 支持把 `--home` 与可重复的 `--skills-dir` 透传给它拉起的
daemon；attach 已运行 daemon 时 SHALL 不影响该 daemon 既有的路径上下文。

#### Scenario: Spawn forwards flags

- **WHEN** TUI 以 `--home /data/home --skills-dir /extra` 拉起 `thewayd`
- **THEN** spawn 参数包含逐项转发的 `--home` / `--skills-dir`

#### Scenario: Attach leaves context intact

- **WHEN** TUI attach 到已运行的 daemon（cwd 键控发现命中）
- **THEN** 该 daemon 启动时的路径上下文保持不变
