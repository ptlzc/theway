# Tasks: daemon-path-context

DAG: `issue-66-daemon-path-context`（dag-2）。节点 ID 与编排一致；
并行节点文件不相交（2/3 并行：skills 族 vs session_factory 族）。

## 1. [1-paths-context] DaemonPaths 上下文与 env 收口

- [x] 1.1 新建 `crates/theway-daemon/src/paths.rs`：`DaemonPaths { base, home, work_dir,
  extra_skill_dirs }` + `from_cli`（环境变量只在此边界读）+ `skills_root()` + 优先级单测。
- [x] 1.2 `bin/thewayd.rs`：Cli 增 `--home` / `--skills-dir`（repeatable）；启动构建
  `paths` 并线程化 config readers / skill_overrides / 组装点。
- [x] 1.3 `file_commands.rs` / `ts_extensions/mod.rs` / `tools/{set_skill_state,remove_skill,install_skill,skill_builder}`
  显式参数化；`new()` 默认构造器保留给测试。
- [x] 1.4 验收：`cargo check -p theway-daemon --all-targets`（local 与 sandbox-only）+
  单测；grep 确认 kernel 无新增 env 读取。

## 2. [depends: 1-paths-context] [2-skills-unify] 统一 skill 根

- [x] 2.1 `skills.rs`：`skills_dirs(&DaemonPaths)` 按 extras > project > `base/skills`
  > home 根排序；删 `user_config_root()` env 读取；`load_all(&DaemonPaths)`（含 sandbox stub）。
- [x] 2.2 `bin/thewayd.rs` 启动与 reload 闭包改用捕获的 `paths`。
- [x] 2.3 `SkillSource` / `builtin_skills.rs` 注释与实现口径对齐（无枚举变更）。
- [x] 2.4 验收：`cargo test -p theway-daemon`（local）+ sandbox-only 编译；新增用例
  （base/skills 可发现 / extra 优先 / base 胜 home）。

## 3. [depends: 1-paths-context] [3-session-workdir] session work_dir 绑定

- [x] 3.1 `SessionHarnessFactory::build`：目标 session work_dir 与 `self.cwd`
  canonicalize 比较；不一致报含路径错误；无 cwd 元数据放行 + debug 日志。
- [x] 3.2 `Session.cwd` 语义注释（work_dir，创建时继承 daemon work_dir）；
  `session_ops.rs` 注释点明继承。
- [x] 3.3 验收：session_factory 镜像测试 —— 跨 work_dir 拒绝、同 work_dir 成功、旧数据放行。

## 4. [depends: 2-skills-unify, 3-session-workdir] [4-tui-plumb] TUI 透传

- [x] 4.1 TUI Cli 增 `--home`（Option）/ `--skills-dir`（repeatable）；
  帮助文本说明仅拉起 daemon 时生效。
- [x] 4.2 `daemon_launch_args` 转发两个旗标 + 序列断言；离线 session 路径不受影响。
- [x] 4.3 验收：`cargo check/test -p theway-tui --all-targets`。

## 5. [depends: 4-tui-plumb] [5-docs] 文档同步

- [x] 5.1 `docs/architecture.md`：路径上下文小节（边界解析、根优先级表、work_dir 校验、
  端口发现 env 例外）。
- [x] 5.2 `README.md` 简述并引用 architecture.md。

## 6. [depends: 5-docs] [6-verify] 终态验收

- [x] 6.1 `cargo fmt/check/clippy/test --workspace` + feature-gate 矩阵。
- [x] 6.2 grep：daemon/src 内 env 读取仅 `paths.rs`。
- [x] 6.3 issue #66 验收项逐条复核（基于最新 HEAD，不信节点自报）。
