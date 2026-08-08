//! Extension trait + registry for c4pt0r/theway#10 Part B. v1 ships the in-process surface:
//! a Rust-native trait that extensions implement, plus a registry that the CLI consults at
//! startup. WASM and dylib hosts plug in behind the same trait — those land as follow-up
//! commits without breaking this API.
//!
//! The shape mirrors the skills loader (#10 Part A): per-extension `name()` + `init(ctx)`
//! that returns the tools, slash-commands, lifecycle observers the extension contributes.
//! Failures during init are isolated — one bad extension can't take down the agent.

#![allow(dead_code)]

use std::sync::Arc;

use theway_core::AgentTool;

use crate::commands::SlashCommand;

/// Inputs an extension may consult during initialization.
pub struct ExtensionContext<'a> {
    pub cwd: &'a std::path::Path,
    pub session_id: &'a str,
}

/// What an extension may contribute. All vecs default to empty.
#[derive(Default)]
pub struct ExtensionContribution {
    pub tools: Vec<Arc<dyn AgentTool>>,
    pub slash_commands: Vec<Arc<dyn SlashCommand>>,
    /// Free-form per-extension banner line shown at startup. `None` suppresses.
    pub banner: Option<String>,
}

/// The extension trait. Implemented by both native Rust extensions (link-in) and (future)
/// WASM/dylib hosts that wrap the host-side handle as an `AgentExtension`.
pub trait AgentExtension: Send + Sync {
    /// Canonical name. Used for collision resolution + diagnostics.
    fn name(&self) -> &'static str;
    /// Brief one-line description for `/extensions`.
    fn description(&self) -> &'static str {
        ""
    }
    /// Build the contribution. Called once at session startup; failures are logged + the
    /// extension is skipped for that session.
    fn init(&self, ctx: &ExtensionContext<'_>) -> anyhow::Result<ExtensionContribution>;
}

/// Static registry. Extensions are added at compile time today; the WASM/dylib loader (which
/// reads `~/.theway/extensions/*` at runtime and constructs trait objects) lands in a follow-up
/// without changing this API.
pub struct ExtensionRegistry {
    extensions: Vec<Arc<dyn AgentExtension>>,
}

impl ExtensionRegistry {
    pub fn new() -> Self {
        Self {
            extensions: Vec::new(),
        }
    }

    pub fn register(&mut self, ext: Arc<dyn AgentExtension>) {
        self.extensions.push(ext);
    }

    pub fn iter(&self) -> impl Iterator<Item = &Arc<dyn AgentExtension>> {
        self.extensions.iter()
    }

    /// Init every extension, collecting their contributions. Per-extension failures emit a
    /// diagnostic in the returned `errors` vec but never abort the load.
    pub fn init_all(&self, ctx: &ExtensionContext<'_>) -> InitOutput {
        let mut tools = Vec::new();
        let mut commands = Vec::new();
        let mut banners = Vec::new();
        let mut errors = Vec::new();
        for ext in &self.extensions {
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| ext.init(ctx))) {
                Ok(Ok(c)) => {
                    tools.extend(c.tools);
                    commands.extend(c.slash_commands);
                    if let Some(b) = c.banner {
                        banners.push(format!("{}: {b}", ext.name()));
                    }
                }
                Ok(Err(e)) => {
                    errors.push(format!("{}: {e}", ext.name()));
                }
                Err(_) => {
                    errors.push(format!("{}: panicked during init", ext.name()));
                }
            }
        }
        InitOutput {
            tools,
            commands,
            banners,
            errors,
        }
    }
}

impl Default for ExtensionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

pub struct InitOutput {
    pub tools: Vec<Arc<dyn AgentTool>>,
    pub commands: Vec<Arc<dyn SlashCommand>>,
    pub banners: Vec<String>,
    pub errors: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Hello;
    impl AgentExtension for Hello {
        fn name(&self) -> &'static str {
            "hello"
        }
        fn init(&self, _ctx: &ExtensionContext<'_>) -> anyhow::Result<ExtensionContribution> {
            Ok(ExtensionContribution {
                banner: Some("ready".into()),
                ..Default::default()
            })
        }
    }
    struct Boom;
    impl AgentExtension for Boom {
        fn name(&self) -> &'static str {
            "boom"
        }
        fn init(&self, _ctx: &ExtensionContext<'_>) -> anyhow::Result<ExtensionContribution> {
            anyhow::bail!("intentional failure")
        }
    }
    struct Panicker;
    impl AgentExtension for Panicker {
        fn name(&self) -> &'static str {
            "panicker"
        }
        fn init(&self, _ctx: &ExtensionContext<'_>) -> anyhow::Result<ExtensionContribution> {
            panic!("oops")
        }
    }

    #[test]
    fn registry_isolates_failing_extension() {
        let mut r = ExtensionRegistry::new();
        r.register(Arc::new(Hello));
        r.register(Arc::new(Boom));
        r.register(Arc::new(Panicker));
        let cwd = std::env::current_dir().unwrap();
        let ctx = ExtensionContext {
            cwd: &cwd,
            session_id: "t",
        };
        let out = r.init_all(&ctx);
        assert_eq!(out.banners.len(), 1);
        assert!(out.banners[0].contains("hello"));
        assert_eq!(out.errors.len(), 2);
        assert!(out.errors.iter().any(|e| e.contains("boom")));
        assert!(out.errors.iter().any(|e| e.contains("panicker")));
    }
}
