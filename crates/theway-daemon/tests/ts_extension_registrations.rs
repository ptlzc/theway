use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};
use tempfile::tempdir;
use theway_contract::extension::{
    ExtensionActionKind, ExtensionClientContributionData, ExtensionHookClass,
    ExtensionLifecycleEvent, ExtensionModelRef, ExtensionPermission, ExtensionScope,
    ExtensionTrustDecision,
};
use theway_core::agent::runtime_extensions::{
    RuntimeExtensionContext, RuntimeExtensionInvocation, RuntimeRequestExtensionPort,
};
use theway_core::{AgentTool, AgentToolError, AgentToolResult, AgentToolUpdate};
use theway_daemon::executor::local::LocalExecutor;
use theway_daemon::ts_extensions::{
    EffectDisposeOutcome, EffectKind, EffectLedger, EffectOwner, EffectRegistration,
    EffectScopeBinding, ExtensionBrokerServices, ExtensionCommandContext, ExtensionTrustStore,
    OwnedRegistration, PackageCatalog, QuickJsEngineLimits, QuickJsEnginePool,
    RuntimeExtensionHostConfig, SessionPluginHost, ToolPermission, ToolRegistration,
};
use theway_llm_provider::{Provider, Tool};
use tokio_util::sync::CancellationToken;

fn project_root(project: &Path) -> PathBuf {
    project.join(".theway").join("extensions")
}

