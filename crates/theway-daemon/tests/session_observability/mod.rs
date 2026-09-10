//! Mirrored unit tests for `DaemonSessionObservability`: authoritative
//! snapshot merge and cursor pagination (session-observability contract).

use super::*;

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;
use theway_contract::user_input::{InputFilePart, InputPart, UserInput};
use theway_core::{AgentMessage, CustomMessage};
use theway_llm_provider::{Message, UserContent, UserMessage};
use theway_transport::feed::WireFeedBlock;
use theway_transport::session_observability::ListSessionMessagesRequest;
use theway_transport::wire::{
    WireContextUsage, WireSessionFeed, WireSessionGraphState, WireSessionInfo, WireSessionLineage,
    WireSessionRuntime, WireSessionSnapshot, WireStatus,
};
use theway_storage::sqlite_repo::SqliteSessionRepo;

use crate::runtime_storage::SessionRepository;

fn user_message(text: &str) -> AgentMessage {
    AgentMessage::Llm(Message::User(UserMessage {
        role: Default::default(),
        content: UserContent::Text(text.to_string()),
        timestamp: 1_700_000_000_000,
    }))
}

/// The canonical `custom:user_input` record of one round of input, in the shape
/// `crates/theway-core` writes it to the transcript.
fn record_input() -> UserInput {
    UserInput {
        text: "look at @src/foo.rs".into(),
        parts: vec![InputPart::File(InputFilePart {
            path: "src/foo.rs".into(),
            name: "foo.rs".into(),
            digest: "sha256:00".into(),
            bytes: 12,
            media_type: "text/plain".into(),
            truncated: false,
        })],
        source: Default::default(),
        source_ref: None,
    }
}

fn record_message(input: &UserInput) -> AgentMessage {
    AgentMessage::Custom(CustomMessage {
        role: UserInput::CUSTOM_ROLE.into(),
        timestamp: 1_700_000_000_000,
        payload: serde_json::to_value(input).expect("record serializes"),
    })
}

fn live_status(session_id: &str, system_context: &str) -> WireStatus {
    WireStatus {
        session_id: session_id.to_string(),
        model: "anthropic:claude-x".into(),
        thinking_level: "high".into(),
        model_catalog: Vec::new(),
        cwd: "/live/cwd".into(),
        busy: false,
        queued_count: 0,
        latest_trigger_poll: None,
        goal: None,
        control_plane_prompt: None,
        sidebar: theway_transport::testing::empty_sidebar_snapshot(),
        feed_blocks: vec![theway_transport::feed::WireFeedBlock::User {
            text: "live feed".into(),
            timestamp: None,
            attachments: Vec::new(),
            source: None,
        }],
        feed_blocks_base: 0,
        feed_block_patches: Vec::new(),
        feed_lines: vec!["live feed".into()],
        feed_lines_base: 0,
        dags: Vec::new(),
        subagents: Vec::new(),
        usage: WireContextUsage::default(),
        session_usage: WireContextUsage::default(),
        tui_max_feed_lines: None,
        extensions: theway_transport::wire::WireExtensionSnapshot::default(),
        system_context: system_context.to_string(),
        shell_count: 0,
        observability: Default::default(),
    }
}

fn resource_snapshot(session_id: &str, cwd: &str, lineage: WireSessionLineage) -> WireSessionSnapshot {
    WireSessionSnapshot {
        session_id: session_id.to_string(),
        info: WireSessionInfo {
            id: session_id.to_string(),
            name: "resource-name".into(),
            cwd: cwd.to_string(),
            created_at: "2026-01-01T00:00:00Z".into(),
            last_activity_at: 0,
            last_activity_at_rfc3339: None,
            busy: false,
            preview: None,
            metadata: HashMap::new(),
            graph_count: 0,
            active_graph_count: 0,
            queued_count: 0,
            sidebar: theway_transport::testing::empty_sidebar_snapshot(),
        },
        runtime: WireSessionRuntime {
            model: Default::default(),
            thinking_level: String::new(),
            supported_thinking_levels: Vec::new(),
            context_usage: Default::default(),
            session_context_usage: Default::default(),
            tui_max_feed_lines: None,
            shell_count: 0,
            model_catalog: Vec::new(),
            latest_trigger_poll: None,
            goal: None,
            control_plane_prompt: None,
            extensions: Default::default(),
            system_context: String::new(),
        },
        feed: WireSessionFeed {
            blocks: Vec::new(),
            lines: Vec::new(),
            blocks_base: 0,
            lines_base: 0,
            block_patches: Vec::new(),
        },
        graph_state: WireSessionGraphState {
            dags: Vec::new(),
            subagents: Vec::new(),
            nodes: vec![theway_transport::wire::WireSessionGraphNode {
                id: "node-1".into(),
                session_id: session_id.to_string(),
                node_type: theway_transport::wire::WireSessionGraphNodeType::Session,
                title: "node".into(),
                summary: String::new(),
                parent_node_id: None,
                child_node_ids: Vec::new(),
                collapsed_session_id: None,
                collapsed_at: None,
                created_at: None,
                updated_at: None,
                message_count: 0,
            }],
            active_node_id: Some("node-1".into()),
        },
        lineage,
    }
}

