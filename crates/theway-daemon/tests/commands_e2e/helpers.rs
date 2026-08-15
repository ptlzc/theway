//! Shared helpers for the slash-command e2e suites: faux model + stream builders,
//! skill fixtures, reloadable-skill harnesses, output capture, env guards, fake
//! `gh` shims, and the process-wide serialization locks.
//!
//! Everything here is `pub` so the sibling domain modules can glob-import it
//! (`use super::helpers::*;`). The `commands` / `skill_overrides` names refer to
//! the `#[path]`-included `src/` modules at the test-crate root (see
//! `commands_e2e_main.rs`).

use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;

use theway_core::{
    AgentHarness, AgentHarnessOptions, ControlPlanePromptDecision, LoadSkillsOutput,
    MemorySessionStorage, OnControlPlanePromptHook, ReloadSkillsFn, Session, SessionStorage, Skill,
    SkillSource,
};
use theway_llm_provider::{
    AssistantMessage, AssistantMessageEvent, AssistantMessageEventStream, AssistantRole,
    ContentBlock, Context, DoneReason, Message, StopReason, ToolCall, Usage,
};

use crate::commands;
use crate::skill_overrides;

pub static GH_BIN_ENV_LOCK: Mutex<()> = Mutex::new(());
pub static THEWAY_DIR_ENV_LOCK: Mutex<()> = Mutex::new(());
pub static DYNAMIC_TRIGGER_LOCK: Mutex<()> = Mutex::new(());
pub static CRON_LOCK: Mutex<()> = Mutex::new(());

/// Auto-approve `on_control_plane_prompt` for tests that exercise a tool whose
/// `permission_classification` returns `Prompt` (issue #110 sub-PR 3 — `NewTriggerTool`,
/// `RemoveTriggerTool`, `SetTriggerStateTool` enable, `InstallSkillTool`, `RemoveSkillTool`,
/// `SetSkillStateTool` enable). Without this, the harness defaults to fail-closed deny and
/// the tool's `execute` never runs. Production behavior is gated on the embedder's real
/// prompt card; these integration tests focus on the post-approval code path.
pub fn allow_all_control_plane_hook() -> OnControlPlanePromptHook {
    use std::sync::Arc;
    Arc::new(|_req, _cancel| Box::pin(async { ControlPlanePromptDecision::Allow }))
}

pub fn faux_model() -> theway_llm_provider::Model {
    theway_llm_provider::Model {
        id: "faux".into(),
        name: "Faux".into(),
        api: theway_llm_provider::Api::from("faux"),
        provider: theway_llm_provider::Provider::from("faux"),
        base_url: String::new(),
        reasoning: false,
        thinking_level_map: None,
        input: vec![],
        cost: theway_llm_provider::ModelCost::default(),
        context_window: 0,
        max_tokens: 0,
        headers: None,
        compat: None,
    }
}

pub fn new_trigger_extraction_stream() -> theway_core::StreamFn {
    Arc::new(|_, context: &Context, _| {
        let has_tool_result = context
            .messages
            .iter()
            .any(|m| matches!(m, Message::ToolResult(_)));
        let message = if has_tool_result {
            assistant_text("created")
        } else {
            assistant_tool_call(
                "call-new-trigger",
                "new_trigger",
                serde_json::json!({
                    "condition": "\u{73b0}\u{5728}\u{662f} 11pm",
                    "action": "\u{5199}\u{4e00}\u{4e2a} tmp \u{6587}\u{4ef6}",
                }),
            )
        };
        stream_one(message)
    })
}

pub fn stream_one(message: AssistantMessage) -> AssistantMessageEventStream {
    let (stream, mut sender) = AssistantMessageEventStream::new();
    tokio::spawn(async move {
        sender.push(AssistantMessageEvent::Start {
            partial: message.clone(),
        });
        sender.push(AssistantMessageEvent::Done {
            reason: match message.stop_reason {
                StopReason::ToolUse => DoneReason::ToolUse,
                _ => DoneReason::Stop,
            },
            message,
        });
    });
    stream
}

pub fn assistant_tool_call(id: &str, name: &str, args: serde_json::Value) -> AssistantMessage {
    let arguments = args.as_object().cloned().unwrap_or_default();
    assistant(vec![ContentBlock::ToolCall(ToolCall {
        id: id.into(),
        name: name.into(),
        arguments,
        thought_signature: None,
    })])
}

pub fn assistant_text(text: &str) -> AssistantMessage {
    assistant(vec![ContentBlock::text(text)])
}

