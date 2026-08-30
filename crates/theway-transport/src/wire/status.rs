#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct WireContextUsage {
    pub cached_tokens: u64,
    pub new_tokens: u64,
    pub total_input_tokens: u64,
    pub output_tokens: u64,
    pub cache_write_tokens: u64,
    /// Provider-reported cache read ratio; `None` when the provider does not
    /// report cache read tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_cache_hit_rate: Option<f64>,
    /// Client-side longest-common-prefix estimate; `None` before the first
    /// request baseline is available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix_cache_hit_rate: Option<f64>,
    /// Prefix-overlap token estimate used to aggregate session-cumulative
    /// prefix hit metrics.
    #[serde(default)]
    pub prefix_hit_tokens: u64,
    pub context_window: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct WireStatus {
    pub session_id: String,
    pub model: String,
    /// Active thinking level ("off" | "minimal" | "low" | "medium" | "high" |
    /// "xhigh"); empty when the daemon reports none.
    #[serde(default)]
    pub thinking_level: String,
    pub model_catalog: Vec<ProviderGroup>,
    pub cwd: String,
    pub busy: bool,
    pub queued_count: usize,
    pub latest_trigger_poll: Option<crate::feed::TriggerPollStatus>,
    pub goal: Option<WireGoalSnapshot>,
    pub control_plane_prompt: Option<WireControlPlanePromptSnapshot>,
    pub sidebar: WireSidebarSnapshot,
    pub feed_blocks: Vec<crate::feed::WireFeedBlock>,
    /// Required consumer block count before applying
    /// [`Self::feed_block_patches`]. Zero with no patches is a full frame.
    #[serde(default)]
    pub feed_blocks_base: u64,
    /// Incremental block appends/replacements for gRPC stream consumers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub feed_block_patches: Vec<WireFeedBlockPatch>,
    pub feed_lines: Vec<String>,
    /// Absolute index of `feed_lines[0]` in a gRPC incremental stream frame.
    /// Authoritative `WireStatus` snapshots keep this at zero and carry every
    /// row; per-client stream projection applies the non-zero cursor.
    #[serde(default)]
    pub feed_lines_base: u64,
    pub dags: Vec<WireDagRunSnapshot>,
    pub subagents: Vec<WireAgentJobSnapshot>,
    /// Running token usage + the active model's context window, published by
    /// the daemon for the TUI prompt chrome (context-usage indicator).
    #[serde(default)]
    pub usage: WireContextUsage,
    /// Session-cumulative token usage: total input, cached input, non-cached
    /// input, output, and cache write totals for the current session.
    #[serde(default)]
    pub session_usage: WireContextUsage,
    /// TUI display settings resolved by the daemon from `config.toml`
    /// (`[tui] max_feed_lines`); `None` → the TUI built-in default applies.
    pub tui_max_feed_lines: Option<u64>,
    /// Structured runtime-extension state. Routine extension activity updates
    /// this plane without adding conversation feed blocks.
    #[serde(default, skip_serializing_if = "WireExtensionSnapshot::is_empty")]
    pub extensions: WireExtensionSnapshot,
    /// Full rendered system context for the next request: base prompt + skills
    /// + tool inventory + working directory + memory + lineage.
    ///
    /// Mirrors the request/header epoch snapshot in deepseek-harness session
    /// logs. Empty for older daemons.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub system_context: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct WireFeedBlockPatch {
    pub index: u64,
    pub block: crate::feed::WireFeedBlock,
}

#[derive(Clone, Debug)]
pub struct WireFeedDelta {
    pub feed_blocks_base: u64,
    pub feed_block_patches: Vec<WireFeedBlockPatch>,
    pub feed_blocks_len: usize,
    pub feed_lines_base: u64,
    pub feed_lines: Vec<String>,
    pub feed_lines_len: usize,
}

/// One daemon publication: either a full authoritative status (for startup,
/// metadata changes, and resync) or a transcript-only delta for streaming.
#[derive(Clone, Debug)]
pub enum WireStatusUpdate {
    Full(WireStatus),
    Delta(WireFeedDelta),
}

