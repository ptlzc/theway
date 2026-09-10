//! Structured user-input intake: `PromptAdmission` produces the canonical record, the live
//! feed renders from that record through the projection replay uses, and the queued and
//! steering paths carry the record with the prompt.

use std::path::PathBuf;
use std::sync::Arc;

use tempfile::TempDir;
use theway_contract::attachments::AttachmentStore;
use theway_contract::user_input::{
    InputFilePart, InputInjectedPart, InputPart, InputSource, UserInput,
};
use theway_core::{
    AgentHarness, AgentHarnessOptions, AgentMessage, MemorySessionStorage, Session, SessionStorage,
};
use theway_llm_provider::{InputModality, ModelCost};
use tokio::sync::mpsc;

use super::super::*;
use crate::agent_session::RetrySettings;
use crate::commands::Registry;
use crate::paths::DaemonPaths;
use crate::session_ops::SessionFactory;
use crate::trigger_engine::execution::TriggerExecutor;
use crate::trigger_engine::runtime::TriggerRuntimeConfig;
use crate::turn::feed::{Block, FeedUpdate};
use crate::turn::kernel::{QueuedTurn, TurnFut, TurnState};
use theway_storage::attachments::LocalAttachmentStore;
use theway_storage::sqlite_repo::SqliteSessionRepo;

/// Leading bytes of a PNG file; the base64 below carries exactly these eight bytes.
const PNG_MAGIC: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
/// Base64 of [`PNG_MAGIC`].
const PNG_B64: &str = "iVBORw0KGgo=";

fn faux_model(input: Vec<InputModality>) -> theway_llm_provider::Model {
    theway_llm_provider::Model {
        id: "faux".into(),
        name: "Faux".into(),
        api: theway_llm_provider::Api::from("faux"),
        provider: theway_llm_provider::Provider::from("faux"),
        base_url: String::new(),
        reasoning: false,
        thinking_level_map: None,
        input,
        cost: ModelCost::default(),
        context_window: 0,
        max_tokens: 0,
        headers: None,
        compat: None,
    }
}

fn test_harness(input: Vec<InputModality>) -> Arc<AgentHarness> {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    Arc::new(AgentHarness::new(AgentHarnessOptions::new(
        faux_model(input),
        session,
    )))
}

fn trigger_executor_for(harness: &Arc<AgentHarness>) -> Arc<TriggerExecutor> {
    Arc::new(TriggerExecutor::new(
        harness.agent_arc(),
        harness.session().clone(),
        TriggerRuntimeConfig::default(),
        None,
        None,
        None,
        None,
        None,
        None,
    ))
}

fn bailing_session_factory() -> SessionFactory {
    Arc::new(
        |_id: String| -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = anyhow::Result<crate::orchestration::SessionRuntime>,
                    > + Send,
            >,
        > { Box::pin(async { anyhow::bail!("session factory unused in intake-record tests") }) },
    )
}

/// Turn host whose attachment library is a scratch directory, so no test touches the
/// environment-derived base dir.
struct Fixture {
    host: TurnHost,
    _scratch: TempDir,
    _repo: TempDir,
}

