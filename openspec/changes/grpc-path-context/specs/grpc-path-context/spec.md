# Capability: grpc-path-context

规范 gRPC 暴露 daemon 路径上下文与运行中动态更新 skill 扫描根。

## ADDED Requirements

### Requirement: Path context is readable over gRPC

`SessionService` SHALL 提供 `GetPathContext(Empty) returns (PathContext)`。
`PathContext` SHALL 包含 `home`、`base`、`work_dir` 与 `skills_dirs`（当前生效的额外 skill
扫描根，有序）。`home` / `base` / `work_dir` 来自 daemon 启动时的 `DaemonPaths`，运行中不变。

#### Scenario: Read startup context

- **WHEN** 客户端调用 `GetPathContext`
- **THEN** 返回 daemon 启动时的 home/base/work_dir 与当前 skills_dirs

#### Scenario: Read-only home and work_dir

- **WHEN** 客户端尝试修改 home / work_dir / base（协议层无对应 setter）
- **THEN** 这些值保持不变，仅有 `skills_dirs` 可通过 `SetSkillDirs` 修改

### Requirement: Skill dirs can be updated at runtime

`SessionService` SHALL 提供 `SetSkillDirs(SetSkillDirsRequest{repeated string dirs})
returns (CommandResult)`。daemon SHALL 将请求的 dirs 写为新的额外 skill 扫描根，并触发
skill catalog 热重载；随后 `GetPathContext.skills_dirs` SHALL 反映新值。

#### Scenario: Update and reload

- **WHEN** 客户端调用 `SetSkillDirs{dirs: ["/extra/a", "/extra/b"]}`
- **THEN** 返回 accepted
- AND daemon 的额外扫描根变为 `/extra/a`、`/extra/b`（保持顺序）
- AND skill catalog 热重载，新目录中的 skill 可被发现
- AND 后续 `GetPathContext` 返回 `skills_dirs = ["/extra/a", "/extra/b"]`

#### Scenario: Serialized application

- **WHEN** SetSkillDirs 与其它控制命令同时到达
- **THEN** 通过事件循环串行应用，避免与进行中的 turn 竞态；若 turn 正在运行则先中止

### Requirement: Shared mutable extras preserve scan priority

`DaemonPaths.extra_skill_dirs` SHALL 以共享可变状态保存，`skills::skills_dirs` SHALL 每次
读取当前值。动态更新后，extras 仍保持最高扫描优先级（先扫描者胜）。

#### Scenario: Dynamic extras beat project roots

- **WHEN** 先以 project 根加载同名 skill，再 `SetSkillDirs` 加入含同名 skill 的 extra 目录并 reload
- **THEN** 新 extra 目录中的同名 skill 生效（extras 优先级最高）
