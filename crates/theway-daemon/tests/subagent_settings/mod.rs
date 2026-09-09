use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use theway_contract::subagent_settings::SubagentRunSettings;
use theway_core::multiagent::graph::types::DagNodeDef;
use theway_daemon::subagent_settings::{
    SubagentSettingsRegistry, SubagentSettingsStore, merge_nodes_settings, merge_run_settings,
};

fn temp_project() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "theway-subagent-settings-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn node(id: &str, provider: Option<&str>, model: Option<&str>, thinking: Option<&str>) -> DagNodeDef {
    DagNodeDef {
        id: id.to_string(),
        agent: "general".to_string(),
        task: "work".to_string(),
        depends_on: None,
        timeout: None,
        cwd: None,
        provider: provider.map(str::to_string),
        model: model.map(str::to_string),
        thinking: thinking.map(str::to_string),
        max_iterations: None,
        tools: None,
    }
}

fn settings(provider: Option<&str>, model: Option<&str>, thinking: Option<&str>) -> SubagentRunSettings {
    SubagentRunSettings {
        provider: provider.map(str::to_string),
        model: model.map(str::to_string),
        thinking: thinking.map(str::to_string),
    }
}

#[test]
fn merge_inherits_remembered_when_nothing_explicit() {
    let remembered = settings(Some("deepseek"), Some("deepseek-v4-flash"), Some("high"));
    let merged = merge_run_settings(None, None, None, &remembered);
    assert_eq!(merged, remembered);
}

#[test]
fn merge_replaces_provider_model_pair_as_one_unit() {
    let remembered = settings(Some("deepseek"), Some("deepseek-v4-flash"), Some("high"));
    // Explicit model only: the remembered provider is NOT inherited.
    let merged = merge_run_settings(None, Some("deepseek-v4-max"), None, &remembered);
    assert_eq!(merged.provider, None);
    assert_eq!(merged.model.as_deref(), Some("deepseek-v4-max"));
    // The independent thinking field is still inherited.
    assert_eq!(merged.thinking.as_deref(), Some("high"));
}

#[test]
fn merge_keeps_thinking_independent_of_the_pair() {
    let remembered = settings(Some("deepseek"), Some("deepseek-v4-flash"), Some("high"));
    let merged = merge_run_settings(None, None, Some("off"), &remembered);
    assert_eq!(merged.provider.as_deref(), Some("deepseek"));
    assert_eq!(merged.model.as_deref(), Some("deepseek-v4-flash"));
    assert_eq!(merged.thinking.as_deref(), Some("off"));
}

#[test]
fn merge_treats_empty_strings_as_clear() {
    let remembered = settings(Some("deepseek"), Some("deepseek-v4-flash"), Some("high"));
    let merged = merge_run_settings(Some(""), Some(""), Some(""), &remembered);
    assert!(merged.is_empty());
    // Clearing only the thinking keeps the remembered pair.
    let merged = merge_run_settings(None, None, Some(""), &remembered);
    assert_eq!(merged.provider.as_deref(), Some("deepseek"));
    assert_eq!(merged.model.as_deref(), Some("deepseek-v4-flash"));
    assert_eq!(merged.thinking, None);
}

#[test]
fn merge_nodes_applies_defaults_and_reports_changes() {
    let mut nodes = vec![
        node("impl", None, None, None),
        node("check", Some("openai"), Some("gpt-5-mini"), None),
        node("probe", Some("anthropic"), None, None),
    ];
    let mut remembered = BTreeMap::new();
    remembered.insert(
        "impl".to_string(),
        settings(Some("deepseek"), Some("deepseek-v4-flash"), Some("high")),
    );
    let updates = merge_nodes_settings(&mut nodes, &remembered);
    // "impl": inherited and already equals the remembered record -> no write.
    assert_eq!(nodes[0].provider.as_deref(), Some("deepseek"));
    assert_eq!(nodes[0].model.as_deref(), Some("deepseek-v4-flash"));
    assert_eq!(nodes[0].thinking.as_deref(), Some("high"));
    // "check": explicit pair -> one update entry; "probe": provider without
    // model is applied for this run but NOT remembered (it can never launch).
    assert_eq!(updates.len(), 1);
    assert!(updates.iter().any(|(id, _)| id == "check"));
    assert_eq!(nodes[2].provider.as_deref(), Some("anthropic"));
    assert_eq!(nodes[2].model, None);
    assert!(!updates.iter().any(|(id, _)| id == "probe"));
}

