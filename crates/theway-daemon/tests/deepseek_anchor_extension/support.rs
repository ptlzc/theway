use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::{Value, json};
use theway_contract::extension::{
    ExtensionActionBatch, ExtensionActionKind, ExtensionDurableEntry, ExtensionHookClass,
    ExtensionLifecycleEvent, ExtensionModelRef, ExtensionPermission, ExtensionTrustDecision,
};
use theway_core::agent::runtime_extensions::{
    RuntimeExtensionContext, RuntimeExtensionInvocation, SessionExtensionStateError,
    SessionExtensionStatePort,
};
use theway_core::{AgentTool, AgentToolError, AgentToolResult, AgentToolUpdate};
use theway_daemon::executor::local::LocalExecutor;
use theway_daemon::ts_extensions::{
    ExtensionBrokerServices, ExtensionTrustStore, PackageCatalog, QuickJsEnginePool,
    RuntimeExtensionHostConfig, SessionPluginHost,
};
use theway_llm_provider::{Tool, UserContentBlock};
use tokio_util::sync::CancellationToken;

pub const EXTENSION_ID: &str = "deepseek-anchor";
pub const SESSION_ID: &str = "anchor-session";
pub const RESTORED_CONTEXT: &str = "Bootstrap completed; restore retained context.";

#[derive(Default)]
pub struct MemoryStatePort {
    entries: Mutex<Vec<ExtensionDurableEntry>>,
    fail_writes: AtomicBool,
}

impl MemoryStatePort {
    pub fn with_entries(entries: Vec<ExtensionDurableEntry>) -> Self {
        Self {
            entries: Mutex::new(entries),
            fail_writes: AtomicBool::new(false),
        }
    }

    pub fn entries(&self) -> Vec<ExtensionDurableEntry> {
        self.entries.lock().clone()
    }

    pub fn set_fail_writes(&self, fail: bool) {
        self.fail_writes.store(fail, Ordering::Release);
    }
}

#[async_trait]
impl SessionExtensionStatePort for MemoryStatePort {
    async fn append_durable_entries(
        &self,
        _extension_id: &str,
        entries: Vec<ExtensionDurableEntry>,
    ) -> Result<Vec<String>, SessionExtensionStateError> {
        if self.fail_writes.load(Ordering::Acquire) {
            return Err(SessionExtensionStateError::Unavailable);
        }
        let offset = self.entries.lock().len();
        let ids = (0..entries.len())
            .map(|index| format!("anchor-entry-{}", offset + index + 1))
            .collect();
        self.entries.lock().extend(entries);
        Ok(ids)
    }

    async fn replay_durable_entries(
        &self,
        _extension_id: &str,
        _leaf_id: Option<&str>,
    ) -> Result<Vec<ExtensionDurableEntry>, SessionExtensionStateError> {
        Ok(self.entries())
    }
}

pub fn anchor_source_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("extensions")
        .join(EXTENSION_ID)
}

pub fn package_dir(project: &Path) -> PathBuf {
    project
        .join(".theway")
        .join("extensions")
        .join(EXTENSION_ID)
}

pub fn install_anchor(project: &Path, config: &Value) {
    let target = package_dir(project);
    std::fs::create_dir_all(&target).unwrap();
    for file in [
        "theway-extension.json",
        "index.js",
        "anchor-config.schema.json",
    ] {
        std::fs::copy(anchor_source_dir().join(file), target.join(file)).unwrap();
    }
    std::fs::write(
        target.join("anchor-config.json"),
        serde_json::to_vec_pretty(config).unwrap(),
    )
    .unwrap();
}

pub fn enabled_config() -> Value {
    json!({
        "$schema": "./anchor-config.schema.json",
        "providerPredicates": ["deepseek", "openai", "anthropic"],
        "modelPredicates": ["deepseek-*", "test-model"],
        "bootstrapPrompt": "ANCHOR BOOTSTRAP",
        "promotionCondition": {
            "kind": "assistant_or_tool_call",
            "textPattern": "ready",
            "toolNames": [],
        },
        "personaScope": "bootstrap_only",
        "restoredContext": RESTORED_CONTEXT,
        "maxEditorOutputChars": 16000,
        "zeroAnchor": false,
    })
}

fn permissions(values: &[&str]) -> Vec<ExtensionPermission> {
    values.iter().map(|value| value.parse().unwrap()).collect()
}

pub fn trust_anchor(project: &Path, base: &Path, grant_override: bool) {
    let requested = permissions(&[
        "session.write",
        "tools.register",
        "workspace.read",
        "workspace.write",
        "tools.override",
    ]);
    let mut granted = permissions(&[
        "session.write",
        "tools.register",
        "workspace.read",
        "workspace.write",
    ]);
    if grant_override {
        granted.push("tools.override".parse().unwrap());
    }
    let mut trust = ExtensionTrustStore::load(base);
    trust
        .decide_project(project, requested, granted, ExtensionTrustDecision::Trusted)
        .unwrap();
    trust.save().unwrap();
}

