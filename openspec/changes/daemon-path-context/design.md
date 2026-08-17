# Design: daemon-path-context

## D1. 边界解析，一次成型

`DaemonPaths::from_cli(cwd, home, extra_skill_dirs)`：

- `base` = `$THEWAY_DIR`，否则 `home/.theway` —— 环境变量**只在 from_cli 里读一次**。
- `home` = 显式参数，否则 `$HOME`。
- `work_dir` = 显式 `--cwd`，否则 `std::env::current_dir()`（canonicalize 失败保留原值）。
- `extra_skill_dirs` = `--skills-dir` 可重复收集，保持 CLI 顺序。

kernel 模块（skills / file_commands / ts_extensions / install·builder·remove·set_skill_state /
skill_overrides 调用方 / reload 闭包）全部改为收显式参数或 `&DaemonPaths`；`new()` 默认构造器
仅测试使用（默认根不变）。**例外**：`client::port_file_path` / `inbox` 的 env 读取是同机
client↔daemon 发现契约，双方从各自进程 env 推导同一 `<base>`，与内容路径无关，保持不动。

## D2. skill 根优先级（install 与 load 对齐）

```
1. paths.extra_skill_dirs        --skills-dir 逐项（控制器显式，最高）
2. work_dir/{.agents,.theway,.codex,.claude}/skills   Project（#37 顺序不变）
3. base/skills                    theway 原生 install 目标（User，新入列）
4. home/{skills,.agents/skills,.codex/skills,.claude/skills}   User
```

first-wins：先扫描者胜。`base/skills` 排在其余 user 根之前 —— install 目标永远可被发现；
`home/skills` 与 `base/skills` 同名冲突时 install 目标（3）胜 `home`（4）。sandbox-only stub
保持显式 warn（layering spec 场景不变），仅签名改为 `load_all(&DaemonPaths)`。

## D3. session work_dir 校验

`SessionHarnessFactory::build(session_id)` 打开目标 session 后：

- 双方 `std::fs::canonicalize`，失败回退原路径字符串比较；
- 不一致 → 错误含两边路径："session <id> belongs to work_dir <target>; this daemon
  serves <current> — start theway from that directory"；
- 目标 session 元数据无 cwd（旧数据）→ 放行 + `tracing::debug`（不能锁死历史 session）；
- `create` 继续盖 daemon work_dir 章（现状即继承语义，注释点明）。

wire 字段 `cwd` 名称不变（兼容），语义 = session 的 work_dir。

## D4. TUI 只在"拉起"时透传

`daemon_launch_args` 追加 `--home`（有才传）与逐项 `--skills-dir`。attach 已运行 daemon
（cwd 键控 port file 发现）不传也不改其配置 —— daemon 的路径上下文是**启动期定死**的，
这正是本 change 的语义。

## D5. 测试策略

- `paths.rs` 单测：解析优先级（THEWAY_DIR 覆盖派生 / 显式 home 覆盖 $HOME / cwd fallback）。
- `cli_skills.rs`：`base/skills` 被发现且 User；extra 根同名战胜 project 根；
  `base/skills` 与 `home/skills` 同名时前者胜。
- session_factory（tests bridge 镜像）：跨 work_dir 拒绝（文案含目标路径）；旧 session 放行。
- `daemon_launch_args`：透传序列断言。
- grep 验收：daemon/src 内 `var_os("HOME")` / `var("THEWAY_DIR")` 仅允许 `paths.rs`。