/// Minimal `SessionOps` that returns a scripted resource snapshot while
/// pagination is served by the real repository.
struct ScriptedSessionOps {
    snapshot: Mutex<Option<WireSessionSnapshot>>,
}

#[async_trait::async_trait]
impl SessionOps for ScriptedSessionOps {
    async fn list(&self) -> anyhow::Result<Vec<theway_transport::wire::SessionSummary>> {
        Ok(Vec::new())
    }

    async fn create(
        &self,
        _session_id: Option<&str>,
        _metadata: &HashMap<String, String>,
    ) -> anyhow::Result<String> {
        anyhow::bail!("unused")
    }

    async fn update_metadata(
        &self,
        _id: &str,
        _metadata: &HashMap<String, String>,
    ) -> anyhow::Result<()> {
        anyhow::bail!("unused")
    }

    async fn rename(&self, _id: &str, _name: &str) -> anyhow::Result<()> {
        anyhow::bail!("unused")
    }

    async fn delete(&self, _id: &str) -> anyhow::Result<Vec<String>> {
        anyhow::bail!("unused")
    }

    async fn session_snapshot(&self, _session_id: &str) -> anyhow::Result<WireSessionSnapshot> {
        self.snapshot
            .lock()
            .clone()
            .ok_or_else(|| anyhow::anyhow!("no scripted snapshot"))
    }
}

fn observability(
    repo: Arc<dyn SessionRepository>,
    resource: WireSessionSnapshot,
    states: Arc<Mutex<HashMap<String, WireStatus>>>,
    latest: Arc<Mutex<WireStatus>>,
) -> Arc<dyn SessionObservabilityOps> {
    Arc::new(DaemonSessionObservability::new(
        Arc::new(ScriptedSessionOps {
            snapshot: Mutex::new(Some(resource)),
        }),
        states,
        latest,
        repo,
    ))
}

#[tokio::test]
async fn authoritative_snapshot_merges_live_and_resource_planes() {
    let temp = tempfile::tempdir().unwrap();
    let repo: Arc<dyn SessionRepository> = Arc::new(SqliteSessionRepo::new(temp.path()));
    let resource = resource_snapshot(
        "sess-1",
        "/resource/cwd",
        WireSessionLineage {
            root_session_id: Some("root-1".into()),
            ..Default::default()
        },
    );
    // The live projection carries the real sidebar inventory (MCP servers +
    // errors); the resource plane's snapshot always has an empty sidebar.
    let mut live = live_status("sess-1", "<context>live</context>");
    live.sidebar.mcp.servers = 2;
    live.sidebar.mcp.tools = 34;
    live.sidebar.mcp.server_names = vec!["devops-mcp".into(), "crg".into()];
    live.sidebar.mcp.errors = vec![theway_transport::wire::WireMcpServerError {
        name: "devin-search".into(),
        error: "transport error".into(),
    }];
    let states = Arc::new(Mutex::new(HashMap::from([("sess-1".into(), live.clone())])));
    let latest = Arc::new(Mutex::new(live));
    let ops = observability(repo, resource, states, latest);

    let snapshot = ops.authoritative_snapshot("sess-1").await.unwrap();

    // Live fields: runtime + system_context + feed.
    assert_eq!(snapshot.runtime.system_context, "<context>live</context>");
    assert_eq!(snapshot.runtime.thinking_level, "high");
    assert_eq!(snapshot.feed.lines, vec!["live feed"]);
    // Resource fields: info identity + graph nodes + lineage.
    assert_eq!(snapshot.info.cwd, "/resource/cwd");
    assert_eq!(snapshot.info.name, "resource-name");
    assert_eq!(snapshot.graph_state.nodes.len(), 1);
    assert_eq!(
        snapshot.graph_state.active_node_id.as_deref(),
        Some("node-1")
    );
    assert_eq!(snapshot.lineage.root_session_id.as_deref(), Some("root-1"));
    // The sidebar is live inventory: the resource plane cannot know it, so
    // the merge must keep the live projection's sidebar (issue: MCP servers
    // vanished from GetSnapshot / the TUI's startup panel view).
    assert_eq!(snapshot.info.sidebar.mcp.servers, 2);
    assert_eq!(snapshot.info.sidebar.mcp.tools, 34);
    assert_eq!(
        snapshot.info.sidebar.mcp.server_names,
        vec!["devops-mcp", "crg"]
    );
    assert_eq!(snapshot.info.sidebar.mcp.errors.len(), 1);
    assert_eq!(snapshot.info.sidebar.mcp.errors[0].name, "devin-search");
}

