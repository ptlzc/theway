//! `/model`, `/thinking`, `/cost` and the model-catalog help text they render.

use super::*;

pub struct ModelCommand;

#[async_trait]
impl SlashCommand for ModelCommand {
    fn name(&self) -> &'static str {
        "model"
    }
    fn description(&self) -> &'static str {
        "show or switch the active model"
    }
    fn usage(&self) -> &'static str {
        "[provider:model-id|list [provider]]"
    }
    async fn run(&self, argv: &[String], ctx: &CommandCtx<'_>) -> CommandOutcome {
        if argv.is_empty() {
            return CommandOutcome::OpenModelPicker;
        }
        if matches!(argv.first().map(|s| s.as_str()), Some("list" | "ls")) {
            let provider = argv.get(1).map(String::as_str);
            match model_catalog_text(provider) {
                Ok(text) => emit_multiline(&text),
                Err(e) => return CommandOutcome::Error(e),
            }
            return CommandOutcome::Handled;
        }
        // Accept `provider:id`, the user's natural `provider/model-id`, or two separate
        // args `provider id`.
        let spec = argv.join(" ");
        let (provider, id) = match parse_model_spec(&spec) {
            Some((p, i)) => (p.to_string(), i.to_string()),
            None => {
                return CommandOutcome::Error(
                    "expected provider:model-id (provider/model-id also works), e.g. /model anthropic:claude-haiku-4-5".into(),
                );
            }
        };
        let provider_obj = Provider::from(provider.as_str());
        let Some(model) = get_model(&provider_obj, &id) else {
            return CommandOutcome::Error(unknown_model_error(&provider, &id));
        };
        match ctx.harness.set_model(model.clone()).await {
            Ok(_) => {
                if let Some(hint) = model_credential_hint(&provider) {
                    cprintln!("selected {provider}:{id}, but login is required: {hint}");
                } else {
                    cprintln!("switched to {provider}:{id}");
                }
                CommandOutcome::Handled
            }
            Err(e) => CommandOutcome::Error(format!("set_model failed: {e}")),
        }
    }
}

pub(crate) fn parse_model_spec(spec: &str) -> Option<(&str, &str)> {
    let spec = spec.trim();
    let (provider, id) = spec
        .split_once(':')
        .or_else(|| spec.split_once('/'))
        .or_else(|| spec.split_once(char::is_whitespace))?;
    let provider = provider.trim();
    let id = id.trim();
    if provider.is_empty() || id.is_empty() {
        return None;
    }
    Some((provider, id))
}

pub(crate) fn model_credential_hint(provider: &str) -> Option<String> {
    let vars = theway_llm_provider::env_api_keys::env_var_names(provider);
    let has_env = vars.iter().any(|var| {
        std::env::var(var)
            .ok()
            .map(|v| !v.trim().is_empty())
            .unwrap_or(false)
    });
    if has_env {
        return None;
    }
    let has_stored = crate::auth::AuthStore::load()
        .ok()
        .and_then(|store| store.get(provider).cloned())
        .is_some();
    if has_stored {
        return None;
    }

    let env_hint = if vars.is_empty() {
        "set the provider API key env var".to_string()
    } else {
        format!("set {}", vars.join(" or "))
    };
    Some(format!("{env_hint} or run /login {provider}"))
}

pub struct ThinkingCommand;

#[async_trait]
impl SlashCommand for ThinkingCommand {
    fn name(&self) -> &'static str {
        "thinking"
    }
    fn description(&self) -> &'static str {
        "show or set the thinking level"
    }
    fn usage(&self) -> &'static str {
        THINKING_LEVEL_USAGE
    }
    async fn run(&self, argv: &[String], ctx: &CommandCtx<'_>) -> CommandOutcome {
        if argv.is_empty() {
            let lvl = ctx.harness.agent().state().thinking_level;
            cprintln!("thinking level: {}", lvl.map(|l| l.as_str()).unwrap_or("?"));
            return CommandOutcome::Handled;
        }
        let raw = argv[0].to_lowercase();
        let level: ThinkingLevel = match raw.parse() {
            Ok(l) => l,
            Err(e) => {
                return CommandOutcome::Error(format!("invalid level: {e}"));
            }
        };
        match ctx.harness.set_thinking_level(level).await {
            Ok(_) => {
                cprintln!("thinking level: {}", level.as_str());
                CommandOutcome::Handled
            }
            Err(e) => CommandOutcome::Error(format!("set_thinking_level failed: {e}")),
        }
    }
}

