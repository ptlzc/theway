# Design: controller-provisioned-catalogs

## Context

#95 已验证的 skill 供给链路（commit bb72028）：

```
TUI skill_scan ──scan──▶ WireDaemonConfig.skills ──reconcile diff──▶ Configure RPC
        ──daemon applier──▶ harness.replace_skills(builtins ⊕ provisioned) + provisioned_skills slot
        ──reload closure──▶ controller 模式读 slot 而非扫盘
```

本 change 把 templates 复制到同一条链路，并给两个目录字段补 proto twin。设计决策如下。

## Key Decisions

### 1. templates 与 skills 共用「body 随 wire 走」的供给形态

`WireProvisionedTemplate` 含 `content`（正文）而非仅路径。理由与 #95 相同：LLM 消费的是正文（`/template` 插值、prompt 注入），daemon 若只有路径就必须回退读盘——那会破坏「controller 模式下 daemon 零资源文件 IO」的边界。路径仅作元数据（展示与写回工具）。

### 2. TUI 模板扫描规则镜像 daemon 加载器（不递归 + project-over-user）

daemon `crate::templates::load_all` 的规则：`work_dir/.theway/templates` 与 `base/templates` 两个根、只收根级 `*.md`、同名 project 覆盖 user。TUI `template_scan` 原样镜像（顺序遍历两根，同名后者胜），frontmatter 只取 `name`（缺省用文件名 stem）与 `description`。与 skill_scan 不同：模板**不递归**、无 disable 标志——保持两个扫描器各自的镜像对象，不强行合并抽象。

### 3. seam 恢复覆盖 templates，但供给 slot 保证不丢

`SessionProjectResources::load(..., load_local_sources)` 对 templates 恢复 #95 之前的语义（controller 模式不扫盘）。与 #95 第一版问题的区别：这次 templates 有完整供给通道（wire + applier + slot），丢失窗口只在「daemon 启动后、Configure 到达前」——与 skills 现状一致（启动快照先回显空目录，第一个 Configure 补上）。独立 `thewayd` 不受影响（seam=true 走扫盘）。

### 4. reload 闭包保留供给目录（与 skills 同一机制）

controller 模式下 reload 闭包返回 slot 快照而非空盘扫结果；standalone 模式扫盘。`/reload`、`SetSkillDirs` 触发的重载因此不会抹掉 TUI 供给的 templates。

### 5. proto twin 用独立 message 而非 oneof/bytes

`settings.proto` 新增：

```proto
message ProvisionedSkill {
  string name = 1;
  string description = 2;
  string content = 3;
  string file_path = 4;
  string source = 5;             // "user" | "project"
  bool disable_model_invocation = 6;
}
message ProvisionedTemplate {
  string name = 1;
  string description = 2;
  string content = 3;
  string file_path = 4;
}
message DaemonConfig { /* ... */ repeated ProvisionedSkill skills = N; repeated ProvisionedTemplate templates = M; }
```

与 wire 字段一一对应、逐字段转换（`resources.rs`），不引入共享 bytes/JSON 载荷——twin 的既有风格就是显式字段。转换无数据丢失（round-trip 测试断言）。

### 6. Configure 应用顺序：templates 分支先于 builtin_skills 分支

skills 分支的既有顺序（provisioned 分支在前、builtin 在后）保证 builtin 重解析时能把非 builtin（含刚供给的）保留。templates 分支独立（harness template catalog 全量替换为供给目录，无 builtin 模板概念），放在 skills 分支之后、`skills_dirs` 分支之前，避免与任何共享字段交叉。

## Risks / Trade-offs

- **启动窗口**：controller 模式下首屏 `/template` 补全为空，直到 TUI 推送第一个 Configure。与 skills 相同的既有窗口，接受；不做启动阻塞握手（scope 膨胀）。
- **wire 尺寸**：skills + templates 正文都进 GetConfig 快照与 Configure 载荷。目录规模为几十 KB 级，可接受；不引入分页/摘要（YAGNI）。
- **proto 面与 serde 面的双维护**：twin 结构已有先例（provider/model 等字段），新增字段照旧；round-trip 测试兜底防漂移。

## Acceptance Criteria

1. TUI spawn 流程中，`/template <name>` 能命中项目与用户层模板（controller 模式，daemon 全程无模板文件读）。
2. daemon 单测：controller 模式 `load` 不扫盘；Configure 应用 templates；`clear_fields: ["templates"]` 清空；reload 闭包保留供给目录。
3. TUI 单测：template_scan 镜像规则（平铺、project-over-user、frontmatter）；assemble/reconcile 差量。
4. transport 单测：WireDaemonConfig merge/clear 含 templates；proto round-trip 含 skills/templates。
5. 独立 thewayd 回归：seam=true 时本地模板照常加载。
6. openspec validate --strict 通过；workspace 相关 crate 测试、clippy、fmt 全绿。
