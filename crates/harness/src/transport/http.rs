//! Local browser HTTP transport for the coding-agent REPL (`--web` mode).
//!
//! This is intentionally a small loopback-only surface. The browser layer sends commands into the
//! same single-turn event loop used by the TUI and receives full feed snapshots over SSE. The
//! protocol model it serializes lives in [`super::types`]; the event loop it drives is
//! `crate::ui::App`.

use std::convert::Infallible;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

use anyhow::{Context as _, Result, bail};
use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine as _;
use futures::Stream;
use tokio::sync::{Mutex, broadcast, mpsc};

use crate::commands::{CommandCtx, CommandOutcome};
use crate::mentions;
use crate::readline::SlashCompleter;
use crate::transport::proto::subagent_job_snapshot;
use crate::transport::types::*;
use crate::transport::ws::ws_upgrade;
use crate::ui::kernel::{QueuedTurn, TurnState, poll_turn};
use crate::ui::{App, feed};
use theway_core::SkillSource;
use theway_core::harness::graph_engineering::types::DagEvent;
use theway_core::harness::subagents::registry::{SubagentEvent, SubagentJobRegistry};

#[derive(Clone)]
pub(crate) struct HttpState {
    pub(crate) commands: mpsc::UnboundedSender<WebCommand>,
    pub(crate) snapshots: broadcast::Sender<WebStatus>,
    pub(crate) latest: Arc<Mutex<WebStatus>>,
    completer: SlashCompleter,
    pub(crate) events: broadcast::Sender<SubagentEvent>,
    /// DAG engine event plane (node_status / run_status), shared with /ws.
    pub(crate) dag_events: broadcast::Sender<DagEvent>,
    pub(crate) registry: SubagentJobRegistry,
}

