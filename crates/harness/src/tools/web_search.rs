//! `web_search` tool. Pluggable backend; v1 ships Brave Search via env credential
//! `BRAVE_SEARCH_API_KEY`. Returns ranked results as formatted text.
//!
//! When the backend is unavailable (no API key or empty results), the tool returns a clear
//! error so the LLM knows to fall back to web_fetch against a known URL.

use std::env;
use std::time::Duration;

use async_trait::async_trait;
use once_cell::sync::Lazy;
use serde::Deserialize;
use serde_json::{Value, json};
use theway_core::{AgentTool, AgentToolError, AgentToolResult, AgentToolUpdate, ToolExecutionMode};
use theway_llm_provider::{Tool, UserContentBlock};
use tokio_util::sync::CancellationToken;

const TIMEOUT_SECS: u64 = 15;
const DEFAULT_RESULTS: usize = 10;
const MAX_RESULTS: usize = 20;

pub struct WebSearchTool {
    base_url: String,
}

impl WebSearchTool {
    /// Default constructor uses Brave Search's production URL. Tests inject a different base
    /// to point at a mock server.
    pub fn new() -> Self {
        Self {
            base_url: "https://api.search.brave.com/res/v1/web/search".into(),
        }
    }

    /// Override the backend URL — used by tests against a local mock server. Production
    /// callers stick with `new()`.
    #[allow(dead_code)]
    pub fn with_base_url(url: String) -> Self {
        Self { base_url: url }
    }
}

impl Default for WebSearchTool {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Deserialize)]
struct BraveResponse {
    #[serde(default)]
    web: Option<BraveWeb>,
}

#[derive(Deserialize)]
struct BraveWeb {
    #[serde(default)]
    results: Vec<BraveResult>,
}

#[derive(Deserialize)]
struct BraveResult {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

#[async_trait]
impl AgentTool for WebSearchTool {
    fn definition(&self) -> &Tool {
        &DEFINITION
    }
    fn label(&self) -> &str {
        "web_search"
    }
    fn execution_mode(&self) -> Option<ToolExecutionMode> {
        Some(ToolExecutionMode::Parallel)
    }

    async fn execute(
        &self,
        _id: &str,
        params: Value,
        cancel: CancellationToken,
        _on_update: Option<AgentToolUpdate>,
    ) -> Result<AgentToolResult, AgentToolError> {
        let query = params
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentToolError::Message("missing required arg: query".into()))?
            .to_string();
        let count = params
            .get("count")
            .and_then(|v| v.as_u64())
            .map(|n| (n as usize).clamp(1, MAX_RESULTS))
            .unwrap_or(DEFAULT_RESULTS);

        let api_key = env::var("BRAVE_SEARCH_API_KEY").map_err(|_| {
            AgentToolError::Message(
                "web_search backend not configured: set BRAVE_SEARCH_API_KEY env var".into(),
            )
        })?;

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(TIMEOUT_SECS))
            .user_agent(format!("theway/{}", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| AgentToolError::Message(format!("http client init: {e}")))?;

        let req = client
            .get(&self.base_url)
            .header("X-Subscription-Token", api_key)
            .header("Accept", "application/json")
            .query(&[("q", query.as_str()), ("count", &count.to_string())]);

        let resp = tokio::select! {
            r = req.send() => r.map_err(|e| AgentToolError::Message(format!("search failed: {e}")))?,
            _ = cancel.cancelled() => {
                return Err(AgentToolError::Message("cancelled".into()));
            }
        };
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| AgentToolError::Message(format!("read body: {e}")))?;
        if !status.is_success() {
            return Err(AgentToolError::Message(format!(
                "search backend status {status}: {}",
                text.chars().take(500).collect::<String>()
            )));
        }
        let parsed: BraveResponse = serde_json::from_str(&text)
            .map_err(|e| AgentToolError::Message(format!("parse response: {e}")))?;
        let results = parsed.web.map(|w| w.results).unwrap_or_default();

        if results.is_empty() {
            return Ok(AgentToolResult {
                content: vec![UserContentBlock::text(format!(
                    "no results for query: {query}"
                ))],
                details: json!({ "query": query, "results": 0 }),
                terminate: None,
            });
        }

        let mut body = format!(
            "web_search {query:?} — top {} of {}:\n\n",
            results.len(),
            count
        );
        for (i, r) in results.iter().enumerate() {
            let title = r.title.as_deref().unwrap_or("(no title)");
            let url = r.url.as_deref().unwrap_or("(no url)");
            let desc = r.description.as_deref().unwrap_or("");
            body.push_str(&format!("{}. {title}\n   {url}\n", i + 1));
            if !desc.is_empty() {
                body.push_str(&format!("   {desc}\n"));
            }
            body.push('\n');
        }

        Ok(AgentToolResult {
            content: vec![UserContentBlock::text(body)],
            details: json!({
                "query": query,
                "results": results.len(),
            }),
            terminate: None,
        })
    }
}

static DEFINITION: Lazy<Tool> = Lazy::new(|| {
    Tool {
    name: "web_search".into(),
    description: "Search the web. v1 backend: Brave Search. Requires BRAVE_SEARCH_API_KEY env var. Returns ranked results with title, URL, and description.".into(),
    parameters: json!({
        "type": "object",
        "properties": {
            "query": {
                "type": "string",
                "description": "Free-text search query.",
            },
            "count": {
                "type": "integer",
                "description": "How many results to request (1-20, default 10).",
            },
        },
        "required": ["query"],
        "additionalProperties": false,
    }),
}
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definition_lists_query_as_required() {
        let def = WebSearchTool::new().definition().clone();
        let req = def
            .parameters
            .get("required")
            .and_then(|v| v.as_array())
            .unwrap();
        assert!(req.iter().any(|v| v.as_str() == Some("query")));
    }
}
