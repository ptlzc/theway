# Crate 双语文档

[English](README.md) | 中文

Crate 文档语料以英文为默认源，以简体中文作为经评审的辅助译文。本文定义文件配对约定；[translation-rules.md](translation-rules.md)定义翻译忠实度与结构要求，[terminology.md](terminology.md)定义共享术语。

## 配对约定

- 一组文档配对由 3 个同目录文件组成：英文 `foo.md`、中文 `foo.zh.md` 和 `foo.i18n.yaml`。
- 无后缀英文文件是默认入口与真源。中文文件必须表达相同的当前机制，不得增加或遗漏行为。
- 两个 Markdown 文件都在 H1 标题之后放置语言切换行。两侧的普通链接都指向无后缀英文路径；只有语言切换行链接到 `.zh.md`。Crate 文档中的仓库相对链接必须留在归属 crate 内。
- 伴随记录保存两侧文件在上次评审确认同步时的 Git blob hash。只修改任一侧而未同步另一侧并重新记录配对，会导致 `make doc-sync` 失败。
- 标题层级、列表形状、表格形状、链接目标和围栏代码块在结构上保持一致。代码块必须逐字节相同。

## 范围

本约定覆盖每个工作区成员的 `README.md`、必需的 `docs/architecture.md`、各 crate 的 `docs/` 目录中的其他 Markdown，以及本目录中的配对政策文档。

- 根目录与 crate 的 `AGENTS.md` 是仅使用英文的 agent 指令。
- 本政策不覆盖根目录中其他产品文档和贡献者文档。

## 更新流程

1. 修改英文源文件，并在同一变更中对中文文件做最小的对应更新。
2. 按照 [translation-rules.md](translation-rules.md)保留代码 span、代码块、命令、路径、API 名称、链接目标、列表形状和表格形状。
3. 确认两侧表达相同内容，然后运行 `scripts/verify-doc-i18n.py --write <source.md>`。
4. 运行 `make doc-sync`；不得手工修改 `*.i18n.yaml` 中的 hash。

## 验证

```bash
scripts/verify-doc-i18n.py --list
scripts/verify-doc-i18n.py --write crates/theway-core/README.md
make doc-sync
```

`scripts/verify-doc-i18n.py` 检查配对完整性、语言切换行、记录的 hash 和 Markdown 结构。检查成功只能证明经评审的文件内容被共同记录；语义与语言质量仍由人工评审负责。
