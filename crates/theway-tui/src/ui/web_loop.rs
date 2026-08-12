//! Transport event loop for the `--web` / `--grpc` servers.
//!
//! The axum/tonic/ws protocol surfaces live in the `theway-server` crate; this
//! module keeps the half that owns *turn semantics*: the kernel-polling
//! `select!` loop ([`App::run_transport_loop`]) plus the App-side command
//! handling and snapshot building it drives. The two sides communicate only
//! through the public channels/state in [`TransportEndpoints`] — the server
//! crate never touches App internals.
//!
//! Startup flow (driven by `theway-server`):
//! 1. [`App::transport_endpoints`] builds the command/snapshot/event channels
//!    and wires the subagent registry + DAG engine event senders.
//! 2. The server crate binds the listener, spawns the protocol server
//!    (`serve_web` / `serve_grpc`) and hands back its [`tokio::task::JoinHandle`].
//! 3. [`App::run_transport_loop`] runs the serialized event loop until Ctrl-C
//!    or server exit.

use std::sync::Arc;

use anyhow::{Context as _, Result, bail};
use async_trait::async_trait;
use base64::Engine as _;
use parking_lot::Mutex;
use tokio::sync::{broadcast, mpsc};

use theway_core::SkillSource;
use theway_core::multiagent::graph::types::DagEvent;
use theway_core::multiagent::registry::AgentJobEvent;

use crate::ui::App;
use crate::ui::kernel::{QueuedTurn, TurnState, poll_turn};
use crate::ui::{feed, prompt_display};
use theway::commands::{CommandCtx, CommandOutcome};
use theway::mentions;
use theway_transport::wire::*;
use theway_transport::{TransportEndpoints, TransportMode};

impl App {
    /// Build the public transport channels and wire the event planes.
    ///
    /// Registers the subagent-registry and DAG-engine event senders (graph
    /// mode) and snapshots the current state; the server crate consumes the
    /// sender side, the event loop ([`App::run_transport_loop`]) the receiver.
    pub fn transport_endpoints(&mut self) -> theway_transport::TransportEndpoints {
        let (command_tx, command_rx) = mpsc::unbounded_channel::<WebCommand>();
        let (snapshot_tx, _) = broadcast::channel::<WebStatus>(128);
        let latest = Arc::new(Mutex::new(self.web_snapshot()));
        // Event plane (graph mode) shared with /ws; the registry broadcasts into it.
        let (event_tx, _) = broadcast::channel::<AgentJobEvent>(256);
        // Forward registry built-in broadcast → event_tx (merged stream for web/gRPC clients).
        let agent_fwd = {
            let mut rx = self.subagent_registry.subscribe();
            let fwd_tx = event_tx.clone();
            tokio::spawn(async move {
                loop {
                    match rx.recv().await {
                        Ok(event) => {
                            let _ = fwd_tx.send(event);
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!("AgentJobEvent broadcast lagged by {n}, skipping");
                            continue;
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
                tracing::debug!("AgentJobEvent registry channel closed; forwarder task exiting");
            })
            .abort_handle()
        };
        // DAG engine broadcasts node_status / run_status (goal runs + DAG runs).
        let (dag_event_tx, _) = broadcast::channel::<DagEvent>(256);
        self.dag_engine.set_event_sender(Some(dag_event_tx.clone()));
        TransportEndpoints {
            command_tx,
            command_rx,
            snapshot_tx,
            latest,
            events: event_tx,
            dag_events: dag_event_tx,
            completer: self.completer.clone(),
            registry: self.subagent_registry.clone(),
            dag_engine: self.dag_engine.clone(),
            session_ops: Arc::new(theway::session_ops::AppSessionOps::new(
                self.session_repo.clone(),
                self.dag_engine.clone(),
                self.current_session_state.clone(),
            )),
            session_id: self.session_id.clone(),
            agent_fwd,
        }
    }

    /// Serialized transport event loop shared by `--web` and `--grpc`.
    ///
    /// Drives turn polling, command intake, feed updates, triggered turns,
    /// relay channels and the control-plane prompt until Ctrl-C or the server
    /// task (`server_task`, spawned by `theway-server`) exits. `mode` only
    /// affects log labels.
    pub async fn run_transport_loop(
        mut self,
        mode: TransportMode,
        endpoints: TransportEndpoints,
        mut server_task: tokio::task::JoinHandle<Result<()>>,
    ) -> Result<()> {
        let label = mode.label();
        let mut command_rx = endpoints.command_rx;
        let latest = endpoints.latest;
        let snapshot_tx = endpoints.snapshot_tx;

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

        // SIGTERM handler (Unix): docker stop / k8s pod termination / systemd
        // send SIGTERM first, then SIGKILL after a grace period. Without a
        // SIGTERM handler, the process ignores the signal and gets force-killed,
        // skipping the session flush + run abort shutdown path.
        #[cfg(unix)]
        let mut sigterm =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();

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
                        self.system_line(format!("[{label}] abort requested"));
                        self.request_abort(&mut turn);
                        self.publish_snapshot(&latest, &snapshot_tx).await;
                    }
                }
                Some(approve) = relay_resolve_rx.recv() => {
                    self.resolve_from_relay(approve);
                    self.publish_snapshot(&latest, &snapshot_tx).await;
                }
                Some(spec) = relay_model_rx.recv() => {
                    self.system_line(format!("[{label}] set model: {spec}"));
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
                _ = async { sigterm.as_mut().unwrap().recv().await }, if sigterm.is_some() => {
                    self.system_line(format!("[{label}] received SIGTERM, shutting down"));
                    if turn.fut.is_some() {
                        self.request_abort(&mut turn);
                        self.publish_snapshot(&latest, &snapshot_tx).await;
                    }
                    break;
                }
                server_result = &mut server_task => {
                    match server_result {
                        Ok(Ok(())) => {}
                        Ok(Err(e)) => self.error_line(format!("{label} server: {e}")),
                        Err(e) => self.error_line(format!("{label} server task: {e}")),
                    }
                    break;
                }
            }
        }
        Ok(())
    }