impl WireStatusUpdate {
    pub fn full(mut status: WireStatus) -> Self {
        status.feed_blocks_base = 0;
        status.feed_block_patches.clear();
        status.feed_lines_base = 0;
        Self::Full(status)
    }

    pub fn delta(
        feed_blocks_base: u64,
        feed_block_patches: Vec<WireFeedBlockPatch>,
        feed_blocks_len: usize,
        feed_lines_base: u64,
        feed_lines: Vec<String>,
        feed_lines_len: usize,
    ) -> Self {
        Self::Delta(WireFeedDelta {
            feed_blocks_base,
            feed_block_patches,
            feed_blocks_len,
            feed_lines_base,
            feed_lines,
            feed_lines_len,
        })
    }

    #[cfg(test)]
    pub(crate) fn delta_from_status(
        mut status: WireStatus,
        feed_blocks_len: usize,
        feed_lines_len: usize,
    ) -> Self {
        Self::delta(
            status.feed_blocks_base,
            std::mem::take(&mut status.feed_block_patches),
            feed_blocks_len,
            status.feed_lines_base,
            std::mem::take(&mut status.feed_lines),
            feed_lines_len,
        )
    }

    pub fn full_status(&self) -> Option<&WireStatus> {
        match self {
            Self::Full(status) => Some(status),
            Self::Delta(_) => None,
        }
    }

    pub fn feed_delta(&self) -> Option<&WireFeedDelta> {
        match self {
            Self::Full(_) => None,
            Self::Delta(delta) => Some(delta),
        }
    }

    /// Apply an incremental publication to a cached authoritative snapshot.
    /// Returns `false` on a cursor/kind gap so the producer can fall back to a
    /// freshly built full snapshot.
    pub fn apply_to(&self, latest: &mut WireStatus) -> bool {
        let delta = match self {
            Self::Full(status) => {
                *latest = status.clone();
                return true;
            }
            Self::Delta(delta) => delta,
        };

        let Ok(block_base) = usize::try_from(delta.feed_blocks_base) else {
            return false;
        };
        let reset_blocks = block_base == 0;
        if !reset_blocks && block_base != latest.feed_blocks.len() {
            return false;
        }
        let mut projected_len = block_base;
        let mut appended = Vec::new();
        for patch in &delta.feed_block_patches {
            let Ok(index) = usize::try_from(patch.index) else {
                return false;
            };
            if index > projected_len {
                return false;
            }
            let existing = if index < block_base {
                latest.feed_blocks.get(index)
            } else if index < projected_len {
                appended.get(index - block_base).copied()
            } else {
                None
            };
            if let Some(existing) = existing
                && std::mem::discriminant(existing) != std::mem::discriminant(&patch.block)
            {
                return false;
            }
            if index == projected_len {
                appended.push(&patch.block);
                projected_len += 1;
            }
        }
        if projected_len != delta.feed_blocks_len {
            return false;
        }

        let Ok(line_base) = usize::try_from(delta.feed_lines_base) else {
            return false;
        };
        if line_base > latest.feed_lines.len()
            || line_base + delta.feed_lines.len() != delta.feed_lines_len
        {
            return false;
        }

        if reset_blocks {
            latest.feed_blocks.clear();
        }
        for patch in &delta.feed_block_patches {
            let index = patch.index as usize;
            if index == latest.feed_blocks.len() {
                latest.feed_blocks.push(patch.block.clone());
            } else {
                latest.feed_blocks[index] = patch.block.clone();
            }
        }
        latest.feed_lines.truncate(line_base);
        latest.feed_lines.extend(delta.feed_lines.iter().cloned());
        true
    }
}

impl From<WireStatus> for WireStatusUpdate {
    fn from(status: WireStatus) -> Self {
        Self::full(status)
    }
}