pub fn assistant(content: Vec<ContentBlock>) -> AssistantMessage {
    let stop_reason = if content
        .iter()
        .any(|block| matches!(block, ContentBlock::ToolCall(_)))
    {
        StopReason::ToolUse
    } else {
        StopReason::Stop
    };
    AssistantMessage {
        role: AssistantRole::Assistant,
        content,
        api: theway_llm_provider::Api::from("faux"),
        provider: theway_llm_provider::Provider::from("faux"),
        model: "faux".into(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason,
        error_message: None,
        timestamp: 0,
    }
}

pub fn skill(name: &str, content: &str, disabled: bool) -> Skill {
    Skill {
        name: name.into(),
        description: format!("description for {name}"),
        file_path: format!("/tmp/project/.theway/skills/{name}/SKILL.md"),
        content: content.into(),
        disable_model_invocation: disabled,
        source: SkillSource::User,
    }
}

pub fn user_skill_at(base_dir: &Path, name: &str, disabled: bool) -> Skill {
    Skill {
        name: name.into(),
        description: format!("description for {name}"),
        file_path: base_dir
            .join("skills")
            .join(name)
            .join("SKILL.md")
            .to_string_lossy()
            .to_string(),
        content: format!("SECRET SKILL BODY for {name}"),
        disable_model_invocation: disabled,
        source: SkillSource::User,
    }
}

pub fn harness_with_reloadable_skills(base_dir: &Path, seed: Vec<Skill>) -> Arc<AgentHarness> {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let source = Arc::new(Mutex::new(seed.clone()));
    let base = base_dir.to_path_buf();
    let loader_source = source.clone();
    let loader: ReloadSkillsFn = Arc::new(move || {
        let source = loader_source.clone();
        let base = base.clone();
        Box::pin(async move {
            let mut skills = source.lock().unwrap().clone();
            let state = skill_overrides::load(&base).await;
            skill_overrides::apply(&state, &mut skills);
            LoadSkillsOutput {
                skills,
                diagnostics: Vec::new(),
            }
        })
    });
    let mut opts = AgentHarnessOptions::new(faux_model(), session);
    opts.skills = seed;
    opts.reload_skills_fn = Some(loader);
    Arc::new(AgentHarness::new(opts))
}

pub fn harness_with_disk_skill_reload(base_dir: &Path, seed: Vec<Skill>) -> Arc<AgentHarness> {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let base = base_dir.to_path_buf();
    let loader: ReloadSkillsFn = Arc::new(move || {
        let base = base.clone();
        Box::pin(async move {
            let env = theway_daemon::env::native::NativeEnv::new(
                std::env::current_dir()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_default(),
            );
            let skills_dir = base.join("skills");
            let mut out = theway_core::load_skills(
                &env,
                &[skills_dir.to_string_lossy().as_ref()],
                tokio_util::sync::CancellationToken::new(),
            )
            .await;
            for skill in out.skills.iter_mut() {
                skill.source = SkillSource::User;
            }
            let state = skill_overrides::load(&base).await;
            skill_overrides::apply(&state, &mut out.skills);
            out
        })
    });
    let mut opts = AgentHarnessOptions::new(faux_model(), session);
    opts.skills = seed;
    opts.reload_skills_fn = Some(loader);
    Arc::new(AgentHarness::new(opts))
}

pub static COMMAND_OUTPUT_LOCK: Mutex<()> = Mutex::new(());

pub struct OutputCapture {
    lines: Arc<Mutex<Vec<String>>>,
}

impl OutputCapture {
    pub fn install() -> Self {
        let lines = Arc::new(Mutex::new(Vec::new()));
        let sink_lines = lines.clone();
        commands::console::set_sink(Box::new(move |line| {
            sink_lines.lock().unwrap().push(line);
        }));
        Self { lines }
    }

    pub fn text(&self) -> String {
        self.lines.lock().unwrap().join("\n")
    }
}

impl Drop for OutputCapture {
    fn drop(&mut self) {
        commands::console::clear_sink();
    }
}

// The path-include duplicates the module, so we silence the dead-code warning about helpers
// that only the binary calls.
#[allow(dead_code)]
pub fn _path_check(_p: &Path) {}

/// Body of the fake `gh` shim that records argv and prints a gist URL.
/// POSIX script on Unix; `.bat` on Windows — `Command` only executes `.bat`/
/// `.cmd` when given the full path, which is why the tests drive the shim
/// through `THEWAY_GH_BIN` instead of PATH (Windows PATH search never finds
/// extensionless scripts or `.bat` files).
pub fn fake_gh_argv_and_url(argv_log: &Path, url: &str) -> String {
    #[cfg(unix)]
    {
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" > '{}'\nprintf '%s\\n' '{url}'\n",
            argv_log.display()
        )
    }
    #[cfg(windows)]
    {
        format!(
            "@echo off\r\necho %* > \"{}\"\r\necho {url}\r\n",
            argv_log.display()
        )
    }
}

/// Body of the fake `gh` shim that fails on stderr with exit code 1.
pub fn fake_gh_fail_stderr() -> String {
    #[cfg(unix)]
    {
        "#!/bin/sh\nprintf '%s\\n' 'unknown flag: --secret' >&2\nexit 1\n".to_string()
    }
    #[cfg(windows)]
    {
        "@echo off\r\necho unknown flag: --secret 1>&2\r\nexit /b 1\r\n".to_string()
    }
}

/// Write the fake `gh` shim into `dir` and return its full path (spawned via
/// `THEWAY_GH_BIN`).
pub fn write_fake_gh(dir: &Path, body: &str) -> std::path::PathBuf {
    #[cfg(unix)]
    {
        let path = dir.join("gh");
        std::fs::write(&path, body).unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&path, permissions).unwrap();
        path
    }
    #[cfg(windows)]
    {
        let path = dir.join("gh.cmd");
        std::fs::write(&path, body).unwrap();
        path
    }
}

pub struct EnvGuard {
    key: &'static str,
    original: Option<std::ffi::OsString>,
}

impl EnvGuard {
    pub fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let original = std::env::var_os(key);
        unsafe { std::env::set_var(key, value) };
        Self { key, original }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match self.original.take() {
            Some(value) => unsafe { std::env::set_var(self.key, value) },
            None => unsafe { std::env::remove_var(self.key) },
        }
    }
}
