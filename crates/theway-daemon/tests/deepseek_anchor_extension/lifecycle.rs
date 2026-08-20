use std::sync::Arc;

use serde_json::json;
use tempfile::tempdir;
use theway_contract::extension::{ExtensionDiagnosticCode, ExtensionDurableEntryPayload};
use theway_core::agent::model_request::{NormalizedGenerationOptions, NormalizedModelRequestDraft};
use theway_core::agent::runtime_extensions::{
    RuntimeMessageExtensionPort, RuntimeRequestExtensionPort,
};
use theway_daemon::ts_extensions::RuntimeExtensionHostConfig;

use super::support::*;

async fn promote(host: &theway_daemon::ts_extensions::SessionPluginHost, sequence: u64) {
    let tools = merged_tool_definitions(host, Vec::new());
    let bootstrap = RuntimeRequestExtensionPort::invoke_request(
        host,
        request_invocation(sequence, "deepseek", "deepseek-chat", tools, Some(4096)),
    )
    .await
    .unwrap();
    assert!(replacement(&bootstrap).is_some());
    RuntimeMessageExtensionPort::invoke_message(
        host,
        assistant_invocation(sequence + 1, "deepseek", "deepseek-chat", "ready"),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn bootstrap_matching_request_filters_exact_tools_and_preserves_default_limit() {
    let project = tempdir().unwrap();
    let base = tempdir().unwrap();
    install_anchor(project.path(), &enabled_config());
    let state = Arc::new(MemoryStatePort::default());
    let (host, _) = start_anchor(
        project.path(),
        base.path(),
        state,
        false,
        RuntimeExtensionHostConfig::default(),
    )
    .await;
    let mut base_tools = vec![compatible_bash()];
    base_tools.push(Arc::new(TestTool::new(
        "unrelated",
        json!({"type": "object"}),
    )));
    let tools = merged_tool_definitions(&host, base_tools);

    let batch = RuntimeRequestExtensionPort::invoke_request(
        &*host,
        request_invocation(1, "deepseek", "deepseek-chat", tools, Some(4096)),
    )
    .await
    .unwrap();

    let request = replacement(&batch).unwrap();
    assert_eq!(request["systemInstructions"], "ANCHOR BOOTSTRAP");
    assert_eq!(request["messages"], json!([]));
    assert_eq!(
        request["executableToolNames"],
        json!(["bash", "str_replace_editor"])
    );
    assert_eq!(request["visibleTools"].as_array().unwrap().len(), 2);
    assert_eq!(request["generationOptions"]["maxTokens"], 4096);
    assert!(host.diagnostics().iter().any(|diagnostic| {
        diagnostic.code == ExtensionDiagnosticCode::LifecycleStatus
            && diagnostic.details.get("phase") == Some(&json!("bootstrap"))
    }));
    host.shutdown().await;
}

#[tokio::test]
async fn bootstrap_explicit_token_limit_applies_only_before_promotion() {
    let project = tempdir().unwrap();
    let base = tempdir().unwrap();
    let mut config = enabled_config();
    config["bootstrapTokenLimit"] = json!(777);
    config["personaScope"] = json!("session");
    install_anchor(project.path(), &config);
    let state = Arc::new(MemoryStatePort::default());
    let (host, _) = start_anchor(
        project.path(),
        base.path(),
        state,
        false,
        RuntimeExtensionHostConfig::default(),
    )
    .await;
    let tools = merged_tool_definitions(&host, Vec::new());
    let bootstrap = RuntimeRequestExtensionPort::invoke_request(
        &*host,
        request_invocation(1, "deepseek", "deepseek-chat", tools.clone(), Some(4096)),
    )
    .await
    .unwrap();
    assert_eq!(
        replacement(&bootstrap).unwrap()["generationOptions"]["maxTokens"],
        777
    );

    RuntimeMessageExtensionPort::invoke_message(
        &*host,
        assistant_invocation(2, "deepseek", "deepseek-chat", "ready"),
    )
    .await
    .unwrap();
    let promoted = RuntimeRequestExtensionPort::invoke_request(
        &*host,
        request_invocation(3, "deepseek", "deepseek-chat", tools, Some(4096)),
    )
    .await
    .unwrap();

    let request = replacement(&promoted).unwrap();
    assert_eq!(request["generationOptions"]["maxTokens"], 4096);
    assert_eq!(
        request["systemInstructions"],
        "ANCHOR BOOTSTRAP\n\nBASE SYSTEM"
    );
    assert_eq!(request["messages"].as_array().unwrap().len(), 1);
    assert!(request["visibleTools"].as_array().unwrap().len() >= 2);
    host.shutdown().await;
}

#[tokio::test]
async fn promotion_commits_marker_event_and_context_once_before_next_request() {
    let project = tempdir().unwrap();
    let base = tempdir().unwrap();
    install_anchor(project.path(), &enabled_config());
    let state = Arc::new(MemoryStatePort::default());
    let (host, _) = start_anchor(
        project.path(),
        base.path(),
        state.clone(),
        false,
        RuntimeExtensionHostConfig::default(),
    )
    .await;

    promote(&host, 1).await;
    RuntimeMessageExtensionPort::invoke_message(
        &*host,
        assistant_invocation(3, "deepseek", "deepseek-chat", "ready again"),
    )
    .await
    .unwrap();

    let entries = state.entries();
    assert_eq!(entries.len(), 3);
    assert_eq!(
        entries
            .iter()
            .filter(|entry| matches!(
                entry.entry,
                ExtensionDurableEntryPayload::StateMutation { .. }
            ))
            .count(),
        1
    );
    assert_eq!(
        entries
            .iter()
            .filter(|entry| matches!(
                entry.entry,
                ExtensionDurableEntryPayload::CustomEvent { .. }
            ))
            .count(),
        1
    );
    assert_eq!(host.model_context_projection().items().len(), 1);
    let tools = merged_tool_definitions(&host, Vec::new());
    let promoted = RuntimeRequestExtensionPort::invoke_request(
        &*host,
        request_invocation(4, "deepseek", "deepseek-chat", tools.clone(), Some(4096)),
    )
    .await
    .unwrap();
    assert!(
        replacement(&promoted).is_none(),
        "bootstrap-only persona keeps the base request"
    );

    let mut request = NormalizedModelRequestDraft {
        provider: "deepseek".into(),
        model: "deepseek-chat".into(),
        system_instructions: Some("BASE SYSTEM".into()),
        messages: Vec::new(),
        visible_tools: tools.clone(),
        executable_tool_names: tools.iter().map(|tool| tool.name.clone()).collect(),
        generation_options: NormalizedGenerationOptions::default(),
    };
    host.model_context_projection()
        .apply_to_request(&mut request);
    assert_eq!(
        request.system_instructions.unwrap(),
        format!("BASE SYSTEM\n\n{RESTORED_CONTEXT}")
    );
    host.shutdown().await;
}

#[tokio::test]
async fn replay_resume_forks_and_branch_switches_reconstruct_branch_phase() {
    let project = tempdir().unwrap();
    let base = tempdir().unwrap();
    install_anchor(project.path(), &enabled_config());
    let promoted_state = Arc::new(MemoryStatePort::default());
    let (first, _) = start_anchor(
        project.path(),
        base.path(),
        promoted_state.clone(),
        false,
        RuntimeExtensionHostConfig::default(),
    )
    .await;
    promote(&first, 1).await;
    let promoted_entries = promoted_state.entries();
    first.shutdown().await;

    let scenarios = [
        ("resume", promoted_entries.clone(), false),
        ("fork-after", promoted_entries.clone(), false),
        ("fork-before", Vec::new(), true),
        ("switched-unpromoted", Vec::new(), true),
        ("switched-promoted", promoted_entries, false),
    ];
    for (name, entries, expects_bootstrap) in scenarios {
        let state = Arc::new(MemoryStatePort::with_entries(entries));
        let (host, _) = start_anchor(
            project.path(),
            base.path(),
            state,
            false,
            RuntimeExtensionHostConfig::default(),
        )
        .await;
        let tools = merged_tool_definitions(&host, Vec::new());
        let result = RuntimeRequestExtensionPort::invoke_request(
            &*host,
            request_invocation(10, "deepseek", "deepseek-chat", tools, Some(4096)),
        )
        .await
        .unwrap();
        assert_eq!(replacement(&result).is_some(), expects_bootstrap, "{name}");
        host.shutdown().await;
    }
}

#[tokio::test]
async fn concurrent_sessions_model_switching_and_zero_anchor_keep_state_isolated() {
    let project = tempdir().unwrap();
    let base = tempdir().unwrap();
    install_anchor(project.path(), &enabled_config());
    let state_a = Arc::new(MemoryStatePort::default());
    let state_b = Arc::new(MemoryStatePort::default());
    let (host_a, _) = start_anchor(
        project.path(),
        base.path(),
        state_a,
        false,
        RuntimeExtensionHostConfig::default(),
    )
    .await;
    let (host_b, _) = start_anchor(
        project.path(),
        base.path(),
        state_b,
        false,
        RuntimeExtensionHostConfig::default(),
    )
    .await;
    promote(&host_a, 1).await;
    let (result_a, result_b) = tokio::join!(
        RuntimeRequestExtensionPort::invoke_request(
            &*host_a,
            request_invocation(
                3,
                "openai",
                "other-model",
                merged_tool_definitions(&host_a, Vec::new()),
                Some(4096),
            ),
        ),
        RuntimeRequestExtensionPort::invoke_request(
            &*host_b,
            request_invocation(
                3,
                "deepseek",
                "deepseek-chat",
                merged_tool_definitions(&host_b, Vec::new()),
                Some(4096),
            ),
        ),
    );
    assert!(replacement(&result_a.unwrap()).is_none());
    assert!(replacement(&result_b.unwrap()).is_some());
    let back = RuntimeRequestExtensionPort::invoke_request(
        &*host_a,
        request_invocation(
            4,
            "deepseek",
            "deepseek-chat",
            merged_tool_definitions(&host_a, Vec::new()),
            Some(4096),
        ),
    )
    .await
    .unwrap();
    assert!(
        replacement(&back).is_none(),
        "temporary model switch preserves promotion"
    );
    host_a.shutdown().await;
    host_b.shutdown().await;

    let zero_project = tempdir().unwrap();
    let zero_base = tempdir().unwrap();
    let mut zero = enabled_config();
    zero["zeroAnchor"] = json!(true);
    install_anchor(zero_project.path(), &zero);
    let (zero_host, _) = start_anchor(
        zero_project.path(),
        zero_base.path(),
        Arc::new(MemoryStatePort::default()),
        false,
        RuntimeExtensionHostConfig::default(),
    )
    .await;
    let zero_result = RuntimeRequestExtensionPort::invoke_request(
        &*zero_host,
        request_invocation(
            1,
            "deepseek",
            "deepseek-chat",
            vec![compatible_bash().definition().clone()],
            Some(4096),
        ),
    )
    .await
    .unwrap();
    assert!(replacement(&zero_result).is_none());
    assert!(zero_host.merge_registered_tools(Vec::new()).is_empty());
    zero_host.shutdown().await;
}