fn write_package(project: &Path, id: &str, permissions: &[&str], source: &str) {
    let package = project_root(project).join(id);
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(
        package.join("theway-extension.json"),
        serde_json::to_vec_pretty(&json!({
            "id": id,
            "version": "1.0.0",
            "abi": 2,
            "entry": "index.js",
            "priority": 0,
            "scope": "session",
            "permissions": permissions,
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(package.join("index.js"), source).unwrap();
}

fn permissions(values: &[&str]) -> Vec<ExtensionPermission> {
    values.iter().map(|value| value.parse().unwrap()).collect()
}

fn trust_project(project: &Path, base: &Path, requested: &[&str]) {
    let requested = permissions(requested);
    let mut trust = ExtensionTrustStore::load(base);
    trust
        .decide_project(
            project,
            requested.clone(),
            requested,
            ExtensionTrustDecision::Trusted,
        )
        .unwrap();
    trust.save().unwrap();
}

async fn start_host(
    project: &Path,
    base: &Path,
    permissions: &[&str],
    services: Option<ExtensionBrokerServices>,
) -> Arc<SessionPluginHost> {
    trust_project(project, base, permissions);
    let engine = match services {
        Some(services) => {
            QuickJsEnginePool::with_broker_services(1, QuickJsEngineLimits::default(), services)
        }
        None => QuickJsEnginePool::new(1),
    };
    Arc::new(
        SessionPluginHost::start(
            PackageCatalog::discover(project, base),
            engine,
            "registration-session",
            project,
        )
        .await,
    )
}

fn tool_effect(
    registration_id: u64,
    name: &str,
    scope: ExtensionScope,
    override_existing: bool,
) -> EffectRegistration {
    EffectRegistration {
        registration_id,
        sequence: registration_id,
        value: OwnedRegistration::Tool(ToolRegistration {
            definition: Tool {
                name: name.into(),
                description: format!("{name} description"),
                parameters: json!({"type": "object"}),
            },
            label: name.into(),
            result_schema: None,
            permission: ToolPermission::Allow,
            scope,
            override_existing,
        }),
    }
}

#[test]
fn effect_ledger_restores_overrides_and_disposes_exact_scopes_in_reverse_order() {
    let ledger = EffectLedger::default();
    let base_owner = EffectOwner {
        extension_id: "base".into(),
        session_id: "session".into(),
    };
    let override_owner = EffectOwner {
        extension_id: "override".into(),
        session_id: "session".into(),
    };
    let base = ledger
        .accept(
            base_owner,
            EffectScopeBinding::setup(ExtensionScope::Session),
            tool_effect(1, "shared", ExtensionScope::Session, false),
            false,
        )
        .unwrap();
    let replacement = ledger
        .accept(
            override_owner.clone(),
            EffectScopeBinding::setup(ExtensionScope::Session),
            tool_effect(2, "shared", ExtensionScope::Session, true),
            true,
        )
        .unwrap();
    assert_eq!(
        ledger
            .record(replacement)
            .unwrap()
            .restoration_data
            .unwrap()["displacedHandle"],
        base
    );
    assert_eq!(
        ledger.active(EffectKind::Tool, "shared").unwrap().handle,
        replacement
    );
    assert_eq!(
        ledger.dispose(replacement).unwrap(),
        EffectDisposeOutcome::Disposed
    );
    assert_eq!(
        ledger.dispose(replacement).unwrap(),
        EffectDisposeOutcome::AlreadyDisposed
    );
    assert_eq!(
        ledger.active(EffectKind::Tool, "shared").unwrap().handle,
        base
    );

    let run_a = ledger
        .accept(
            override_owner.clone(),
            EffectScopeBinding::bound(ExtensionScope::Run, Some("run-a".into()), None).unwrap(),
            tool_effect(3, "run-a-tool", ExtensionScope::Run, false),
            false,
        )
        .unwrap();
    let run_b = ledger
        .accept(
            override_owner.clone(),
            EffectScopeBinding::bound(ExtensionScope::Run, Some("run-b".into()), None).unwrap(),
            tool_effect(4, "run-b-tool", ExtensionScope::Run, false),
            false,
        )
        .unwrap();
    let request = ledger
        .accept(
            override_owner.clone(),
            EffectScopeBinding::bound(
                ExtensionScope::Request,
                Some("run-b".into()),
                Some("request-b".into()),
            )
            .unwrap(),
            tool_effect(5, "request-tool", ExtensionScope::Request, false),
            false,
        )
        .unwrap();
    assert_eq!(
        ledger.dispose_scope(ExtensionScope::Run, Some("run-a")),
        [run_a]
    );
    assert!(ledger.record(run_b).is_ok());
    assert!(ledger.record(request).is_ok());
    assert_eq!(
        ledger.dispose_owner(&override_owner),
        [request, run_b],
        "owner disposal must use reverse acceptance order"
    );
}

#[tokio::test]
async fn registered_tool_validates_arguments_and_results_and_honours_permission_classification() {
    let project = tempdir().unwrap();
    let base = tempdir().unwrap();
    write_package(
        project.path(),
        "tool-extension",
        &["tools.register"],
        r#"import { defineExtension } from "theway";
export default defineExtension((api) => {
  api.registerTool({
    name: "extension_echo",
    label: "Extension echo",
    description: "Echo validated text",
    inputSchema: {
      type: "object",
      required: ["text"],
      properties: { text: { type: "string" } },
      additionalProperties: false,
    },
    resultSchema: {
      type: "object",
      required: ["content", "details"],
      properties: { content: { type: "array" }, details: { type: "object" } },
    },
    permission: "prompt",
  }, async ({ arguments: args }) => ({
    content: [{ type: "text", text: args.text }],
    details: { source: "extension" },
  }));
});"#,
    );
    let host = start_host(project.path(), base.path(), &["tools.register"], None).await;
    let tools = host.merge_registered_tools(Vec::new());
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].definition().name, "extension_echo");
    assert!(matches!(
        tools[0].permission_classification(&json!({"text": "hi"})),
        theway_core::PermissionClassification::Prompt { .. }
    ));
    let error = tools[0]
        .execute(
            "call-invalid",
            json!({"text": 7}),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("inputSchema"));
    let result = tools[0]
        .execute(
            "call-ok",
            json!({"text": "hello"}),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(result).unwrap()["content"][0]["text"],
        "hello"
    );
    host.shutdown().await;
}

struct BuiltinTool {
    definition: Tool,
}

impl BuiltinTool {
    fn named(name: &str) -> Self {
        Self {
            definition: Tool {
                name: name.into(),
                description: "built-in implementation".into(),
                parameters: json!({"type": "object"}),
            },
        }
    }
}

#[async_trait]
impl AgentTool for BuiltinTool {
    fn definition(&self) -> &Tool {
        &self.definition
    }

    fn label(&self) -> &str {
        "Built-in"
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        _params: Value,
        _cancel: CancellationToken,
        _on_update: Option<AgentToolUpdate>,
    ) -> Result<AgentToolResult, AgentToolError> {
        Ok(AgentToolResult::default())
    }
}

#[tokio::test]
async fn tool_conflicts_reject_by_default_and_authorized_override_restores_base_snapshot() {
    let project = tempdir().unwrap();
    let base = tempdir().unwrap();
    write_package(
        project.path(),
        "override-extension",
        &["tools.register", "tools.override"],
        r#"import { defineExtension } from "theway";
export default defineExtension((api) => {
  api.registerTool({
    name: "shared_tool", label: "Override", description: "Override implementation",
    inputSchema: { type: "object" }, override: true,
  }, async () => ({ content: [], details: { implementation: "override" } }));
});"#,
    );
    let requested = ["tools.register", "tools.override"];
    let host = start_host(project.path(), base.path(), &requested, None).await;
    let base_tool: Arc<dyn AgentTool> = Arc::new(BuiltinTool::named("shared_tool"));
    let merged = host.merge_registered_tools(vec![Arc::clone(&base_tool)]);
    assert_eq!(merged[0].label(), "Override");
    host.shutdown().await;
    let restored = host.merge_registered_tools(vec![Arc::clone(&base_tool)]);
    assert_eq!(restored[0].label(), "Built-in");

    let project = tempdir().unwrap();
    let base = tempdir().unwrap();
    write_package(
        project.path(),
        "collision-extension",
        &["tools.register"],
        r#"import { defineExtension } from "theway";
export default defineExtension((api) => {
  api.registerTool({
    name: "shared_tool", label: "Collision", description: "Must not replace",
    inputSchema: { type: "object" },
  }, async () => ({ content: [], details: {} }));
});"#,
    );
    let host = start_host(project.path(), base.path(), &["tools.register"], None).await;
    let merged = host.merge_registered_tools(vec![base_tool]);
    assert_eq!(merged[0].label(), "Built-in");
    assert!(host.diagnostics().iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("conflicts with an existing tool")
    }));
    host.shutdown().await;
}

