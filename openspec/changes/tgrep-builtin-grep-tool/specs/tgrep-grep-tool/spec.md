## Purpose

定义 tgrep 驱动的内置 grep 工具契约：daemon 托管 `tgrep serve` 进程、grep 工具按就绪门选择 tgrep 客户端路径或 walker 回退路径，两条路径的输出格式逐字节一致，且任何时刻结果完整正确（不存在半索引窗口）。

## ADDED Requirements

### Requirement: TgrepServerRegistry 托管 serve 进程

daemon SHALL 通过进程级 `TgrepServerRegistry` 按 canonical 根目录管理 `tgrep serve` 子进程：首个该根下的 grep 查询惰性 spawn，二进制按 `current_exe` 同目录 → PATH 发现，LRU 上限淘汰，daemon 退出收割全部子进程。

#### Scenario: 惰性 spawn 与共享

- **WHEN** 两个 session 先后在同一项目根下执行 grep
- **THEN** registry 只 spawn 一个 serve 进程，两个 session 的查询共用
- **AND** 该根下的后续查询不再 spawn

#### Scenario: 二进制缺失

- **WHEN** `tgrep` 二进制既不在 daemon 同目录也不在 PATH
- **THEN** registry 标记该根为 Missing，grep 工具走 walker 路径
- **AND** 不重复尝试 spawn（同一进程生命周期内）

#### Scenario: 子进程退出与回收

- **WHEN** serve 子进程意外退出
- **THEN** registry 清掉该条目，下一个查询重新 spawn
- **AND** daemon 退出（registry Drop）时全部存活子进程被杀死

### Requirement: 索引就绪门

registry SHALL 通过 serve 的 JSON-RPC `status` 判定索引完成（`indexing == false`）；grep 工具 SHALL 只在 registry 判定 Ready 后使用 tgrep 客户端路径，否则使用 walker 路径。

#### Scenario: 建索引期间查询结果完整

- **WHEN** serve 刚 spawn、后台建索引未完成
- **THEN** grep 走 walker 路径，结果覆盖完整文件集
- **AND** 不返回部分索引结果

#### Scenario: 就绪后加速

- **WHEN** registry 已缓存 Ready 且查询根位于 session cwd 之下
- **THEN** grep 走 tgrep 客户端路径（连接 serve）
- **AND** 不重复轮询 status

### Requirement: 输出格式逐字节一致

tgrep 客户端路径与 walker 路径 SHALL 经同一 formatter 产出相同的三种 output_mode（content/files_with_matches/count），包括行号右对齐、`--` 分段、截断预览与 max_results 上限。

#### Scenario: 同一 fixture 两路径输出一致

- **WHEN** 同一查询分别走 walker 路径与 tgrep 路径
- **THEN** content/files_with_matches/count 三种模式的输出文本逐字节一致

#### Scenario: max_results 截断

- **WHEN** 匹配数超过 max_results
- **THEN** 输出以「(N matches shown, may be more)」开头且只含 N 条匹配
- **AND** 客户端进程被提前终止

#### Scenario: 客户端失败回退

- **WHEN** tgrep 客户端执行失败（serve 刚死、路径消失）
- **THEN** grep 工具返回 walker 路径结果或等价完整结果，不报错中断

### Requirement: 分发包含 tgrep 二进制

`scripts/install.sh` 与 GitHub Release SHALL 与 theway/thewayd 同批安装/发布 `tgrep` 二进制，daemon 的同目录发现策略可用。

#### Scenario: 安装

- **WHEN** 运行 `scripts/install.sh`
- **THEN** `tgrep` 与 `theway`、`thewayd` 安装到同一 bin 目录
