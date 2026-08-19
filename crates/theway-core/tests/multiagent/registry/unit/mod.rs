    use super::*;

    /// Shared in-memory `JobTranscriptStore` test double (stands in for the
    /// daemon's disk-backed store without touching the filesystem).
    #[derive(Default)]
    struct MemoryTranscriptStore {
        nodes:
            parking_lot::Mutex<std::collections::HashMap<(String, String), Vec<serde_json::Value>>>,
        jobs: parking_lot::Mutex<std::collections::HashMap<String, Vec<serde_json::Value>>>,
    }

    impl JobTranscriptStore for MemoryTranscriptStore {
        fn save(&self, transcript: &JobTranscript) {
            let messages = transcript.messages.to_vec();
            match (transcript.run_id, transcript.node_id) {
                (Some(run), Some(node)) => {
                    self.nodes
                        .lock()
                        .insert((run.to_string(), node.to_string()), messages);
                }
                _ => {
                    self.jobs
                        .lock()
                        .insert(transcript.job_id.to_string(), messages);
                }
            }
        }

        fn load_node(&self, run_id: &str, node_id: &str) -> Option<Vec<serde_json::Value>> {
            self.nodes
                .lock()
                .get(&(run_id.to_string(), node_id.to_string()))
                .cloned()
        }

        fn load_job(&self, job_id: &str) -> Option<Vec<serde_json::Value>> {
            self.jobs.lock().get(job_id).cloned()
        }
    }

    #[test]
    fn control_handle_routes_interrupt_and_steer_by_job_id() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let registry = AgentJobRegistry::new();
        let id = registry.register(JobInit {
            agent: "general".into(),
            source: "subagent".into(),
            run_id: None,
            node_id: None,
            session_id: None,
        });

        let interrupted = Arc::new(AtomicBool::new(false));
        let steered = Arc::new(std::sync::Mutex::new(None::<String>));
        registry.set_control(
            &id,
            Some(AgentControlHandle {
                interrupt: {
                    let flag = interrupted.clone();
                    Arc::new(move || flag.store(true, Ordering::SeqCst))
                },
                steer: {
                    let buf = steered.clone();
                    Arc::new(move |text: String| *buf.lock().unwrap() = Some(text))
                },
            }),
        );

        // Unknown job / no handle -> false, no panic.
        assert!(!registry.interrupt("no-such-job"));
        assert!(!registry.steer("no-such-job", "x".into()));

        assert!(registry.interrupt(&id));
        assert!(interrupted.load(Ordering::SeqCst));
        assert!(registry.steer(&id, "use plan B".into()));
        assert_eq!(steered.lock().unwrap().as_deref(), Some("use plan B"));

        // finish detaches the handle -> no longer controllable.
        registry.finish(&id, JobStatus::Succeeded, None);
        assert!(!registry.interrupt(&id));
        assert!(registry.job(&id).unwrap().control.is_none());
    }

    #[test]
    fn control_handle_routes_by_run_node_ids() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let registry = AgentJobRegistry::new();
        let id = registry.register(JobInit {
            agent: "explorer".into(),
            source: "dag".into(),
            run_id: Some("run-9".into()),
            node_id: Some("node-2".into()),
            session_id: None,
        });
        let interrupted = Arc::new(AtomicBool::new(false));
        let steered = Arc::new(std::sync::Mutex::new(None::<String>));
        registry.set_control(
            &id,
            Some(AgentControlHandle {
                interrupt: {
                    let flag = interrupted.clone();
                    Arc::new(move || flag.store(true, Ordering::SeqCst))
                },
                steer: {
                    let buf = steered.clone();
                    Arc::new(move |text: String| *buf.lock().unwrap() = Some(text))
                },
            }),
        );

        assert!(registry.interrupt_node("run-9", "node-2"));
        assert!(interrupted.load(Ordering::SeqCst));
        assert!(registry.steer_node("run-9", "node-2", "dig deeper".into()));
        assert_eq!(steered.lock().unwrap().as_deref(), Some("dig deeper"));

        // Wrong node / run -> false.
        assert!(!registry.interrupt_node("run-9", "nope"));
        assert!(!registry.steer_node("other", "node-2", "x".into()));
    }

    #[test]
    fn register_list_finish_roundtrip() {
        let registry = AgentJobRegistry::new();
        let id = registry.register(JobInit {
            agent: "general".into(),
            source: "subagent".into(),
            run_id: None,
            node_id: None,
            session_id: None,
        });
        let job = registry.job(&id).unwrap();
        assert_eq!(job.status, JobStatus::Running);
        assert_eq!(job.source, "subagent");

        registry.update(&id, |job| {
            job.chars = 10;
            append_output(job, "hello world");
        });
        registry.finish(&id, JobStatus::Succeeded, None);

        let job = registry.job(&id).unwrap();
        assert_eq!(job.status, JobStatus::Succeeded);
        assert_eq!(job.chars, 10);
        assert_eq!(job.output, "hello world");
        assert!(!job.truncated);
        assert!(job.completed_at.is_some());
        assert_eq!(registry.list().len(), 1);
    }

    #[test]
    fn output_buffer_caps_and_flags_truncated() {
        let mut job = AgentJob::new(
            "j1".into(),
            "general".into(),
            "subagent".into(),
            None,
            None,
            None,
        );
        let big = "x".repeat(MAX_OUTPUT_BYTES + 10);
        append_output(&mut job, &big);
        assert!(job.truncated);
        assert!(job.output.len() <= MAX_OUTPUT_BYTES);
        // Tail is preserved (last chunk lands at the end).
        assert!(job.output.ends_with(&"x".repeat(10)));
    }

    #[test]
    fn evicts_oldest_terminal_job_when_over_cap() {
        let registry = AgentJobRegistry::new();
        let mut first_id = None;
        for i in 0..(MAX_JOBS + 5) {
            let id = registry.register(JobInit {
                agent: "general".into(),
                source: "subagent".into(),
                run_id: None,
                node_id: None,
                session_id: None,
            });
            if i == 0 {
                first_id = Some(id.clone());
            }
            // terminal states for all but the last, which stays running
            if i < MAX_JOBS + 4 {
                registry.finish(&id, JobStatus::Succeeded, None);
            }
        }
        assert_eq!(registry.list().len(), MAX_JOBS);
        // The first (oldest terminal) job is evicted.
        assert!(registry.job(first_id.as_ref().unwrap()).is_none());
        // The running job survives.
        let jobs = registry.list();
        let running = jobs
            .iter()
            .find(|j| j.status == JobStatus::Running)
            .expect("running job kept");
        assert!(running.completed_at.is_none());
    }

    #[test]
    fn metrics_listener_counts_tools_and_turns() {
        let registry = AgentJobRegistry::new();
        let id = registry.register(JobInit {
            agent: "general".into(),
            source: "subagent".into(),
            run_id: None,
            node_id: None,
            session_id: None,
        });
        let listener = metrics_listener(registry.clone(), id.clone());
        listener(&LoopEvent::ToolExecutionStart {
            tool_call_id: "t1".into(),
            tool_name: "read".into(),
            args: serde_json::Value::Null,
        });
        listener(&LoopEvent::TurnStart);

        let job = registry.job(&id).unwrap();
        assert_eq!(job.tools_called, 1);
        assert_eq!(job.turn, 1);
        // chars accumulate via TextDelta (covered end-to-end; constructing an
        // AssistantMessage here is not worth the fixture surface).
        assert_eq!(job.chars, 0);
    }

    #[test]
    fn message_end_captures_full_transcript_in_order() {
        use theway_llm_provider::{
            AssistantMessage, ContentBlock, StopReason, ToolResultMessage, ToolResultRole, Usage,
            UserContent, UserContentBlock, UserMessage, UserRole,
        };
        let registry = AgentJobRegistry::new();
        let id = registry.register(JobInit {
            agent: "explorer".into(),
            source: "dag".into(),
            run_id: Some("run-1".into()),
            node_id: Some("node-1".into()),
            session_id: None,
        });
        let listener = metrics_listener(registry.clone(), id.clone());

        // User prompt (run_loop replays new_messages through MessageStart/MessageEnd).
        listener(&LoopEvent::MessageEnd {
            message: AgentMessage::Llm(PiMessage::User(UserMessage {
                role: UserRole::User,
                content: UserContent::Text("explore the repo".into()),
                timestamp: 0,
            })),
        });
        // Assistant turn with a text block + usage (tokens must still accumulate).
        listener(&LoopEvent::MessageEnd {
            message: AgentMessage::Llm(PiMessage::Assistant(AssistantMessage {
                role: theway_llm_provider::AssistantRole::Assistant,
                content: vec![ContentBlock::text("found it")],
                api: theway_llm_provider::Api::from("faux"),
                provider: theway_llm_provider::Provider::from("faux"),
                model: "faux".into(),
                response_model: None,
                response_id: None,
                diagnostics: None,
                usage: Usage {
                    input: 10,
                    output: 5,
                    ..Default::default()
                },
                stop_reason: StopReason::Stop,
                error_message: None,
                timestamp: 0,
            })),
        });
        // Tool result (also replayed through MessageStart/MessageEnd).
        listener(&LoopEvent::MessageEnd {
            message: AgentMessage::Llm(PiMessage::ToolResult(ToolResultMessage {
                role: ToolResultRole::ToolResult,
                tool_call_id: "t1".into(),
                tool_name: "grep".into(),
                content: vec![UserContentBlock::text("3 matches")],
                details: None,
                is_error: false,
                timestamp: 0,
            })),
        });

        let job = registry.job(&id).unwrap();
        assert_eq!(
            job.messages.len(),
            3,
            "user + assistant + tool result transcript"
        );
        // User prompt: internally tagged by `role` (Message is `#[serde(tag="role")]`).
        let m0 = &job.messages[0];
        assert_eq!(m0["role"], serde_json::json!("user"));
        assert_eq!(m0["content"], serde_json::json!("explore the repo"));
        // Assistant turn: content + usage preserved.
        let m1 = &job.messages[1];
        assert_eq!(m1["role"], serde_json::json!("assistant"));
        assert_eq!(m1["content"][0]["text"], serde_json::json!("found it"));
        assert_eq!(m1["usage"]["input"], serde_json::json!(10));
        // Tool result.
        let m2 = &job.messages[2];
        assert_eq!(m2["role"], serde_json::json!("toolResult"));
        assert_eq!(m2["toolName"], serde_json::json!("grep"));
        assert!(!job.messages_truncated);
        // Usage still accumulated from the assistant turn.
        assert_eq!(job.input_tokens, 10);
        assert_eq!(job.output_tokens, 5);
    }

    #[test]
    fn message_buffer_caps_drops_oldest_keeps_newest() {
        let mut job = AgentJob::new(
            "j1".into(),
            "general".into(),
            "subagent".into(),
            None,
            None,
            None,
        );
        // A single message alone exceeds the cap: it is kept (never drop newest).
        let huge = serde_json::json!({"role": "note", "blob": "x".repeat(MAX_MESSAGES_BYTES)});
        append_message(&mut job, &huge);
        assert!(job.messages_truncated);
        assert_eq!(job.messages.len(), 1);
        // The next small message evicts the huge one (drop oldest until under cap).
        let small = serde_json::json!({"role": "note", "text": "tail"});
        append_message(&mut job, &small);
        assert_eq!(job.messages.len(), 1, "huge message dropped, tail kept");
        assert_eq!(job.messages[0]["text"], serde_json::json!("tail"));
        assert!(job.messages_truncated);
    }

    #[test]
    fn finish_saves_transcript_to_host_store() {
        let store = Arc::new(MemoryTranscriptStore::default());
        let registry = AgentJobRegistry::new();
        registry.set_transcript_store(Some(store.clone()));
        let id = registry.register(JobInit {
            agent: "explorer".into(),
            source: "dag".into(),
            run_id: Some("run-1".into()),
            node_id: Some("node-1".into()),
            session_id: None,
        });
        registry.update(&id, |job| {
            append_message(
                job,
                &serde_json::json!({"role": "note", "text": "recover me"}),
            );
        });

        registry.finish(&id, JobStatus::Succeeded, None);

        // Simulated restart: a fresh registry (empty memory) with the same
        // host store resolves the finished transcript through the seam.
        let restarted = AgentJobRegistry::new();
        restarted.set_transcript_store(Some(store.clone()));
        let messages = restarted
            .node_messages("run-1", "node-1")
            .expect("transcript recovered from host store after restart");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["text"], serde_json::json!("recover me"));
        // In-memory lookup still serves the live job first.
        let live = registry.node_messages("run-1", "node-1").unwrap();
        assert_eq!(live.len(), 1);
        // Unknown node → None.
        assert!(restarted.node_messages("run-1", "nope").is_none());
    }

    #[test]
    fn job_messages_fall_back_to_host_store_after_restart() {
        let store = Arc::new(MemoryTranscriptStore::default());
        let registry = AgentJobRegistry::new();
        registry.set_transcript_store(Some(store.clone()));
        let id = registry.register(JobInit {
            agent: "general".into(),
            source: "subagent".into(),
            run_id: None,
            node_id: None,
            session_id: None,
        });
        registry.update(&id, |job| {
            append_message(
                job,
                &serde_json::json!({"role": "note", "text": "task transcript"}),
            );
        });
        registry.finish(&id, JobStatus::Succeeded, None);

        let restarted = AgentJobRegistry::new();
        restarted.set_transcript_store(Some(store.clone()));
        let messages = restarted.job_messages(&id).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["text"], serde_json::json!("task transcript"));
    }