#[tokio::test]
async fn authoritative_snapshot_falls_back_to_resource_when_no_live_projection() {
    let temp = tempfile::tempdir().unwrap();
    let repo: Arc<dyn SessionRepository> = Arc::new(SqliteSessionRepo::new(temp.path()));
    let store = SessionRepository::create_with_id(repo.as_ref(), temp.path(), Some("sess-1"))
        .await
        .unwrap();
    let session = Session::from_store(store);
    session.append_message(user_message("persisted first")).await.unwrap();
    session.append_message(user_message("persisted second")).await.unwrap();
    let resource = resource_snapshot("sess-1", "/resource/cwd", Default::default());
    let states = Arc::new(Mutex::new(HashMap::new()));
    let latest = Arc::new(Mutex::new(live_status("other-session", "")));
    let ops = observability(repo, resource, states, latest);

    let snapshot = ops.authoritative_snapshot("sess-1").await.unwrap();
    assert_eq!(snapshot.info.cwd, "/resource/cwd");
    // No live runtime: the resource snapshot is seeded from persisted history
    // so a cold resume has visible context (full runtime builds on first send).
    assert_eq!(snapshot.feed.blocks.len(), 2);
    let theway_transport::feed::WireFeedBlock::User { text, .. } = &snapshot.feed.blocks[0] else {
        panic!("expected user block");
    };
    assert_eq!(text, "persisted first");
    let theway_transport::feed::WireFeedBlock::User { text, .. } = &snapshot.feed.blocks[1] else {
        panic!("expected user block");
    };
    assert_eq!(text, "persisted second");
    assert!(snapshot.runtime.system_context.is_empty());
}

#[tokio::test]
async fn list_messages_paginates_newest_first_with_cursor() {
    let temp = tempfile::tempdir().unwrap();
    let repo: Arc<dyn SessionRepository> = Arc::new(SqliteSessionRepo::new(temp.path()));
    let store = SessionRepository::create_with_id(repo.as_ref(), temp.path(), Some("sess-1"))
        .await
        .unwrap();
    let session = Session::from_store(store);
    let first = session.append_message(user_message("first")).await.unwrap();
    let second = session.append_message(user_message("second")).await.unwrap();
    session.append_message(user_message("third")).await.unwrap();
    let states = Arc::new(Mutex::new(HashMap::new()));
    let latest = Arc::new(Mutex::new(live_status("sess-1", "")));
    let ops = observability(
        repo,
        resource_snapshot("sess-1", "/resource/cwd", Default::default()),
        states,
        latest,
    );

    // Newest page without a cursor: limit=2, old→new within the page.
    let page = ops
        .list_session_messages(&ListSessionMessagesRequest {
            session_id: "sess-1".into(),
            before_entry_id: None,
            limit: 2,
        })
        .await
        .unwrap();
    assert_eq!(page.total, 3);
    assert!(page.has_more);
    assert_eq!(page.next_before_entry_id.as_deref(), Some(second.as_str()));
    assert_eq!(page.blocks.len(), 2);
    let theway_transport::feed::WireFeedBlock::User { text, .. } = &page.blocks[0] else {
        panic!("expected user block");
    };
    assert_eq!(text, "second");
    let theway_transport::feed::WireFeedBlock::User { text, .. } = &page.blocks[1] else {
        panic!("expected user block");
    };
    assert_eq!(text, "third");

    // Older page using the cursor: no overlap, no gap.
    let page = ops
        .list_session_messages(&ListSessionMessagesRequest {
            session_id: "sess-1".into(),
            before_entry_id: Some(second),
            limit: 2,
        })
        .await
        .unwrap();
    assert!(!page.has_more);
    assert_eq!(page.next_before_entry_id.as_deref(), Some(first.as_str()));
    assert_eq!(page.blocks.len(), 1);
    let theway_transport::feed::WireFeedBlock::User { text, .. } = &page.blocks[0] else {
        panic!("expected user block");
    };
    assert_eq!(text, "first");

    // Oldest cursor → empty terminal page.
    let page = ops
        .list_session_messages(&ListSessionMessagesRequest {
            session_id: "sess-1".into(),
            before_entry_id: Some(first),
            limit: 2,
        })
        .await
        .unwrap();
    assert!(page.blocks.is_empty());
    assert!(!page.has_more);
    assert_eq!(page.next_before_entry_id, None);

    // Unknown cursor → empty page (fork/compaction changed the active branch).
    let page = ops
        .list_session_messages(&ListSessionMessagesRequest {
            session_id: "sess-1".into(),
            before_entry_id: Some("unknown-entry".into()),
            limit: 2,
        })
        .await
        .unwrap();
    assert!(page.blocks.is_empty());
    assert!(!page.has_more);
    assert_eq!(page.total, 3);
}