    async fn handle_web_command(&mut self, command: WebCommand, turn: &mut TurnState) {
        match command {
            WebCommand::Submit {
                text,
                images,
                interrupt,
            } => self.submit_web_text(text, images, interrupt, turn).await,
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
            WebCommand::SwitchSession { id } => self.handle_switch_session(id, turn).await,
        }
    }

    /// session-resource-model: validate + run a session switch on the serialized loop.
    ///
    /// Existence is checked BEFORE aborting anything so an invalid id is harmless; an
    /// in-flight turn on the old harness is aborted first (same semantics as `Abort`) and
    /// unwinds on the next loop iteration after the harness swap.
    async fn handle_switch_session(&mut self, id: String, turn: &mut TurnState) {
        let id = id.trim().to_string();
        if id.is_empty() {
            self.error_line("switch session: missing session id");
            return;
        }
        if id == self.session_id {
            self.system_line(format!("already on session {id}"));
            return;
        }
        match theway::session::find_path_by_id(&self.session_repo, &id).await {
            Ok(Some(_)) => {}
            Ok(None) => {
                self.error_line(format!("switch session: no session matches id {id}"));
                return;
            }
            Err(e) => {
                self.error_line(format!("switch session: {e}"));
                return;
            }
        }
        if turn.fut.is_some() {
            self.request_abort(turn);
        }
        if let Err(e) = self.switch_session(id).await {
            self.error_line(format!("switch session: {e:#}"));
        }
    }

    fn trigger_web_rule_now(&mut self, id: String, turn: &mut TurnState) {
        let id = id.trim();
        if id.is_empty() {
            self.error_line("trigger: missing rule id");
            return;
        }

        let Some(rule) = theway::triggers::global_registry()
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
        interrupt: bool,
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
            theway::commands::attach_skill_prompt(expanded, self.pending_skill.take().as_deref());
        let display = prompt_display(&trimmed, loaded_images.len());
        if interrupt {
            // INTERRUPT mode: stop the in-flight turn and drop anything queued
            // behind it; this message runs as soon as the aborted turn unwinds.
            self.request_abort(turn);
            self.queued_turns.clear();
            self.system_line("interrupt: stopping current turn for new message");
        }
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
                trigger_executor: self.kernel.trigger_executor(),
                session_id: &self.session_id,
                log_path: self.log_path.as_ref(),
                tool_count: self.tool_count,
                cwd: &self.cwd,
            };
            theway::commands::dispatch(input, &self.registry, &ctx).await
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