impl App {
    pub async fn run_web(mut self, options: WebOptions) -> Result<()> {
        let addr = bind_addr(&options.host, options.port)?;
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .with_context(|| format!("bind web ui on {addr}"))?;
        let actual = listener.local_addr()?;

        let (command_tx, mut command_rx) = mpsc::unbounded_channel::<WebCommand>();
        let (snapshot_tx, _) = broadcast::channel::<WebStatus>(128);
        let latest = Arc::new(Mutex::new(self.web_snapshot()));
        // Event plane (graph mode) shared with /ws; the registry broadcasts into it.
        let (event_tx, _) =
            broadcast::channel::<theway_core::harness::subagents::registry::SubagentEvent>(256);
        self.subagent_registry
            .set_event_sender(Some(event_tx.clone()));
        // DAG engine broadcasts node_status / run_status (goal runs + DAG runs).
        let (dag_event_tx, _) = broadcast::channel::<DagEvent>(256);
        self.dag_engine.set_event_sender(Some(dag_event_tx.clone()));
        let router = web_router(HttpState {
            commands: command_tx,
            snapshots: snapshot_tx.clone(),
            latest: latest.clone(),
            completer: self.completer.clone(),
            events: event_tx,
            dag_events: dag_event_tx,
            registry: self.subagent_registry.clone(),
        });

        let server = axum::serve(listener, router.into_make_service());
        let mut server_task = tokio::spawn(async move { server.await });
        let url = format!("http://{actual}");
        println!("theway web listening on {url}");
        if let Err(e) = open_web_browser(&url) {
            eprintln!("web browser auto-open skipped: {e}");
        }

        let mut feed_rx = self.feed_rx.take().expect("feed_rx taken once");
        let mut main_run_rx = self.main_run_rx.take().expect("main_run_rx taken once");
        let mut control_plane_prompt_rx = self.control_plane_prompt_rx.take();
        let mut relay_prompt_rx = self
            .relay_prompt_rx
            .take()
            .expect("relay_prompt_rx taken once");
        let mut relay_abort_rx = self
            .relay_abort_rx
            .take()
            .expect("relay_abort_rx taken once");
        let mut relay_resolve_rx = self
            .relay_resolve_rx
            .take()
            .expect("relay_resolve_rx taken once");
        let mut relay_model_rx = self
            .relay_model_rx
            .take()
            .expect("relay_model_rx taken once");
        let mut turn = TurnState::default();
        self.refresh_goal_state().await;
        self.publish_snapshot(&latest, &snapshot_tx).await;

        loop {
            tokio::select! {
                biased;
                result = poll_turn(&mut turn.fut), if turn.fut.is_some() => {
                    self.finish_turn(&mut turn, result).await;
                    self.publish_snapshot(&latest, &snapshot_tx).await;
                }
                Some(command) = command_rx.recv() => {
                    self.handle_web_command(command, &mut turn).await;
                    self.publish_snapshot(&latest, &snapshot_tx).await;
                }
                Some(update) = feed_rx.recv() => {
                    self.apply_feed_update(update);
                    while let Ok(update) = feed_rx.try_recv() {
                        self.apply_feed_update(update);
                    }
                    self.publish_snapshot(&latest, &snapshot_tx).await;
                }
                Some(trace_id) = main_run_rx.recv(), if turn.fut.is_none() => {
                    self.start_triggered_turn(trace_id, &mut turn);
                    self.publish_snapshot(&latest, &snapshot_tx).await;
                }
                Some(text) = relay_prompt_rx.recv() => {
                    self.submit_remote_text(text, &mut turn);
                    self.publish_snapshot(&latest, &snapshot_tx).await;
                }
                Some(()) = relay_abort_rx.recv() => {
                    if turn.fut.is_some() {
                        self.system_line("[web] abort requested");
                        self.request_abort(&mut turn);
                        self.publish_snapshot(&latest, &snapshot_tx).await;
                    }
                }
                Some(approve) = relay_resolve_rx.recv() => {
                    self.resolve_from_relay(approve);
                    self.publish_snapshot(&latest, &snapshot_tx).await;
                }
                Some(spec) = relay_model_rx.recv() => {
                    self.system_line(format!("[web] set model: {spec}"));
                    self.set_model_from_spec(&spec).await;
                    self.publish_snapshot(&latest, &snapshot_tx).await;
                }
                Some(prompt) = async {
                    match control_plane_prompt_rx.as_mut() {
                        Some(rx) => rx.recv().await,
                        None => None,
                    }
                }, if self.control_plane_prompt.is_none() && control_plane_prompt_rx.is_some() => {
                    self.show_control_plane_prompt(prompt);
                    self.publish_snapshot(&latest, &snapshot_tx).await;
                }
                _ = tokio::signal::ctrl_c() => {
                    if turn.fut.is_some() {
                        self.request_abort(&mut turn);
                        self.publish_snapshot(&latest, &snapshot_tx).await;
                    }
                    break;
                }
                server_result = &mut server_task => {
                    match server_result {
                        Ok(Ok(())) => {}
                        Ok(Err(e)) => self.error_line(format!("web server: {e}")),
                        Err(e) => self.error_line(format!("web server task: {e}")),
                    }
                    break;
                }
            }
        }
        Ok(())
    }

    pub(crate) async fn handle_web_command(&mut self, command: WebCommand, turn: &mut TurnState) {
        match command {
            WebCommand::Submit { text, images } => self.submit_web_text(text, images, turn).await,
            WebCommand::TriggerRuleNow { id } => self.trigger_web_rule_now(id, turn),
            WebCommand::Abort => self.request_abort(turn),
            WebCommand::ResolveControlPlane { approve } => {
                let decision = if approve {
                    theway_core::ControlPlanePromptDecision::Allow
                } else {
                    theway_core::ControlPlanePromptDecision::Deny {
                        reason: Some("denied by user".into()),
                    }
                };
                self.resolve_control_plane_prompt(decision);
            }
            WebCommand::SetModel { spec } => self.set_model_from_spec(&spec).await,
        }
    }

    fn trigger_web_rule_now(&mut self, id: String, turn: &mut TurnState) {
        let id = id.trim();
        if id.is_empty() {
            self.error_line("trigger: missing rule id");
            return;
        }

        let Some(rule) = crate::triggers::global_registry()
            .list()
            .into_iter()
            .find(|rule| rule.id == id)
        else {
            self.error_line(format!("trigger: no dynamic trigger rule with id `{id}`"));
            return;
        };

        let display = format!(
            "trigger now {}: {}",
            feed::truncate_chars(&rule.id, 18),
            web_preview(&rule.action)
        );
        self.follow = true;
        if turn.fut.is_some() {
            self.queue_user_prompt(display, rule.action, Vec::new());
        } else {
            self.feed.push_user(display);
            self.start_user_prompt_turn(rule.action, Vec::new(), turn);
        }
    }