impl From<&WireStatus> for WireSessionSnapshot {
    fn from(status: &WireStatus) -> Self {
        let (provider, model) = split_model_spec(&status.model);
        Self {
            session_id: status.session_id.clone(),
            info: WireSessionInfo {
                id: status.session_id.clone(),
                name: String::new(),
                cwd: status.cwd.clone(),
                created_at: String::new(),
                last_activity_at: 0,
                last_activity_at_rfc3339: None,
                busy: status.busy,
                preview: None,
                metadata: HashMap::new(),
                graph_count: 0,
                active_graph_count: 0,
                queued_count: status.queued_count,
                sidebar: status.sidebar.clone(),
            },
            runtime: WireSessionRuntime {
                model: WireModelRef {
                    provider,
                    model,
                    base_url: None,
                },
                thinking_level: status.thinking_level.clone(),
                supported_thinking_levels: vec![
                    "off".into(),
                    "minimal".into(),
                    "low".into(),
                    "medium".into(),
                    "high".into(),
                    "xhigh".into(),
                ],
                context_usage: status.usage.clone(),
                session_context_usage: status.session_usage.clone(),
                tui_max_feed_lines: status.tui_max_feed_lines,
                model_catalog: status.model_catalog.clone(),
                latest_trigger_poll: status.latest_trigger_poll.clone(),
                goal: status.goal.clone(),
                control_plane_prompt: status.control_plane_prompt.clone(),
                extensions: status.extensions.clone(),
                system_context: status.system_context.clone(),
            },
            feed: WireSessionFeed {
                blocks: status.feed_blocks.clone(),
                lines: status.feed_lines.clone(),
                blocks_base: status.feed_blocks_base,
                lines_base: status.feed_lines_base,
                block_patches: status.feed_block_patches.clone(),
            },
            graph_state: WireSessionGraphState {
                dags: status.dags.clone(),
                subagents: status.subagents.clone(),
                nodes: Vec::new(),
                active_node_id: None,
            },
            lineage: WireSessionLineage::default(),
        }
    }
}

impl From<&WireSessionSnapshot> for WireStatus {
    fn from(snapshot: &WireSessionSnapshot) -> Self {
        let model = join_model_spec(
            &snapshot.runtime.model.provider,
            &snapshot.runtime.model.model,
        );
        Self {
            session_id: if !snapshot.session_id.is_empty() {
                snapshot.session_id.clone()
            } else {
                snapshot.info.id.clone()
            },
            model,
            thinking_level: snapshot.runtime.thinking_level.clone(),
            model_catalog: snapshot.runtime.model_catalog.clone(),
            cwd: snapshot.info.cwd.clone(),
            busy: snapshot.info.busy,
            queued_count: snapshot.info.queued_count,
            latest_trigger_poll: snapshot.runtime.latest_trigger_poll.clone(),
            goal: snapshot.runtime.goal.clone(),
            control_plane_prompt: snapshot.runtime.control_plane_prompt.clone(),
            sidebar: snapshot.info.sidebar.clone(),
            feed_blocks: snapshot.feed.blocks.clone(),
            feed_blocks_base: snapshot.feed.blocks_base,
            feed_block_patches: snapshot.feed.block_patches.clone(),
            feed_lines: snapshot.feed.lines.clone(),
            feed_lines_base: snapshot.feed.lines_base,
            dags: snapshot.graph_state.dags.clone(),
            subagents: snapshot.graph_state.subagents.clone(),
            usage: snapshot.runtime.context_usage.clone(),
            session_usage: snapshot.runtime.session_context_usage.clone(),
            tui_max_feed_lines: snapshot.runtime.tui_max_feed_lines,
            extensions: snapshot.runtime.extensions.clone(),
            system_context: snapshot.runtime.system_context.clone(),
        }
    }
}

fn split_model_spec(spec: &str) -> (String, String) {
    match spec.split_once(':') {
        Some((provider, model)) if !model.is_empty() => (provider.to_string(), model.to_string()),
        _ => (String::new(), spec.to_string()),
    }
}

fn join_model_spec(provider: &str, model: &str) -> String {
    if provider.is_empty() {
        model.to_string()
    } else {
        format!("{provider}:{model}")
    }
}