pub async fn start_anchor(
    project: &Path,
    base: &Path,
    state: Arc<dyn SessionExtensionStatePort>,
    grant_override: bool,
    config: RuntimeExtensionHostConfig,
) -> (Arc<SessionPluginHost>, QuickJsEnginePool) {
    trust_anchor(project, base, grant_override);
    let services = ExtensionBrokerServices::new(base, Arc::new(LocalExecutor::with_cwd(project)));
    let engine = QuickJsEnginePool::with_broker_services(1, Default::default(), services);
    let host = SessionPluginHost::load_with_state(
        PackageCatalog::discover(project, base),
        engine.clone(),
        SESSION_ID,
        project,
        config,
        state,
    )
    .await;
    (host, engine)
}

pub struct TestTool {
    definition: Tool,
}

impl TestTool {
    pub fn new(name: &str, parameters: Value) -> Self {
        Self {
            definition: Tool {
                name: name.into(),
                description: format!("{name} test tool"),
                parameters,
            },
        }
    }
}

#[async_trait]
impl AgentTool for TestTool {
    fn definition(&self) -> &Tool {
        &self.definition
    }

    fn label(&self) -> &str {
        &self.definition.name
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        _params: Value,
        _cancel: CancellationToken,
        _on_update: Option<AgentToolUpdate>,
    ) -> Result<AgentToolResult, AgentToolError> {
        Ok(AgentToolResult {
            content: vec![UserContentBlock::text("test")],
            ..Default::default()
        })
    }
}

pub fn compatible_bash() -> Arc<dyn AgentTool> {
    Arc::new(TestTool::new(
        "bash",
        json!({
            "type": "object",
            "properties": { "command": { "type": "string" } },
            "required": ["command"],
        }),
    ))
}

pub fn compatible_editor() -> Arc<dyn AgentTool> {
    Arc::new(TestTool::new(
        "str_replace_editor",
        json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "enum": ["view", "create", "str_replace", "insert"] },
                "path": { "type": "string" },
            },
            "required": ["command", "path"],
        }),
    ))
}

pub fn incompatible_editor() -> Arc<dyn AgentTool> {
    Arc::new(TestTool::new(
        "str_replace_editor",
        json!({
            "type": "object",
            "properties": { "command": { "type": "integer" } },
            "required": ["command"],
        }),
    ))
}

pub fn merged_tool_definitions(
    host: &SessionPluginHost,
    mut base: Vec<Arc<dyn AgentTool>>,
) -> Vec<Tool> {
    if !base.iter().any(|tool| tool.definition().name == "bash") {
        base.push(compatible_bash());
    }
    host.merge_registered_tools(base)
        .iter()
        .map(|tool| tool.definition().clone())
        .collect()
}

pub fn request_invocation(
    sequence: u64,
    provider: &str,
    model: &str,
    tools: Vec<Tool>,
    max_tokens: Option<u32>,
) -> RuntimeExtensionInvocation {
    let executable_tool_names = tools
        .iter()
        .map(|tool| tool.name.clone())
        .collect::<Vec<_>>();
    let mut context = RuntimeExtensionContext::new(SESSION_ID, "/workspace", sequence);
    context.model = Some(ExtensionModelRef {
        provider: provider.into(),
        model: model.into(),
    });
    RuntimeExtensionInvocation::new(
        ExtensionLifecycleEvent::BeforeModelRequest,
        ExtensionHookClass::Transform,
        context,
        json!({"request": {
            "provider": provider,
            "model": model,
            "systemInstructions": "BASE SYSTEM",
            "messages": [{"role": "user", "content": "retained", "timestamp": 0}],
            "visibleTools": tools,
            "executableToolNames": executable_tool_names,
            "generationOptions": {"maxTokens": max_tokens},
        }}),
    )
    .unwrap()
}

pub fn assistant_invocation(
    sequence: u64,
    provider: &str,
    model: &str,
    text: &str,
) -> RuntimeExtensionInvocation {
    let mut context = RuntimeExtensionContext::new(SESSION_ID, "/workspace", sequence);
    context.model = Some(ExtensionModelRef {
        provider: provider.into(),
        model: model.into(),
    });
    RuntimeExtensionInvocation::new(
        ExtensionLifecycleEvent::MessageEnd,
        ExtensionHookClass::Transform,
        context,
        json!({"message": {
            "role": "assistant",
            "content": [{"type": "text", "text": text}],
            "stopReason": "stop",
        }}),
    )
    .unwrap()
}

pub fn replacement(batch: &ExtensionActionBatch) -> Option<&Value> {
    batch
        .actions
        .iter()
        .find(|action| action.kind == ExtensionActionKind::ReplaceModelRequest)
        .map(|action| &action.payload["request"])
}