impl Fixture {
    fn new(input: Vec<InputModality>) -> Self {
        let harness = test_harness(input);
        let trigger_executor = trigger_executor_for(&harness);
        let scratch = TempDir::new().unwrap();
        let work_dir = scratch.path().join("work");
        let home = scratch.path().join("home");
        let base = scratch.path().join("base");
        let paths = DaemonPaths {
            home,
            base: base.clone(),
            work_dir: work_dir.clone(),
            extra_skill_dirs: Arc::new(std::sync::RwLock::new(Vec::new())),
        };
        let repo_dir = TempDir::new().unwrap();
        let (feed_tx, feed_rx) = mpsc::unbounded_channel::<(String, FeedUpdate)>();
        let (_main_run_tx, main_run_rx) = mpsc::unbounded_channel::<String>();
        let config = DaemonConfig {
            harness,
            extension_host: None,
            trigger_executor,
            retry: RetrySettings::default(),
            registry: Registry::with_daemon_commands(),
            cwd: work_dir,
            paths,
            provisioned_skills: Arc::new(std::sync::RwLock::new(Vec::new())),
            provisioned_templates: Arc::new(std::sync::RwLock::new(Vec::new())),
            mcp_provision: Arc::new(std::sync::RwLock::new(
                crate::mcp_loader::McpProvisionState::default(),
            )),
            session_id: "sess-records".into(),
            log_path: None,
            tool_count: 0,
            feed_rx,
            feed_tx,
            main_run_rx,
            control_plane_prompt_rx: None,
            dag_engine: Arc::new(theway_core::multiagent::graph::engine::DagEngine::new()),
            subagent_registry: theway_core::multiagent::jobs::SubagentJobRegistry::new(),
            session_factory: bailing_session_factory(),
            session_repo: Arc::new(SqliteSessionRepo::new(repo_dir.path())),
            capabilities: RuntimeCapabilities::default(),
            thinking_summary: None,
            startup: crate::startup_config::StartupConfig::default(),
            services: crate::orchestration::DaemonServices::new().with_attachments_base(&base),
            observability: Default::default(),
        };
        Self {
            host: TurnHost::new(config),
            _scratch: scratch,
            _repo: repo_dir,
        }
    }

    /// Session cwd: the directory `@path` mentions resolve against.
    fn work_dir(&self) -> PathBuf {
        self._scratch.path().join("work")
    }

    /// The host's own attachment library, for byte-level assertions.
    fn store(&self) -> &LocalAttachmentStore {
        &self.host.automation.services.attachments
    }
}

fn png_wire_image(data: &str, name: Option<&str>) -> theway_transport::wire::WirePromptImage {
    theway_transport::wire::WirePromptImage {
        data: data.to_string(),
        name: name.map(str::to_string),
    }
}

/// The `custom:user_input` records in the harness transcript, oldest first.
fn records(harness: &AgentHarness) -> Vec<UserInput> {
    harness
        .agent()
        .state()
        .messages
        .iter()
        .filter_map(|message| match message {
            AgentMessage::Custom(custom) if custom.role == UserInput::CUSTOM_ROLE => {
                Some(serde_json::from_value::<UserInput>(custom.payload.clone()).expect("record"))
            }
            _ => None,
        })
        .collect()
}

/// The record's `File` parts.
fn file_parts(record: &UserInput) -> Vec<&InputFilePart> {
    record
        .parts
        .iter()
        .filter_map(|part| match part {
            InputPart::File(file) => Some(file),
            _ => None,
        })
        .collect()
}