    async fn submit_web_text(
        &mut self,
        text: String,
        images: Vec<WebPromptImage>,
        turn: &mut TurnState,
    ) {
        let trimmed = text.trim().to_string();
        if trimmed.is_empty() && images.is_empty() {
            return;
        }
        let loaded_images = match load_web_prompt_images(&images) {
            Ok(images) => images,
            Err(e) => {
                self.error_line(format!("pasted image: {e}"));
                return;
            }
        };
        if !loaded_images.is_empty() && !self.current_model_accepts_images() {
            self.error_line(format!(
                "current model does not support image input; switch to a vision-capable model before sending {} image attachment(s)",
                loaded_images.len()
            ));
            return;
        }
        if !trimmed.is_empty() {
            self.history.append(&trimmed);
        }
        self.follow = true;

        if trimmed.starts_with('/') && loaded_images.is_empty() {
            self.feed.push_user(&trimmed);
            self.dispatch_web_slash(&trimmed, turn).await;
            return;
        }

        let expanded = if trimmed.is_empty() {
            String::new()
        } else {
            mentions::expand(&trimmed, &self.cwd).await.0
        };
        let prompt_text =
            crate::commands::attach_skill_prompt(expanded, self.pending_skill.take().as_deref());
        let display = crate::ui::prompt_display(&trimmed, loaded_images.len());
        if turn.fut.is_some() {
            self.queue_user_prompt(display, prompt_text, loaded_images);
        } else {
            self.feed.push_user(display);
            self.start_user_prompt_turn(prompt_text, loaded_images, turn);
        }
    }

    async fn dispatch_web_slash(&mut self, input: &str, turn: &mut TurnState) {
        let outcome = {
            let ctx = CommandCtx {
                harness: self.kernel.harness(),
                session_id: &self.session_id,
                log_path: self.log_path.as_ref(),
                tool_count: self.tool_count,
                cwd: &self.cwd,
            };
            crate::commands::dispatch(input, &self.registry, &ctx).await
        };
        match outcome {
            CommandOutcome::Quit => {
                self.system_line("web ui stays running; close the browser tab or press Ctrl-C in the terminal to stop the server");
            }
            CommandOutcome::ClearScreen => {
                self.feed.clear();
                self.follow = true;
            }
            CommandOutcome::Error(e) => self.error_line(e),
            CommandOutcome::AttachSkill { name } => {
                self.pending_skill = Some(name);
            }
            CommandOutcome::RunAgentPrompt {
                prompt,
                error_context,
            } => {
                if turn.fut.is_some() {
                    self.enqueue_turn(QueuedTurn::AgentPrompt {
                        display: input.to_string(),
                        prompt,
                        error_context,
                    });
                } else {
                    self.start_prompt_turn(prompt, error_context, turn);
                }
            }
            CommandOutcome::RunPromptTemplate { name, vars } => {
                if turn.fut.is_some() {
                    self.enqueue_turn(QueuedTurn::PromptTemplate {
                        display: input.to_string(),
                        name,
                        vars,
                    });
                } else {
                    self.start_template_turn(name, vars, turn);
                }
            }
            CommandOutcome::RunCompaction { custom } => {
                if turn.fut.is_some() {
                    self.enqueue_turn(QueuedTurn::Compaction {
                        display: input.to_string(),
                        custom,
                    });
                } else {
                    self.start_compaction_turn(custom, turn);
                }
            }
            CommandOutcome::WebRelay(action) => self.handle_web_relay(action).await,
            CommandOutcome::SessionImportActivation {
                session_path,
                trigger_ids,
                cron_ids,
            } => self.prompt_import_activation(session_path, trigger_ids, cron_ids),
            CommandOutcome::LoginSecret {
                provider,
                recovery_command,
                ..
            } => {
                let command = recovery_command.unwrap_or_else(|| format!("/login {provider}"));
                self.error_line(format!(
                    "web login is not implemented yet; run `{command}` from the terminal UI"
                ));
            }
            CommandOutcome::OpenModelPicker => {
                let active = match self.kernel.harness().agent().state().model.clone() {
                    Some(m) => format!("active model: {}:{}", m.provider.0, m.id),
                    None => "(no model active)".into(),
                };
                self.system_line(format!(
                    "{active} — click the model name in the header to switch"
                ));
            }
            CommandOutcome::Handled => {}
        }
        if input.trim_start().starts_with("/goal") {
            self.refresh_goal_state().await;
        }
    }