#[tokio::test]
async fn list_messages_renders_a_record_from_the_previous_page() {
    let temp = tempfile::tempdir().unwrap();
    let repo: Arc<dyn SessionRepository> = Arc::new(SqliteSessionRepo::new(temp.path()));
    let store = SessionRepository::create_with_id(repo.as_ref(), temp.path(), Some("sess-1"))
        .await
        .unwrap();
    let session = Session::from_store(store);
    let first = session.append_message(user_message("first")).await.unwrap();
    let input = record_input();
    let appended = session
        .append_messages(vec![
            record_message(&input),
            user_message("look at @src/foo.rs with file contents"),
        ])
        .await
        .unwrap();
    let record_id = appended[0].clone();
    let prompt_id = appended[1].clone();
    let states = Arc::new(Mutex::new(HashMap::new()));
    let latest = Arc::new(Mutex::new(live_status("sess-1", "")));
    let ops = observability(
        repo,
        resource_snapshot("sess-1", "/resource/cwd", Default::default()),
        states,
        latest,
    );

    // The record entry describes a message: `total` counts transcript messages only.
    let page = ops
        .list_session_messages(&ListSessionMessagesRequest {
            session_id: "sess-1".into(),
            before_entry_id: None,
            limit: 1,
        })
        .await
        .unwrap();
    assert_eq!(page.total, 2);
    assert!(page.has_more);
    assert_eq!(page.next_before_entry_id.as_deref(), Some(prompt_id.as_str()));
    assert_eq!(page.blocks.len(), 1);
    // The page starts after the record and still renders the record's text + chips.
    let WireFeedBlock::User {
        text,
        attachments,
        source,
        ..
    } = &page.blocks[0]
    else {
        panic!("expected user block: {:?}", page.blocks[0]);
    };
    assert_eq!(text, "look at @src/foo.rs");
    assert_eq!(attachments.len(), 1, "{attachments:?}");
    assert_eq!(attachments[0].name, "foo.rs");
    assert_eq!(attachments[0].detail.as_deref(), Some("src/foo.rs"));
    assert_eq!(source.as_ref().map(|origin| origin.kind.as_str()), Some("user"));

    // The older page is the legacy message: no record, no chips.
    let page = ops
        .list_session_messages(&ListSessionMessagesRequest {
            session_id: "sess-1".into(),
            before_entry_id: Some(prompt_id),
            limit: 1,
        })
        .await
        .unwrap();
    assert_eq!(page.total, 2);
    assert!(!page.has_more);
    assert_eq!(page.next_before_entry_id.as_deref(), Some(first.as_str()));
    assert_eq!(page.blocks.len(), 1);
    let block = &page.blocks[0];
    let WireFeedBlock::User { text, .. } = block else {
        panic!("expected user block: {block:?}");
    };
    assert_eq!(text, "first");
    assert!(
        matches!(block, WireFeedBlock::User { attachments, .. } if attachments.is_empty()),
        "{block:?}"
    );

    // Cursor semantics still address message entries only: a record entry is not one.
    let page = ops
        .list_session_messages(&ListSessionMessagesRequest {
            session_id: "sess-1".into(),
            before_entry_id: Some(record_id),
            limit: 1,
        })
        .await
        .unwrap();
    assert!(page.blocks.is_empty());
    assert!(!page.has_more);
    assert_eq!(page.total, 2);
}