/// The text of a user message, whether it is text-only or block-shaped.
fn plain_user_text(message: &theway_llm_provider::Message) -> Option<String> {
    let theway_llm_provider::Message::User(user) = message else {
        return None;
    };
    Some(match &user.content {
        theway_llm_provider::UserContent::Text(text) => text.clone(),
        theway_llm_provider::UserContent::Blocks(blocks) => blocks
            .iter()
            .filter_map(|block| match block {
                theway_llm_provider::UserContentBlock::Text(text) => Some(text.text.clone()),
                theway_llm_provider::UserContentBlock::Image(_) => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
    })
}

/// The persisted text of every user message in the transcript.
fn user_texts(harness: &AgentHarness) -> Vec<String> {
    harness
        .agent()
        .state()
        .messages
        .iter()
        .filter_map(|message| match message {
            AgentMessage::Llm(message) => plain_user_text(message),
            AgentMessage::Custom(_) => None,
        })
        .collect()
}

fn user_block(blocks: &[Block]) -> (&str, &[theway_transport::feed::WireFeedAttachment]) {
    blocks
        .iter()
        .find_map(|block| match block {
            Block::User {
                text, attachments, ..
            } => Some((text.as_str(), attachments.as_slice())),
            _ => None,
        })
        .expect("a user block in the live feed")
}

// ── live projection ────────────────────────────────────────────────────────────

#[tokio::test]
async fn submitted_mention_records_one_file_part_and_displays_the_original_text() {
    let mut fixture = Fixture::new(Vec::new());
    std::fs::create_dir_all(fixture.work_dir()).unwrap();
    std::fs::write(fixture.work_dir().join("note.txt"), "hello from note").unwrap();
    let submitted = "看下 @note.txt";
    let mut turn = TurnState::default();

    fixture
        .host
        .submit_web_text(submitted.into(), Vec::new(), false, &mut turn)
        .await;

    // The live user block is the submitted text plus one file chip; the expanded file body
    // never enters the bubble.
    let blocks = fixture.host.projection.feed.blocks();
    let (text, attachments) = user_block(blocks);
    assert_eq!(text, submitted);
    assert!(!text.contains("Files in context:"), "{text}");
    assert_eq!(attachments.len(), 1, "{attachments:?}");
    assert_eq!(attachments[0].kind, "file");
    assert_eq!(attachments[0].name, "note.txt");
    assert_eq!(attachments[0].detail.as_deref(), Some("note.txt"));

    poll_turn(&mut turn.fut).await.expect("faux turn completes");

    let records = records(fixture.host.session.kernel.harness());
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(records[0].text, submitted);
    assert_eq!(records[0].source, InputSource::User);
    let files = file_parts(&records[0]);
    assert_eq!(files.len(), 1, "one part per mentioned path: {files:?}");
    assert_eq!(files[0].path, "note.txt");
    assert_eq!(files[0].name, "note.txt");
    assert!(!files[0].truncated);
    let stored = fixture
        .store()
        .get(&files[0].digest)
        .expect("mentioned bytes are stored under the record digest");
    assert_eq!(stored.as_slice(), b"hello from note".as_slice());

    // The materialized model message still carries the file body, exactly once.
    let texts = user_texts(fixture.host.session.kernel.harness());
    assert_eq!(texts.len(), 1, "{texts:?}");
    assert_eq!(
        texts[0].matches("hello from note").count(),
        1,
        "{}",
        texts[0]
    );
}

#[tokio::test]
async fn submitted_image_records_an_image_part_and_chips_it() {
    let mut fixture = Fixture::new(vec![InputModality::Image]);
    let mut turn = TurnState::default();

    fixture
        .host
        .submit_web_text(
            "look".into(),
            vec![png_wire_image(PNG_B64, Some("pic.png"))],
            false,
            &mut turn,
        )
        .await;

    let blocks = fixture.host.projection.feed.blocks();
    let (text, attachments) = user_block(blocks);
    assert_eq!(text, "look", "no image-count suffix in the bubble");
    assert_eq!(attachments.len(), 1, "{attachments:?}");
    assert_eq!(attachments[0].kind, "image");
    assert_eq!(attachments[0].name, "pic.png");
    assert_eq!(attachments[0].detail.as_deref(), Some("image/png · 8 B"));

    poll_turn(&mut turn.fut).await.expect("faux vision turn completes");

    let records = records(fixture.host.session.kernel.harness());
    assert_eq!(records.len(), 1, "{records:?}");
    assert!(
        file_parts(&records[0]).is_empty(),
        "a text without mention produces no file part: {:?}",
        records[0].parts
    );
    assert_eq!(records[0].parts.len(), 1, "{:?}", records[0].parts);
    let InputPart::Image(image) = &records[0].parts[0] else {
        panic!("not an image part: {:?}", records[0].parts);
    };
    assert_eq!(image.name.as_deref(), Some("pic.png"));
    assert_eq!(image.media_type, "image/png");
    assert_eq!(image.bytes, PNG_MAGIC.len() as u64);
    let stored = fixture
        .store()
        .get(&image.digest)
        .expect("submitted bytes are stored under the record digest");
    assert_eq!(stored.as_slice(), PNG_MAGIC.as_slice());
}

// ── record travels with the prompt ─────────────────────────────────────────────

#[tokio::test]
async fn model_less_submission_records_the_round_and_drains_into_a_turn() {
    let mut fixture = Fixture::new(Vec::new());
    std::fs::create_dir_all(fixture.work_dir()).unwrap();
    std::fs::write(fixture.work_dir().join("note.txt"), "queued body").unwrap();
    fixture.host.session.kernel.harness().agent().state().model = None;
    let submitted = "看下 @note.txt";
    let mut turn = TurnState::default();

    fixture
        .host
        .submit_web_text(submitted.into(), Vec::new(), false, &mut turn)
        .await;

    assert!(turn.fut.is_none(), "a model-less session must not start a turn");
    let recorded = records(fixture.host.session.kernel.harness());
    assert_eq!(recorded.len(), 1, "the queued round is persisted with its record");
    assert_eq!(recorded[0].text, submitted);
    assert_eq!(file_parts(&recorded[0]).len(), 1);
    assert_eq!(
        user_texts(fixture.host.session.kernel.harness()).len(),
        1,
        "one record plus one user message"
    );
    let (text, _) = user_block(fixture.host.projection.feed.blocks());
    assert_eq!(text, submitted);
    assert!(matches!(
        fixture.host.session.queue.front(),
        Some(QueuedTurn::UserPrompt {
            persisted: true,
            input: Some(_),
            ..
        })
    ));

    // A model landing drains the job into a turn that reuses the persisted round
    // (`continue_`) instead of appending a second copy.
    fixture.host.session.kernel.harness().agent().state().model = Some(faux_model(Vec::new()));
    assert!(fixture.host.start_next_queued_turn(&mut turn));
    assert!(turn.fut.is_some());
    assert!(fixture.host.session.busy);
    assert!(fixture.host.session.queue.is_empty());
    assert_eq!(
        records(fixture.host.session.kernel.harness()).len(),
        1,
        "draining must not write a second record"
    );
}

#[tokio::test]
async fn queued_round_renders_injected_parts_as_context_rows() {
    let mut fixture = Fixture::new(Vec::new());
    let mut record = UserInput::user("injected round");
    record.parts.push(InputPart::Injected(InputInjectedPart {
        source: "skill".into(),
        name: Some("review-pr".into()),
        text: "skill preamble".into(),
    }));
    fixture.host.enqueue_turn(QueuedTurn::UserPrompt {
        display: "injected round".into(),
        prompt: "injected round".into(),
        images: Vec::new(),
        input: Some(record),
        persisted: true,
    });

    let mut turn = TurnState::default();
    assert!(fixture.host.start_next_queued_turn(&mut turn));

    let blocks = fixture.host.projection.feed.blocks();
    let (text, _) = user_block(blocks);
    assert_eq!(text, "injected round");
    let Some(Block::Context {
        label,
        text: injected,
        ..
    }) = blocks
        .iter()
        .find(|block| matches!(block, Block::Context { .. }))
    else {
        panic!("an injected part must render as a context row: {blocks:?}");
    };
    assert_eq!(label, "skill:review-pr");
    assert_eq!(injected, "skill preamble");
    assert!(turn.fut.is_some(), "a persisted round continues from state");
}

#[tokio::test]
async fn steering_submission_queues_the_record_before_the_message() {
    let mut fixture = Fixture::new(Vec::new());
    let pending: TurnFut =
        Box::pin(async { Ok::<Option<String>, theway_core::AgentRunError>(None) });
    let mut turn = TurnState {
        fut: Some(pending),
        aborted: false,
        prefix: "",
    };
    let submitted = "hello mid-turn";

    fixture
        .host
        .submit_web_text(submitted.into(), Vec::new(), false, &mut turn)
        .await;

    assert!(
        fixture.host.session.queue.is_empty(),
        "a running turn takes the message through steering, not the daemon queue"
    );
    let (text, _) = user_block(fixture.host.projection.feed.blocks());
    assert_eq!(text, submitted, "the steered round is echoed immediately");

    // The steering queue drains after the running turn's first LLM call; the record must
    // reach the transcript before the message it describes.
    fixture
        .host
        .session
        .kernel
        .harness()
        .prompt("kick off")
        .await
        .expect("faux prompt completes");

    let harness = fixture.host.session.kernel.harness();
    let state = harness.agent().state();
    let messages = &state.messages;
    let record_index = messages
        .iter()
        .position(|message| {
            matches!(message, AgentMessage::Custom(custom) if custom.role == UserInput::CUSTOM_ROLE)
        })
        .expect("the steered record reached the transcript");
    let steered_index = messages
        .iter()
        .position(|message| match message {
            AgentMessage::Llm(message) => plain_user_text(message).as_deref() == Some(submitted),
            AgentMessage::Custom(_) => false,
        })
        .expect("the steered message reached the transcript");
    assert!(
        record_index < steered_index,
        "record at {record_index} must precede its message at {steered_index}"
    );
}

// ── command-synthesised prompts ────────────────────────────────────────────────

#[test]
fn command_prompt_record_shapes_skill_and_host_prompts() {
    // A skill envelope is a human's round: the user's own text plus the injected preamble.
    let envelope =
        theway_transport::commands::attach_skill_prompt("summarize the diff", Some("review-pr"));
    let record = command_prompt_record(&envelope);

    assert_eq!(record.text, "summarize the diff");
    assert_eq!(record.source, InputSource::User);
    assert!(record.source_ref.is_none(), "{:?}", record.source_ref);
    let [InputPart::Injected(part)] = record.parts.as_slice() else {
        panic!("a skill envelope records one injected part: {:?}", record.parts);
    };
    assert_eq!(part.source, "skill");
    assert_eq!(part.name.as_deref(), Some("review-pr"));
    assert_eq!(
        part.text,
        theway_transport::commands::skill_prompt_preamble("review-pr")
    );

    // A prompt the daemon or a trigger synthesised is host-authored, verbatim, with no parts.
    let host = command_prompt_record("/etc/hosts is a path, not a command");
    assert_eq!(host.text, "/etc/hosts is a path, not a command");
    assert_eq!(host.source, InputSource::Host);
    assert!(host.source_ref.is_none(), "{:?}", host.source_ref);
    assert!(host.parts.is_empty(), "{:?}", host.parts);
}

#[tokio::test]
async fn dispatched_command_prompt_records_host_provenance() {
    let mut fixture = Fixture::new(Vec::new());
    let mut turn = TurnState::default();
    let command = "/definitely-not-a-daemon-command";

    fixture.host.dispatch_web_slash(command, &mut turn).await;
    poll_turn(&mut turn.fut).await.expect("faux turn completes");

    let harness = fixture.host.session.kernel.harness();
    let recorded = records(harness);
    assert_eq!(recorded.len(), 1, "{recorded:?}");
    assert_eq!(recorded[0].text, command);
    assert_eq!(recorded[0].source, InputSource::Host);
    assert!(recorded[0].parts.is_empty(), "{:?}", recorded[0].parts);
    let texts = user_texts(harness);
    assert_eq!(texts.len(), 1, "{texts:?}");
    assert_eq!(
        texts[0], command,
        "the record must not rewrite the prompt the model receives"
    );
}

#[tokio::test]
async fn parked_command_prompt_queues_its_record_with_the_turn() {
    let mut fixture = Fixture::new(Vec::new());
    fixture
        .host
        .sessions
        .insert(SessionRuntimeState::for_test("parked-record"));
    let envelope =
        theway_transport::commands::attach_skill_prompt("summarize the diff", Some("review-pr"));

    fixture.host.handle_parked_command_outcome(
        "parked-record",
        "/review-pr summarize the diff",
        CommandOutcome::RunAgentPrompt {
            prompt: envelope.clone(),
            error_context: "skill command failed: ",
        },
    );

    let parked = fixture
        .host
        .sessions
        .get("parked-record")
        .expect("the parked session stays registered");
    assert_eq!(parked.queue.len(), 1);
    let Some(QueuedTurn::AgentPrompt {
        display,
        prompt,
        error_context,
        input,
    }) = parked.queue.front()
    else {
        panic!("a parked command prompt queues a record-carrying agent prompt");
    };
    assert_eq!(display, "/review-pr summarize the diff");
    assert_eq!(prompt, &envelope, "the model-facing prompt stays byte-identical");
    assert_eq!(
        *error_context,
        "skill command failed: ",
        "the queued job keeps the command's error prefix",
    );
    let Some(record) = input else {
        panic!("the queued job carries the round's record");
    };
    assert_eq!(record.text, "summarize the diff");
    assert_eq!(record.source, InputSource::User);
    assert_eq!(record.parts.len(), 1, "{:?}", record.parts);
}