    pub(crate) fn web_snapshot(&self) -> WebStatus {
        let model = {
            let state = self.kernel.harness().agent().state();
            state
                .model
                .as_ref()
                .map(|m| format!("{}:{}", m.provider.0, m.id))
                .unwrap_or_else(|| "no-model".to_string())
        };
        WebStatus {
            session_id: self.session_id.clone(),
            model,
            model_catalog: self.model_catalog.clone(),
            cwd: self.cwd.display().to_string(),
            busy: self.busy,
            queued_count: self.queued_turns.len(),
            latest_trigger_poll: self.latest_trigger_poll.clone(),
            goal: self.latest_goal.as_ref().map(|goal| WebGoalSnapshot {
                condition: crate::bug_report::redact(&goal.condition),
                status: goal.status.as_str().to_string(),
                iterations: goal.iterations,
                last_reason: goal.last_reason.as_deref().map(crate::bug_report::redact),
            }),
            control_plane_prompt: self
                .control_plane_prompt
                .as_ref()
                .map(|prompt| web_control_plane_prompt_snapshot(&prompt.request)),
            sidebar: self.web_sidebar_snapshot(),
            feed_blocks: self.feed.web_blocks(),
            feed_lines: web_feed_lines(&self.feed),
            // graph mode: DAG run/node state from the shared engine (P1).
            dags: self
                .dag_engine
                .list_runs()
                .iter()
                .map(WebStatus::from_dag_run)
                .collect(),
            // graph mode: subagent jobs (task tool + DAG nodes) from the registry.
            subagents: self
                .subagent_registry
                .list()
                .iter()
                .map(subagent_job_snapshot)
                .collect(),
        }
    }

    fn web_sidebar_snapshot(&self) -> WebSidebarSnapshot {
        const ITEM_LIMIT: usize = 8;

        let skills = self.kernel.harness().skills();
        let disabled = skills
            .iter()
            .filter(|skill| skill.disable_model_invocation)
            .count();
        let enabled = skills.len().saturating_sub(disabled);
        let source_count = |source| skills.iter().filter(|skill| skill.source == source).count();

        let rules = crate::triggers::global_registry().list();
        let trigger_enabled = rules.iter().filter(|rule| rule.enabled).count();
        let trigger_rules = rules
            .iter()
            .take(ITEM_LIMIT)
            .map(|rule| WebTriggerRuleSnapshot {
                id: feed::truncate_chars(&rule.id, 18),
                full_id: rule.id.clone(),
                enabled: rule.enabled,
                mode: if rule.fire_once { "once" } else { "repeat" }.to_string(),
                condition: web_preview(&rule.condition),
                action: web_preview(&rule.action),
            })
            .collect::<Vec<_>>();

        let cron_jobs = crate::triggers::global_cron_registry().list();
        let cron_enabled = cron_jobs.iter().filter(|job| job.enabled).count();
        let cron_job_rows = cron_jobs
            .iter()
            .take(ITEM_LIMIT)
            .map(|job| WebCronJobSnapshot {
                id: feed::truncate_chars(&job.id, 18),
                enabled: job.enabled,
                schedule: job.schedule.clone(),
                action: web_preview(&job.action),
                skipped_overlap_count: job.skipped_overlap_count,
                last_error: job.last_error.as_deref().map(web_preview),
            })
            .collect::<Vec<_>>();

        WebSidebarSnapshot {
            inbox_new: crate::inbox::new_count(&crate::inbox::default_inbox_path()),
            skills: WebSkillsSnapshot {
                total: skills.len(),
                enabled,
                disabled,
                builtin: source_count(SkillSource::Builtin),
                user: source_count(SkillSource::User),
                project: source_count(SkillSource::Project),
                items: skills
                    .iter()
                    .map(|skill| WebSkillSnapshot {
                        name: skill.name.clone(),
                        source: skill.source.label().to_string(),
                        file_path: skill.file_path.clone(),
                        enabled: !skill.disable_model_invocation,
                    })
                    .collect(),
            },
            triggers: WebTriggersSnapshot {
                total: rules.len(),
                enabled: trigger_enabled,
                disabled: rules.len().saturating_sub(trigger_enabled),
                rules: trigger_rules,
            },
            cron: WebCronSnapshot {
                total: cron_jobs.len(),
                enabled: cron_enabled,
                disabled: cron_jobs.len().saturating_sub(cron_enabled),
                jobs: cron_job_rows,
            },
            mcp: WebMcpSnapshot {
                servers: self.panel_status.mcp_servers,
                tools: self.panel_status.mcp_tools,
                notification_hooks: self.panel_status.mcp_notification_hooks,
                server_names: self.panel_status.mcp_server_names.clone(),
                tool_names: self.panel_status.mcp_tool_names.clone(),
            },
            tools: WebToolsSnapshot {
                total: self.panel_status.tool_names.len(),
                names: self.panel_status.tool_names.clone(),
            },
            hooks: self.panel_status.hook_points.clone(),
            runtime: self.panel_status.trigger_features.clone(),
        }
    }