    pub(super) fn web_snapshot(&self) -> WebStatus {
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
                condition: theway::bug_report::redact(&goal.condition),
                status: goal.status.as_str().to_string(),
                iterations: goal.iterations,
                last_reason: goal.last_reason.as_deref().map(theway::bug_report::redact),
            }),
            control_plane_prompt: self
                .control_plane_prompt
                .as_ref()
                .map(|prompt| web_control_plane_prompt_snapshot(&prompt.request)),
            sidebar: self.web_sidebar_snapshot(),
            feed_blocks: self.feed.web_blocks(),
            feed_lines: web_feed_lines(&self.feed),
            // graph mode: DAG run/node state from the shared engine (P1). Session-scoped:
            // only runs belonging to the current session are surfaced (the engine is
            // process-wide and outlives session switches).
            dags: self
                .dag_engine
                .list_runs()
                .iter()
                .filter(|run| run.session_id.as_deref() == Some(self.session_id.as_str()))
                .map(WebStatus::from_dag_run)
                .collect(),
            // graph mode: subagent jobs (task tool + DAG nodes) from the registry,
            // session-scoped the same way (jobs are stamped with their owning session).
            subagents: self
                .subagent_registry
                .list()
                .iter()
                .filter(|job| job.session_id.as_deref() == Some(self.session_id.as_str()))
                .map(theway_transport::wire::subagent_job_snapshot)
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

        let rules = theway::triggers::global_registry().list();
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

        let cron_jobs = theway::triggers::global_cron_registry().list();
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
            inbox_new: theway_transport::inbox::new_count(
                &theway_transport::inbox::default_inbox_path(),
            ),
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

    async fn publish_snapshot(
        &self,
        latest: &Arc<Mutex<WebStatus>>,
        snapshots: &broadcast::Sender<WebStatus>,
    ) {
        // Keep the SessionOps view of "current session" in lockstep with the loop state;
        // every state mutation on this loop is followed by a publish, so this is the sync
        // point.
        self.sync_current_session_state();
        let snapshot = self.web_snapshot();
        *latest.lock() = snapshot.clone();
        if let Some(active) = &self.relay {
            active.push_snapshot(snapshot.clone());
        }
        let _ = snapshots.send(snapshot);
    }
}

fn web_feed_lines(feed: &feed::Feed) -> Vec<String> {
    feed.plain_lines(100)
}

fn web_preview(text: &str) -> String {
    feed::truncate_chars(&theway::bug_report::redact(text), 120)
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
    feed::truncate_chars(&theway::bug_report::redact(text), cap)
}

fn load_web_prompt_images(
    images: &[WebPromptImage],
) -> Result<Vec<theway_llm_provider::ImageContent>> {
    if images.len() > theway::images::MAX_IMAGES_PER_MESSAGE {
        bail!(
            "{} images exceeds per-message cap of {}",
            images.len(),
            theway::images::MAX_IMAGES_PER_MESSAGE
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
        out.push(theway::images::load_bytes(&label, &bytes)?);
    }
    Ok(out)
}

// ──────────────────────────────────────────────────────────────────────────
// TransportHost impl (theway-transport drives the App through this surface)
// ──────────────────────────────────────────────────────────────────────────

#[async_trait(?Send)]
impl theway_transport::host::TransportHost for App {
    fn transport_endpoints(&mut self) -> theway_transport::TransportEndpoints {
        App::transport_endpoints(self)
    }

    async fn run_transport_loop(
        self: Box<Self>,
        mode: theway_transport::TransportMode,
        endpoints: theway_transport::TransportEndpoints,
        server_task: tokio::task::JoinHandle<anyhow::Result<()>>,
    ) -> anyhow::Result<()> {
        (*self)
            .run_transport_loop(mode, endpoints, server_task)
            .await
    }
}

#[cfg(test)]
mod tests {
    //! Unit tests for the App-side web helpers (feed-line projection and prompt
    //! image decoding); router/bind tests live in `theway-server`.

    use super::*;

    #[test]
    fn web_feed_lines_keeps_all_rows() {
        let mut feed = feed::Feed::new();
        for i in 0..250 {
            feed.apply(feed::FeedUpdate::Plain {
                text: format!("line {i}"),
                level: feed::Level::Output,
            });
        }

        let lines = web_feed_lines(&feed);
        assert_eq!(lines.len(), 250);
        assert!(
            lines.first().is_some_and(|line| line.contains("line 0")),
            "{lines:?}"
        );
        assert!(
            lines.last().is_some_and(|line| line.contains("line 249")),
            "{lines:?}"
        );
        let first = lines.first().expect("first line");
        assert_eq!(first.chars().nth(4), Some('-'), "{lines:?}");
        assert_eq!(first.chars().nth(7), Some('-'), "{lines:?}");
        assert_eq!(first.chars().nth(10), Some(' '), "{lines:?}");
        assert_eq!(first.chars().nth(13), Some(':'), "{lines:?}");
    }

    #[test]
    fn web_prompt_images_decode_to_image_content() {
        let data = base64::engine::general_purpose::STANDARD.encode(b"\x89PNG\r\n\x1a\npng");
        let images = load_web_prompt_images(&[WebPromptImage {
            data,
            name: Some("clip.png".into()),
        }])
        .unwrap();

        assert_eq!(images.len(), 1);
        assert_eq!(images[0].mime_type, "image/png");
        assert!(!images[0].data.is_empty());
    }

    #[test]
    fn web_prompt_images_enforce_count_limit() {
        let images = vec![
            WebPromptImage {
                data: String::new(),
                name: None,
            };
            theway::images::MAX_IMAGES_PER_MESSAGE + 1
        ];
        let err = load_web_prompt_images(&images).unwrap_err().to_string();
        assert!(err.contains("exceeds per-message cap"), "{err}");
    }
}