pub fn cli_model_help_text() -> String {
    let mut out = String::new();
    out.push_str("Model catalog:\n");
    for line in model_help_summary_lines() {
        out.push_str("  ");
        out.push_str(line.trim_start());
        out.push('\n');
    }
    out
}

pub(super) fn emit_multiline(text: &str) {
    for line in text.lines() {
        cprintln!("{line}");
    }
}

pub(super) fn model_help_summary_lines() -> Vec<String> {
    let groups = model_groups();
    let total = groups.values().map(Vec::len).sum::<usize>();
    vec![
        format!(
            "  Supported providers ({}), models ({}): {}",
            groups.len(),
            total,
            provider_summary(&groups)
        ),
        "  Full list: /help models or /model list [provider]".into(),
        "  Custom models: ~/.theway/models.json and <cwd>/.theway/models.json".into(),
        "  Credentials: set provider env vars or run /login <provider>.".into(),
    ]
}

pub(super) fn model_catalog_text(provider_filter: Option<&str>) -> Result<String, String> {
    let groups = model_groups();
    let total = groups.values().map(Vec::len).sum::<usize>();
    let mut out = Vec::new();
    match provider_filter {
        Some(provider) => {
            let Some(models) = groups.get(provider) else {
                return Err(unknown_provider_error(provider, &groups));
            };
            out.push(format!(
                "Supported models for provider '{provider}' ({}):",
                models.len()
            ));
            append_model_lines(&mut out, models);
        }
        None => {
            out.push(format!(
                "Supported providers/models: {} providers, {} models",
                groups.len(),
                total
            ));
            out.push(
                "Custom models are loaded from ~/.theway/models.json and <cwd>/.theway/models.json."
                    .into(),
            );
            for (provider, models) in &groups {
                out.push(format!("  {provider} ({})", models.len()));
                append_model_lines(&mut out, models);
            }
        }
    }
    Ok(out.join("\n"))
}

pub(super) fn model_groups() -> BTreeMap<String, Vec<Model>> {
    let mut groups: BTreeMap<String, Vec<Model>> = BTreeMap::new();
    for model in list_models() {
        groups
            .entry(model.provider.0.clone())
            .or_default()
            .push(model);
    }
    for models in groups.values_mut() {
        models.sort_by(|a, b| a.id.cmp(&b.id));
    }
    groups
}

fn provider_summary(groups: &BTreeMap<String, Vec<Model>>) -> String {
    groups
        .iter()
        .map(|(provider, models)| format!("{provider}({})", models.len()))
        .collect::<Vec<_>>()
        .join(", ")
}

fn append_model_lines(out: &mut Vec<String>, models: &[Model]) {
    for model in models {
        if model.name.trim().is_empty() || model.name == model.id {
            out.push(format!("    - {}", model.id));
        } else {
            out.push(format!("    - {} — {}", model.id, model.name));
        }
    }
}

pub(super) fn unknown_provider_error(
    provider: &str,
    groups: &BTreeMap<String, Vec<Model>>,
) -> String {
    format!(
        "unknown provider '{provider}'. Known providers: {}",
        provider_summary(groups)
    )
}

pub(super) fn unknown_model_error(provider: &str, id: &str) -> String {
    let groups = model_groups();
    let Some(models) = groups.get(provider) else {
        return unknown_provider_error(provider, &groups);
    };
    let candidates = models
        .iter()
        .take(12)
        .map(|m| m.id.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let more = if models.len() > 12 {
        format!(
            "; run /model list {provider} for all {} models",
            models.len()
        )
    } else {
        String::new()
    };
    format!("unknown model in catalog: {provider}:{id}. Candidates: {candidates}{more}")
}

pub struct CostCommand;

#[async_trait]
impl SlashCommand for CostCommand {
    fn name(&self) -> &'static str {
        "cost"
    }
    fn description(&self) -> &'static str {
        "show running token / USD totals for this session"
    }
    fn usage(&self) -> &'static str {
        "[reset]"
    }
    async fn run(&self, argv: &[String], ctx: &CommandCtx<'_>) -> CommandOutcome {
        if argv.first().map(|s| s.as_str()) == Some("reset") {
            ctx.harness.reset_cost();
            cprintln!("cost counters reset");
            return CommandOutcome::Handled;
        }
        let snap = ctx.harness.cost();
        cprintln!("{}", theway_core::cost_full_breakdown(&snap));
        CommandOutcome::Handled
    }
}