    pub(crate) async fn publish_snapshot(
        &self,
        latest: &Arc<Mutex<WebStatus>>,
        snapshots: &broadcast::Sender<WebStatus>,
    ) {
        let snapshot = self.web_snapshot();
        *latest.lock().await = snapshot.clone();
        if let Some(active) = &self.relay {
            active.push_snapshot(snapshot.clone());
        }
        let _ = snapshots.send(snapshot);
    }
}
fn web_router(state: HttpState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/state", get(state_snapshot))
        .route("/events", get(events))
        .route("/ws", get(ws_upgrade))
        .route("/prompt", post(prompt))
        .route("/model", post(set_model))
        .route("/complete", post(complete))
        .route("/abort", post(abort))
        .route("/trigger/immediate", post(trigger_immediate))
        .route("/control-plane/resolve", post(resolve_control_plane))
        .with_state(state)
}

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

async fn state_snapshot(State(state): State<HttpState>) -> Json<WebStatus> {
    Json(state.latest.lock().await.clone())
}

async fn events(
    State(state): State<HttpState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = state.snapshots.subscribe();
    let stream = futures::stream::unfold(rx, |mut rx| async move {
        loop {
            match rx.recv().await {
                Ok(snapshot) => {
                    let data = serde_json::to_string(&snapshot)
                        .unwrap_or_else(|_| "{\"error\":\"serialize\"}".to_string());
                    return Some((Ok(Event::default().event("status").data(data)), rx));
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

async fn prompt(
    State(state): State<HttpState>,
    Json(req): Json<PromptRequest>,
) -> impl IntoResponse {
    let accepted = state
        .commands
        .send(WebCommand::Submit {
            text: req.text,
            images: req.images,
        })
        .is_ok();
    Json(CommandAccepted { accepted })
}

async fn complete(
    State(state): State<HttpState>,
    Json(req): Json<CompleteRequest>,
) -> impl IntoResponse {
    Json(CompleteResponse {
        completions: state.completer.matches(&req.text),
    })
}

async fn abort(State(state): State<HttpState>) -> impl IntoResponse {
    let accepted = state.commands.send(WebCommand::Abort).is_ok();
    Json(CommandAccepted { accepted })
}

async fn trigger_immediate(
    State(state): State<HttpState>,
    Json(req): Json<TriggerRuleRequest>,
) -> impl IntoResponse {
    let accepted = state
        .commands
        .send(WebCommand::TriggerRuleNow { id: req.id })
        .is_ok();
    Json(CommandAccepted { accepted })
}

async fn set_model(
    State(state): State<HttpState>,
    Json(request): Json<SetModelRequest>,
) -> impl IntoResponse {
    let accepted = state
        .commands
        .send(WebCommand::SetModel {
            spec: request.model,
        })
        .is_ok();
    Json(CommandAccepted { accepted })
}

async fn resolve_control_plane(
    State(state): State<HttpState>,
    Json(req): Json<ControlPlaneDecisionRequest>,
) -> impl IntoResponse {
    let accepted = state
        .commands
        .send(WebCommand::ResolveControlPlane {
            approve: req.approve,
        })
        .is_ok();
    Json(CommandAccepted { accepted })
}

fn web_feed_lines(feed: &feed::Feed) -> Vec<String> {
    feed.plain_lines(100)
}

fn web_preview(text: &str) -> String {
    feed::truncate_chars(&crate::bug_report::redact(text), 120)
}

fn web_control_plane_prompt_snapshot(
    request: &theway_core::ControlPlanePromptRequest,
) -> WebControlPlanePromptSnapshot {
    let payload = serde_json::to_string_pretty(&request.payload)
        .unwrap_or_else(|_| request.payload.to_string());
    WebControlPlanePromptSnapshot {
        tool_name: web_prompt_text(&request.tool_name, 80),
        label: web_prompt_text(&request.label, 160),
        reason: web_prompt_text(&request.reason, 180),
        args_hash: request.args_hash.chars().take(12).collect(),
        payload: web_prompt_text(&payload, 800),
    }
}

fn web_prompt_text(text: &str, cap: usize) -> String {
    feed::truncate_chars(&crate::bug_report::redact(text), cap)
}

fn load_web_prompt_images(
    images: &[WebPromptImage],
) -> Result<Vec<theway_llm_provider::ImageContent>> {
    if images.len() > crate::images::MAX_IMAGES_PER_MESSAGE {
        bail!(
            "{} images exceeds per-message cap of {}",
            images.len(),
            crate::images::MAX_IMAGES_PER_MESSAGE
        );
    }
    let mut out = Vec::with_capacity(images.len());
    for (idx, image) in images.iter().enumerate() {
        let label = image
            .name
            .as_deref()
            .filter(|name| !name.trim().is_empty())
            .map(|name| format!("clipboard image `{name}`"))
            .unwrap_or_else(|| format!("clipboard image #{}", idx + 1));
        let data = image
            .data
            .rsplit_once(',')
            .map(|(_, data)| data)
            .unwrap_or(image.data.as_str());
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data)
            .with_context(|| format!("decode {label}"))?;
        out.push(crate::images::load_bytes(&label, &bytes)?);
    }
    Ok(out)
}

pub(crate) fn bind_addr(host: &str, port: u16) -> Result<SocketAddr> {
    let ip = match host {
        "localhost" => IpAddr::V4(Ipv4Addr::LOCALHOST),
        host => host
            .parse::<IpAddr>()
            .with_context(|| format!("parse --web-host `{host}` as an IP address"))?,
    };
    if !ip.is_loopback() {
        bail!("refusing non-loopback web bind {ip}; Web UI is loopback-only");
    }
    Ok(SocketAddr::new(ip, port))
}

/// Minimal sidebar used by snapshot fixtures in tests (web + grpc).
#[cfg(test)]
pub(crate) fn empty_sidebar_snapshot() -> WebSidebarSnapshot {
    WebSidebarSnapshot {
        inbox_new: crate::inbox::new_count(&crate::inbox::default_inbox_path()),
        skills: WebSkillsSnapshot {
            total: 0,
            enabled: 0,
            disabled: 0,
            builtin: 0,
            user: 0,
            project: 0,
            items: Vec::new(),
        },
        triggers: WebTriggersSnapshot {
            total: 0,
            enabled: 0,
            disabled: 0,
            rules: Vec::new(),
        },
        cron: WebCronSnapshot {
            total: 0,
            enabled: 0,
            disabled: 0,
            jobs: Vec::new(),
        },
        mcp: WebMcpSnapshot {
            servers: 0,
            tools: 0,
            notification_hooks: 0,
            server_names: Vec::new(),
            tool_names: Vec::new(),
        },
        tools: WebToolsSnapshot {
            total: 0,
            names: Vec::new(),
        },
        hooks: Vec::new(),
        runtime: Vec::new(),
    }
}

fn open_web_browser(url: &str) -> Result<()> {
    let mut command = open_browser_command(url);
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    command.spawn().context("spawn system browser")?;
    Ok(())
}

fn open_browser_command(url: &str) -> std::process::Command {
    #[cfg(target_os = "macos")]
    {
        let mut cmd = std::process::Command::new("open");
        cmd.arg(url);
        cmd
    }
    #[cfg(target_os = "windows")]
    {
        let mut cmd = std::process::Command::new("cmd");
        cmd.args(["/C", "start", "", url]);
        cmd
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let mut cmd = std::process::Command::new("xdg-open");
        cmd.arg(url);
        cmd
    }
}

const INDEX_HTML: &str = include_str!("../ui/web_index.html");

#[cfg(test)]
mod tests;