#[tokio::test]
async fn daemon_commands_and_client_contributions_are_typed_and_client_neutral() {
    let project = tempdir().unwrap();
    let base = tempdir().unwrap();
    let requested = ["commands.register", "client.contribute"];
    write_package(
        project.path(),
        "client-extension",
        &requested,
        r#"import { defineExtension } from "theway";
export default defineExtension((api) => {
  api.registerCommand({
    name: "extension-inspect",
    label: "Inspect",
    description: "Inspect an item",
    argumentSchema: {
      type: "object", required: ["id"], properties: { id: { type: "string" } },
    },
    availability: { providers: ["openai"], requiresInteractiveClient: false },
  }, async ({ arguments: args }) => ({
    status: "success", message: `inspected:${args.id}`, data: { id: args.id },
  }));
  api.contribute({
    contributionId: "runtime-status",
    extensionId: "client-extension",
    scope: "session",
    contribution: { kind: "status_item", label: "Extension", value: "ready" },
  });
  api.contribute({
    contributionId: "runtime-form",
    extensionId: "client-extension",
    scope: "session",
    contribution: {
      kind: "form_action", title: "Inspect", schema: { type: "object" },
      submitCommand: "extension-inspect",
    },
  });
  api.contribute({
    contributionId: "runtime-notice",
    extensionId: "client-extension",
    scope: "session",
    contribution: {
      kind: "notification", level: "info", title: "Extension", body: "Loaded",
    },
  });
  api.contribute({
    contributionId: "runtime-command",
    extensionId: "client-extension",
    scope: "session",
    contribution: {
      kind: "command",
      command: {
        name: "extension-inspect", label: "Inspect", description: "Inspect an item",
        argumentSchema: { type: "object" },
      },
    },
  });
});"#,
    );
    let host = start_host(project.path(), base.path(), &requested, None).await;
    let commands = host.registered_commands();
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].extension_id, "client-extension");
    assert_eq!(commands[0].descriptor.name, "extension-inspect");
    let context = ExtensionCommandContext {
        provider: "openai".into(),
        model: "gpt-test".into(),
        has_interactive_client: false,
    };
    assert!(
        host.invoke_registered_command("extension-inspect", json!({"id": 7}), &context)
            .await
            .unwrap_err()
            .contains("argumentSchema")
    );
    let outcome = host
        .invoke_registered_command("extension-inspect", json!({"id": "abc"}), &context)
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(outcome).unwrap()["message"],
        "inspected:abc"
    );
    let contributions = host.client_contributions();
    assert_eq!(contributions.len(), 4);
    assert!(
        contributions
            .iter()
            .all(|item| item.extension_id == "client-extension")
    );
    assert!(contributions.iter().any(|item| {
        matches!(
            item.contribution,
            ExtensionClientContributionData::StatusItem { .. }
        )
    }));
    assert!(contributions.iter().any(|item| {
        matches!(
            item.contribution,
            ExtensionClientContributionData::FormAction { .. }
        )
    }));
    assert!(contributions.iter().any(|item| {
        matches!(
            item.contribution,
            ExtensionClientContributionData::Notification { .. }
        )
    }));
    assert!(contributions.iter().any(|item| {
        matches!(
            item.contribution,
            ExtensionClientContributionData::Command { .. }
        )
    }));
    host.shutdown().await;
}

