use crate::agent::AgentRunError;
use crate::agent::system_prompt::format_skills_for_system_prompt;
use crate::agent::types::{PromptTemplate, Skill};

use super::{AgentHarness, ReloadSkillsError, SessionEvent};
use std::sync::Arc;

impl AgentHarness {
    pub fn skills(&self) -> Vec<Skill> {
        self.skills.lock().clone()
    }

    /// Snapshot of the loaded prompt templates. Listing-only — callers run them via
    /// [`Self::prompt_from_template`].
    pub fn templates(&self) -> Vec<PromptTemplate> {
        self.templates.lock().clone()
    }

    pub fn system_prompt(&self) -> String {
        self.agent.state().system_prompt.clone()
    }

    /// Replace the skill catalog. Rebuilds the system prompt so the in-flight Agent state has
    /// the new `<skills>` block on its next LLM call.
    pub fn replace_skills(&self, skills: Vec<Skill>) {
        *self.skills.lock() = skills;
        let prompt = build_system_prompt(&self.base_system_prompt, &self.skills.lock());
        self.agent.state().system_prompt = prompt;
    }

    /// Replace a previously provisioned set of MCP tools. Every currently-loaded tool whose
    /// `Arc` pointer matches one in `old` (via [`Arc::ptr_eq`]) is removed from the live
    /// harness tool set, then `new` is appended. Used by MCP re-provisioning so that a
    /// reconfigure swaps out exactly the tools that were connected before without disturbing
    /// built-in harness tools. Unlike `replace_skills` no system-prompt rebuild is needed —
    /// tools live on the [`crate::agent::AgentState`], not in the `<skills>` block.
    ///
    /// Name collisions are resolved here because the LLM rejects duplicate tool names
    /// outright (DeepSeek: HTTP 400 "Tool names must be unique"), which breaks every
    /// turn. Built-in harness tools win over provisioned MCP tools; among the
    /// provisioned batch itself, the earlier server wins. Dropped duplicates are
    /// reported so operators can see a server's tool never made it in.
    pub fn replace_mcp_tools(
        &self,
        old: &[Arc<dyn crate::AgentTool>],
        new: Vec<Arc<dyn crate::AgentTool>>,
    ) {
        let mut tools = std::mem::take(&mut self.agent.state().tools);
        tools.retain(|t| !old.iter().any(|o| Arc::ptr_eq(o, t)));
        let mut seen: std::collections::HashSet<String> =
            tools.iter().map(|t| t.definition().name.clone()).collect();
        let mut dropped: Vec<String> = Vec::new();
        for tool in new {
            let name = tool.definition().name.clone();
            if seen.insert(name.clone()) {
                tools.push(tool);
            } else {
                dropped.push(name);
            }
        }
        if !dropped.is_empty() {
            tracing::warn!(
                "dropped MCP tool(s) with duplicate names (built-ins and earlier servers win): {}",
                dropped.join(", ")
            );
        }
        self.agent.state().tools = tools;
    }

    /// Replace the prompt-template catalog (issue #96). Templates do not feed the system
    /// prompt, so — unlike replace_skills — no system-prompt rebuild is needed.
    pub fn replace_templates(&self, templates: Vec<PromptTemplate>) {
        *self.templates.lock() = templates;
    }

    /// Hot-reload the skill catalog from disk via the embedder-supplied
    /// [`super::AgentHarnessOptions::reload_skills_fn`] closure. Used by `InstallSkillTool`,
    /// `/skills reload`, and any future control-plane that needs to refresh the catalog
    /// after a filesystem write — they all share the same source directories + dedup
    /// policy as startup because they go through the same closure.
    ///
    /// Returns the loader's [`crate::agent::skills::LoadSkillsOutput`] (skills + per-skill
    /// diagnostics) so the caller can surface a summary to the user. On success the new
    /// catalog has already been applied via [`Self::replace_skills`] and the system prompt
    /// rebuilt — the next prompt will see the new `<skills>` block. In-flight turns
    /// continue against their existing context (no mid-turn prompt mutation).
    ///
    /// Errors with [`ReloadSkillsError::NotConfigured`] if no loader was wired at
    /// construction — embedders that don't need reload simply leave `reload_skills_fn` as
    /// `None` and use [`Self::replace_skills`] directly.
    pub async fn reload_skills_from_disk(
        &self,
    ) -> Result<crate::agent::skills::LoadSkillsOutput, ReloadSkillsError> {
        let loader = self
            .reload_skills_fn
            .as_ref()
            .ok_or(ReloadSkillsError::NotConfigured)?
            .clone();
        let out = loader().await;
        self.replace_skills(out.skills.clone());
        self.emit_harness_event(SessionEvent::SkillsReloaded {
            total: out.skills.len(),
        });
        Ok(out)
    }

    /// Pick a template by name, interpolate, and prompt the agent.
    pub async fn prompt_from_template(
        &self,
        name: &str,
        vars: serde_json::Map<String, serde_json::Value>,
    ) -> Result<(), AgentRunError> {
        let template = {
            let g = self.templates.lock();
            g.iter().find(|t| t.name == name).cloned()
        };
        let template = match template {
            Some(t) => t,
            None => {
                return Err(AgentRunError::Other(format!(
                    "unknown prompt template: {name}"
                )));
            }
        };
        let rendered = template.interpolate(&vars);
        self.prompt(rendered).await
    }
}

pub(super) fn build_system_prompt(base: &str, skills: &[Skill]) -> String {
    let skills_block = format_skills_for_system_prompt(skills);
    if base.is_empty() {
        return skills_block;
    }
    if skills_block.is_empty() {
        return base.to_string();
    }
    format!("{base}\n\n{skills_block}")
}
