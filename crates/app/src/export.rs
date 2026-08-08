//! Session-to-Markdown export. Walks the session jsonl active branch and renders a
//! human-readable transcript: one heading per message kind, code-fenced tool I/O, model and
//! thinking-level changes annotated inline.
//!
//! Used by `/save` and (in a follow-up) `/share` once we add a paste backend.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use theway_core::{AgentMessage, Session, SessionContext};

use crate::config::base_dir;

pub async fn render(session: &Session) -> Result<String> {
    let ctx = session.build_context().await?;
    Ok(render_context(&ctx))
}

pub fn render_context(ctx: &SessionContext) -> String {
    let mut out = String::new();
    out.push_str("# Session Transcript\n\n");
    if let Some(model) = &ctx.model {
        out.push_str(&format!(
            "- Model: `{}:{}`\n",
            model.provider, model.model_id
        ));
    }
    out.push_str(&format!("- Thinking level: `{}`\n", ctx.thinking_level));
    out.push_str(&format!("- Messages: {}\n", ctx.messages.len()));
    out.push('\n');

    for (i, m) in ctx.messages.iter().enumerate() {
        match m {
            AgentMessage::Llm(theway_llm_provider::Message::User(u)) => {
                out.push_str(&format!("## {i}. User\n\n"));
                out.push_str(&render_user_content(&u.content));
                out.push_str("\n\n");
            }
            AgentMessage::Llm(theway_llm_provider::Message::Assistant(a)) => {
                out.push_str(&format!("## {i}. Assistant\n\n"));
                for block in &a.content {
                    match block {
                        theway_llm_provider::ContentBlock::Text(t) => {
                            out.push_str(&t.text);
                            out.push_str("\n\n");
                        }
                        theway_llm_provider::ContentBlock::Thinking(t) => {
                            out.push_str("<details><summary>thinking</summary>\n\n");
                            out.push_str(&format!("```\n{}\n```\n", t.thinking));
                            out.push_str("\n</details>\n\n");
                        }
                        theway_llm_provider::ContentBlock::ToolCall(c) => {
                            out.push_str(&format!(
                                "**tool call** `{}` `{}`:\n```json\n{}\n```\n\n",
                                c.name,
                                c.id,
                                serde_json::Value::Object(c.arguments.clone())
                            ));
                        }
                        theway_llm_provider::ContentBlock::Image(_) => {
                            out.push_str("`[image]`\n\n");
                        }
                    }
                }
            }
            AgentMessage::Llm(theway_llm_provider::Message::ToolResult(t)) => {
                out.push_str(&format!("### tool result `{}`\n\n", t.tool_call_id));
                out.push_str(&render_user_content(
                    &theway_llm_provider::UserContent::Blocks(t.content.clone()),
                ));
                out.push_str("\n\n");
            }
            AgentMessage::Custom(c) => {
                out.push_str(&format!(
                    "### custom: {}\n\n```json\n{}\n```\n\n",
                    c.role,
                    serde_json::to_string_pretty(&c.payload).unwrap_or_default()
                ));
            }
        }
    }
    out
}

fn render_user_content(content: &theway_llm_provider::UserContent) -> String {
    match content {
        theway_llm_provider::UserContent::Text(s) => s.clone(),
        theway_llm_provider::UserContent::Blocks(blocks) => blocks
            .iter()
            .map(|b| match b {
                theway_llm_provider::UserContentBlock::Text(t) => t.text.clone(),
                theway_llm_provider::UserContentBlock::Image(_) => "`[image]`".into(),
            })
            .collect::<Vec<_>>()
            .join("\n\n"),
    }
}

pub fn default_export_path(session_id: &str) -> PathBuf {
    base_dir().join("exports").join(format!("{session_id}.md"))
}

pub async fn save(session: &Session, dest: &Path) -> Result<PathBuf> {
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("create exports dir {}", parent.display()))?;
    }
    let body = render(session).await?;
    tokio::fs::write(dest, body)
        .await
        .with_context(|| format!("write {}", dest.display()))?;
    Ok(dest.to_path_buf())
}