#[tokio::test]
async fn declarative_provider_formats_use_existing_adapters_and_never_expose_credentials() {
    let project = tempdir().unwrap();
    let base = tempdir().unwrap();
    let requested = ["providers.register", "secrets.read:provider-token"];
    write_package(
        project.path(),
        "provider-extension",
        &requested,
        r#"import { defineExtension } from "theway";
export default defineExtension((api) => {
  for (const [providerId, format, model] of [
    ["extension-chat-provider", "openai_chat_completions", "chat-model"],
    ["extension-responses-provider", "openai_responses", "responses-model"],
    ["extension-anthropic-provider", "anthropic_messages", "anthropic-model"],
  ]) {
    api.registerProvider({
      providerId, baseUrl: `https://${providerId}.example.test/v1`, format,
      credentialRef: "provider-token",
      models: [{ id: model, name: model, contextWindow: 32000, maxTokens: 4096 }],
    });
  }
});"#,
    );
    let services = ExtensionBrokerServices::new(
        base.path(),
        Arc::new(LocalExecutor::with_cwd(project.path())),
    );
    services.set_secret("provider-token", "sk-secret-never-serialize");
    let host = start_host(project.path(), base.path(), &requested, Some(services)).await;
    let formats = [
        (
            "extension-chat-provider",
            "chat-model",
            "openai-completions",
        ),
        (
            "extension-responses-provider",
            "responses-model",
            "openai-responses",
        ),
        (
            "extension-anthropic-provider",
            "anthropic-model",
            "anthropic-messages",
        ),
    ];
    for (provider, model, api) in formats {
        let registered = theway_llm_provider::get_model(&Provider(provider.into()), model).unwrap();
        assert_eq!(registered.api.0, api);
        assert!(
            !serde_json::to_string(&registered)
                .unwrap()
                .contains("sk-secret")
        );
    }
    assert!(
        !serde_json::to_string(&host.diagnostics())
            .unwrap()
            .contains("sk-secret")
    );
    host.shutdown().await;
    for (provider, model, _) in formats {
        assert!(theway_llm_provider::get_model(&Provider(provider.into()), model).is_none());
    }
}

#[tokio::test]
async fn unsupported_provider_format_rejects_only_that_registration() {
    let project = tempdir().unwrap();
    let base = tempdir().unwrap();
    let requested = ["providers.register", "tools.register"];
    write_package(
        project.path(),
        "mixed-registration-extension",
        &requested,
        r#"import { defineExtension } from "theway";
export default defineExtension((api) => {
  api.registerProvider({
    providerId: "unsupported-provider", baseUrl: "https://example.test/v1",
    format: "name-similar-but-unsupported",
    models: [{ id: "bad-model", name: "Bad", contextWindow: 1000, maxTokens: 100 }],
  });
  api.registerTool({
    name: "surviving_tool", label: "Survivor", description: "Still accepted",
    inputSchema: { type: "object" },
  }, async () => ({ content: [], details: {} }));
});"#,
    );
    let host = start_host(project.path(), base.path(), &requested, None).await;
    assert_eq!(
        host.active_extension_ids().await,
        ["mixed-registration-extension"]
    );
    assert_eq!(host.merge_registered_tools(Vec::new()).len(), 1);
    assert!(host.diagnostics().iter().any(|diagnostic| {
        diagnostic.extension_id == "mixed-registration-extension"
            && diagnostic.message.contains("name-similar-but-unsupported")
    }));
    assert!(
        theway_llm_provider::get_model(&Provider("unsupported-provider".into()), "bad-model")
            .is_none()
    );
    host.shutdown().await;
}