#[test]
fn merge_nodes_reports_no_updates_when_effective_equals_remembered() {
    let mut nodes = vec![node("impl", None, None, None)];
    let mut remembered = BTreeMap::new();
    remembered.insert(
        "impl".to_string(),
        settings(Some("deepseek"), Some("deepseek-v4-flash"), Some("high")),
    );
    // Inherited values equal the remembered record: nothing to persist.
    let updates = merge_nodes_settings(&mut nodes, &remembered);
    assert!(updates.is_empty());
    assert_eq!(nodes[0].provider.as_deref(), Some("deepseek"));
}

#[tokio::test]
async fn store_round_trips_node_memory() {
    let project = temp_project();
    let store = SubagentSettingsStore::new(&project);
    let mut nodes = vec![node("impl", None, None, None)];
    let updates = store.merge_nodes(&mut nodes).await;
    assert!(updates.is_empty());
    assert!(nodes[0].provider.is_none());

    // Remember an override, then a fresh merge inherits it.
    store
        .remember_nodes(vec![(
            "impl".to_string(),
            settings(Some("deepseek"), Some("deepseek-v4-flash"), Some("high")),
        )])
        .await;
    let mut nodes = vec![node("impl", None, None, None)];
    let updates = store.merge_nodes(&mut nodes).await;
    assert_eq!(nodes[0].provider.as_deref(), Some("deepseek"));
    assert_eq!(nodes[0].model.as_deref(), Some("deepseek-v4-flash"));
    assert_eq!(nodes[0].thinking.as_deref(), Some("high"));
    // The inherited record is already the effective one: nothing to upsert.
    assert!(updates.is_empty());

    // An empty record removes the memory.
    store.remember_nodes(vec![("impl".to_string(), settings(None, None, None))]).await;
    let mut nodes = vec![node("impl", None, None, None)];
    let updates = store.merge_nodes(&mut nodes).await;
    assert!(updates.is_empty());
    assert!(nodes[0].provider.is_none());
}

#[tokio::test]
async fn store_survives_corrupt_file_with_defaults() {
    let project = temp_project();
    let pi = project.join(".pi");
    std::fs::create_dir_all(&pi).unwrap();
    std::fs::write(pi.join("subagent-settings.json"), "{not json").unwrap();
    let store = SubagentSettingsStore::new(&project);
    let mut nodes = vec![node("impl", None, None, None)];
    let updates = store.merge_nodes(&mut nodes).await;
    assert!(updates.is_empty());
    assert!(nodes[0].model.is_none());
    // A later save replaces the corrupt file.
    store
        .remember_nodes(vec![(
            "impl".to_string(),
            settings(None, Some("deepseek-v4-flash"), None),
        )])
        .await;
    let mut nodes = vec![node("impl", None, None, None)];
    store.merge_nodes(&mut nodes).await;
    assert_eq!(nodes[0].model.as_deref(), Some("deepseek-v4-flash"));
}

#[tokio::test]
async fn registry_shares_one_store_per_project_directory() {
    let project = temp_project();
    let registry = SubagentSettingsRegistry::new();
    let a = registry.store_for(&project);
    let b = registry.store_for(&project.join("."));
    assert!(Arc::ptr_eq(&a, &b));
    let other = temp_project();
    let c = registry.store_for(&other);
    assert!(!Arc::ptr_eq(&a, &c));
}

#[tokio::test]
async fn agent_memory_merges_and_removes() {
    let project = temp_project();
    let store = SubagentSettingsStore::new(&project);
    let merged = store.merge_agent("executor-coder", None, None, None).await;
    assert!(merged.is_empty());
    store
        .remember_agent(
            "executor-coder",
            settings(Some("anthropic"), Some("claude-sonnet-4-5"), None),
        )
        .await;
    let merged = store
        .merge_agent("executor-coder", None, None, Some("medium"))
        .await;
    assert_eq!(merged.provider.as_deref(), Some("anthropic"));
    assert_eq!(merged.model.as_deref(), Some("claude-sonnet-4-5"));
    assert_eq!(merged.thinking.as_deref(), Some("medium"));
    // The caller persists the merged record (mirrors SubagentTool.execute).
    store.remember_agent("executor-coder", merged).await;
    // Explicit pair replaces; the persisted thinking stays.
    let merged = store
        .merge_agent("executor-coder", Some("deepseek"), Some("deepseek-v4-flash"), None)
        .await;
    assert_eq!(merged.provider.as_deref(), Some("deepseek"));
    assert_eq!(merged.thinking.as_deref(), Some("medium"));
    // Remove.
    store.remember_agent("executor-coder", settings(None, None, None)).await;
    let merged = store.merge_agent("executor-coder", None, None, None).await;
    assert!(merged.is_empty());
}
