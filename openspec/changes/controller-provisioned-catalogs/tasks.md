# 实施任务

> 对应 GitHub issue：#96（父）。每项完成一个 commit（Conventional Commits + issue 引用）。
> 编排：节点按 DAG 声明依赖；文件不相交的节点并行（theway-tui / theway-transport / theway-daemon 各归各的 crate）。
> agent 映射：executor = executor-coder，verify = checker，writer = executor-writer。

## 1-template-scanner（theway-tui）

- [ ] 1.1 新增 `crates/theway-tui/src/template_scan.rs`：镜像 daemon `crate::templates` 规则——根 `work_dir/.theway/templates` 与 `base/templates`、只收根级 `*.md`、不递归、同名 project 覆盖 user；frontmatter name（缺省 = 文件 stem）/description；产出 `WireProvisionedTemplate`。
- [ ] 1.2 单元测试：平铺命中、project 覆盖 user、非 md 跳过、缺 description 跳过、frontmatter 缺 name 用 stem。

## 2-wire-catalogs（theway-transport）

- [ ] 2.1 `wire/commands.rs`：新增 `WireProvisionedTemplate { name, description, content, file_path }`；`WireDaemonConfig.templates: Vec<WireProvisionedTemplate>`；`FIELDS` 加 `"templates"`；`merge_from` / clear 分支同步。
- [ ] 2.2 `proto/resources.rs`：`daemon_config_to_proto` / `daemon_config_from_proto` 对 skills/templates 逐字段转换（skills 为已落地字段，本次补 templates 转换；skills 的 to_proto 空缺一并补齐或显式标注）。
- [ ] 2.3 wire 测试：merge/clear 含 templates；proto round-trip 含 skills/templates 全字段。

## 3-daemon-templates（theway-daemon）

- [ ] 3.1 `orchestration/session/resources.rs`：seam 重新覆盖 templates（controller 模式不加载本地模板）；新增 `provisioned_templates` slot（`Arc<RwLock<Vec<PromptTemplate>>>`）；reload 闭包 controller 模式返回 slot 快照。
- [ ] 3.2 `turn/daemon.rs` + `runtime.rs`：`DaemonConfig` / `RuntimeConfiguration` 增加 `provisioned_templates` 字段；启动 `WireDaemonConfig` 种子回显 slot。
- [ ] 3.3 `turn/daemon/commands.rs`：`Configure` 新增 templates 分支（wire → `PromptTemplate` 映射 → harness template catalog 替换 + slot 写 + 配置视图回显；clear 语义）。
- [ ] 3.4 测试：controller 模式 load 不扫盘 + reload 保留 slot；Configure 应用 / 清除 templates；standalone 回归（本地模板照常加载）。

## 4-tui-wiring（theway-tui）

- [ ] 4.1 `config_payload.rs`：`assemble_config_from` 扫描模板（template_scan）填入 `payload.templates`；`reconcile` 增加 templates 差量（与 skills 同形态）。
- [ ] 4.2 测试：assemble 产出模板目录；reconcile 只在目录变化时推送、相等时跳过。
- [ ] [depends: 1-template-scanner, 2-wire-catalogs]

## 5-proto-parity（theway-transport / theway-contract）

- [ ] 5.1 `settings.proto`：新增 `ProvisionedSkill` / `ProvisionedTemplate` message，`DaemonConfig` 增加 `repeated ProvisionedSkill skills` / `repeated ProvisionedTemplate templates`。
- [ ] 5.2 生成代码（repo 的 proto 生成流程）+ `resources.rs` 双向转换接入新字段。
- [ ] 5.3 round-trip 测试覆盖 skills/templates（含 source、disable_model_invocation）。
- [ ] [depends: 2-wire-catalogs]

## 6-docs-closeout（文档与验证）

- [ ] 6.1 `docs/architecture.md` 同步 controller 供给边界（skills/templates 两目录均 controller 扫描供给，daemon controller 模式零资源文件 IO）。
- [ ] 6.2 `openspec validate --strict`；workspace 相关 crate 测试 + clippy + fmt 全绿；按 design.md 验收标准逐条核对。
- [ ] [depends: 3-daemon-templates, 4-tui-wiring, 5-proto-parity]
