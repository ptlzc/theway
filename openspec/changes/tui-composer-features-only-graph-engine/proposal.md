# tui-composer-features-only-graph-engine

Issue: #39

## Problem

#38 在 composer 顶部分隔线右端加了 features 标签，来源两块：

- daemon `active_trigger_features()`（经 sidebar snapshot 的 `runtime` 字段）：
  `dedup` / `cycle suppress` / `fire-once rules` / `inject-and-run` —— trigger
  runtime 的管道特性，对 composer 用户是噪音（"dedup 是什么？cycle suppress
  又是什么？"）。
- dags kind 推导：`graph engine` / `goal`。

用户昨天只规划了 `graph engine`，其余都是乱显示。

## What changes

composer 右上角只保留 `graph engine`（dag-kind run 存在时显示）：

- `ui/mod.rs` `feature_labels()`：去掉 `runtime` 透传与 `goal` 推导，签名收窄为
  `feature_labels(dags) -> Vec<String>`，只有 dag-kind run 存在时返回
  `["graph engine"]`。
- 调用点（render() 里组装 `PromptChrome`）不再传 `sidebar.runtime` / `has_goal`。
- trigger 面板的 Runtime 区块**不动**：`PanelStatus.trigger_features` 继续展示
  trigger runtime 特性（那是它们的家）。
- `prompt_chrome.rs` 渲染逻辑不动（features 为空时本来就不占格子）。
- 单测更新：删 `feature_labels_passes_runtime_features_through` /
  `feature_labels_goal_from_run_or_active_goal_once` / `feature_labels_combined_order`
  ／改 `feature_labels_empty_without_sources` / `feature_labels_derives_graph_engine_from_dag_run`
  与 `chrome_top_divider_shows_feature_labels` fixture。

## Out of scope

- 不动 daemon 的 `active_trigger_features()` 与 sidebar `runtime` 字段
  （trigger 面板仍需要）。
- 不改 `graph engine` 的触发条件（dag-kind run 存在即显示，含已完成 run）。

## Acceptance

- composer 顶部分隔线右端只有 `graph engine`，无 dag run 时什么都不显示。
- trigger 面板 Runtime 区块仍列出 trigger features。
- `make check` / `make test` / `make lint` / `make fmt-check` 通过。