#[tokio::test]
async fn prompt_sections_and_request_policies_match_model_and_follow_priority_order() {
    let project = tempdir().unwrap();
    let base = tempdir().unwrap();
    write_package(
        project.path(),
        "request-extension",
        &[],
        r#"import { defineExtension } from "theway";
export default defineExtension((api) => {
  api.registerPromptSection({
    sectionId: "low", text: "low-priority", priority: 5,
    predicate: { providers: ["openai"], models: ["target-model"] },
  });
  api.registerPromptSection({
    sectionId: "high", text: "high-priority", priority: 20,
    predicate: { providers: ["openai"], models: ["target-model"] },
  });
  api.registerRequestPolicy({
    policyId: "token-policy", priority: 10,
    predicate: { providers: ["openai"], models: ["target-model"] },
  }, async ({ request }) => ({
    abiMajor: 2,
    actions: [{ kind: "replace_model_request", payload: {
      request: { ...request, generationOptions: { maxTokens: 7 } },
    }}],
  }));
});"#,
    );
    let host = start_host(project.path(), base.path(), &[], None).await;
    let request = |model: &str| {
        let mut context = RuntimeExtensionContext::new("registration-session", "/workspace", 1);
        context.model = Some(ExtensionModelRef {
            provider: "openai".into(),
            model: model.into(),
        });
        RuntimeExtensionInvocation::new(
            ExtensionLifecycleEvent::BeforeModelRequest,
            ExtensionHookClass::Transform,
            context,
            json!({"request": {
                "provider": "openai", "model": model,
                "systemInstructions": "base", "generationOptions": {}, "tools": [],
            }}),
        )
        .unwrap()
    };
    let matched = RuntimeRequestExtensionPort::invoke_request(&*host, request("target-model"))
        .await
        .unwrap();
    let replacement = matched
        .actions
        .iter()
        .find(|action| action.kind == ExtensionActionKind::ReplaceModelRequest)
        .unwrap();
    assert_eq!(
        replacement.payload["request"]["systemInstructions"],
        "base\n\nhigh-priority\n\nlow-priority"
    );
    assert_eq!(
        replacement.payload["request"]["generationOptions"]["maxTokens"],
        7
    );
    let unmatched = RuntimeRequestExtensionPort::invoke_request(&*host, request("other-model"))
        .await
        .unwrap();
    assert!(unmatched.actions.is_empty());
    host.shutdown().await;
}

#[tokio::test]
async fn runtime_handle_disposal_is_idempotent_and_stops_future_dispatch() {
    let project = tempdir().unwrap();
    let base = tempdir().unwrap();
    write_package(
        project.path(),
        "handle-extension",
        &["client.contribute"],
        r#"import { defineExtension } from "theway";
export default defineExtension((api) => {
  const status = api.contribute({
    contributionId: "temporary-status", extensionId: "handle-extension", scope: "session",
    contribution: { kind: "status_item", label: "Temporary", value: "active" },
  });
  let hook;
  hook = api.on("input", () => {
    status.dispose();
    status.dispose();
    hook.dispose();
    let updateError;
    try { status.update({}); } catch (error) { updateError = error.code; }
    return {
      abiMajor: 2,
      actions: [{ kind: "emit_diagnostic", payload: { updateError } }],
    };
  });
});"#,
    );
    let host = start_host(project.path(), base.path(), &["client.contribute"], None).await;
    assert_eq!(host.active_effect_count().await, 2);
    let first = host.invoke(ExtensionLifecycleEvent::Input, json!({})).await;
    assert_eq!(
        first[0].value["actions"][0]["payload"]["updateError"],
        "effect_disposed"
    );
    assert_eq!(host.active_effect_count().await, 0);
    assert!(host.client_contributions().is_empty());
    let second = host.invoke(ExtensionLifecycleEvent::Input, json!({})).await;
    assert!(second[0].value["actions"].as_array().unwrap().is_empty());
    host.shutdown().await;
    assert_eq!(host.active_effect_count().await, 0);
}

#[tokio::test]
async fn circuit_fault_reverses_every_effect_owned_by_the_extension() {
    let project = tempdir().unwrap();
    let base = tempdir().unwrap();
    write_package(
        project.path(),
        "faulted-registration-extension",
        &["tools.register"],
        r#"import { defineExtension } from "theway";
export default defineExtension((api) => {
  api.registerTool({
    name: "temporary_tool", label: "Temporary", description: "Removed on fault",
    inputSchema: { type: "object" },
  }, async () => ({ content: [], details: {} }));
  api.on("input", () => { throw new Error("open circuit"); });
});"#,
    );
    trust_project(project.path(), base.path(), &["tools.register"]);
    let engine = QuickJsEnginePool::new(1);
    let host = SessionPluginHost::start_with_config(
        PackageCatalog::discover(project.path(), base.path()),
        engine.clone(),
        "registration-session",
        project.path(),
        RuntimeExtensionHostConfig {
            circuit_failure_threshold: 1,
            ..RuntimeExtensionHostConfig::default()
        },
    )
    .await;
    assert_eq!(host.active_effect_count().await, 2);
    assert_eq!(host.merge_registered_tools(Vec::new()).len(), 1);
    assert!(
        host.invoke(ExtensionLifecycleEvent::Input, json!({}))
            .await
            .is_empty()
    );
    assert_eq!(host.active_effect_count().await, 0);
    assert!(host.merge_registered_tools(Vec::new()).is_empty());
    assert_eq!(engine.instance_count().await, 0);
    host.shutdown().await;
}
