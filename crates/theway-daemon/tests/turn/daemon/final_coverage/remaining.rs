// ── model switch branches ──────────────────────────────────────────────────────

#[tokio::test]
async fn set_model_from_spec_switches_to_model_without_credential_hint() {
    let _env_lock = ENV_LOCK.lock().unwrap();

    let model = theway_llm_provider::list_models()
        .into_iter()
        .find(|m| {
            SUPPORTED_APIS.contains(&m.api.0.as_str())
                && !theway_llm_provider::env_api_keys::env_var_names(&m.provider.0).is_empty()
        })
        .expect("a supported model with env vars should exist in the catalog");
    let var_name = theway_llm_provider::env_api_keys::env_var_names(&model.provider.0)[0];
    let _env = EnvGuard::set(var_name, "test-credential");
    let spec = format!("{}:{}", model.provider.0, model.id);

    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();
    host.set_model_from_spec(&spec).await;

    assert_eq!(current_model_label(host.session.kernel.harness()), spec);
}

struct FailingAppendStorage {
    inner: Arc<MemorySessionStorage>,
}

#[async_trait]
impl SessionStorage for FailingAppendStorage {
    async fn get_metadata_json(&self) -> Result<serde_json::Value, SessionError> {
        self.inner.get_metadata_json().await
    }
    async fn append_entry(&self, _entry: SessionTreeEntry) -> Result<(), SessionError> {
        Err(SessionError {
            code: SessionErrorCode::StorageFailure,
            message: "synthetic write failure".into(),
        })
    }
    async fn get_entry(&self, id: &str) -> Result<Option<SessionTreeEntry>, SessionError> {
        self.inner.get_entry(id).await
    }
    async fn get_entries(&self) -> Result<Vec<SessionTreeEntry>, SessionError> {
        self.inner.get_entries().await
    }
    async fn get_path_to_root(
        &self,
        entry_id: Option<&str>,
    ) -> Result<Vec<SessionTreeEntry>, SessionError> {
        self.inner.get_path_to_root(entry_id).await
    }
    async fn find_entries(
        &self,
        entry_type: &str,
    ) -> Result<Vec<SessionTreeEntry>, SessionError> {
        self.inner.find_entries(entry_type).await
    }
    async fn get_leaf_id(&self) -> Result<Option<String>, SessionError> {
        self.inner.get_leaf_id().await
    }
    async fn set_leaf_id(&self, id: Option<String>) -> Result<(), SessionError> {
        self.inner.set_leaf_id(id).await
    }
    async fn create_entry_id(&self) -> Result<String, SessionError> {
        self.inner.create_entry_id().await
    }
    async fn get_label(&self, id: &str) -> Result<Option<String>, SessionError> {
        self.inner.get_label(id).await
    }
}

#[tokio::test]
async fn set_model_from_spec_maps_set_model_errors() {
    let session = Session::new(
        Arc::new(FailingAppendStorage {
            inner: Arc::new(MemorySessionStorage::new()),
        }) as Arc<dyn SessionStorage>,
    );
    let harness = harness_with_options(AgentHarnessOptions::new(faux_model(Vec::new()), session));
    let model = theway_llm_provider::list_models()
        .into_iter()
        .find(|m| SUPPORTED_APIS.contains(&m.api.0.as_str()))
        .expect("a supported model should exist in the catalog");
    let spec = format!("{}:{}", model.provider.0, model.id);

    let built = build_host(harness.clone());
    let (mut host, _scratch, _repo) = built.into_parts();
    host.set_model_from_spec(&spec).await;

    // The failed model change must not stick.
    assert_eq!(current_model_label(host.session.kernel.harness()), "faux:faux");
}

// ── remaining command-outcome branches ──────────────────────────────────────────

struct OverflowImportStubCommand;

#[async_trait]
impl SlashCommand<crate::commands::DaemonCtx> for OverflowImportStubCommand {
    fn name(&self) -> &'static str {
        "overflow-import"
    }
    fn description(&self) -> &'static str {
        "stub import with more than five enabled trigger ids"
    }
    async fn run(
        &self,
        _argv: &[String],
        _ctx: &TransportCommandCtx<'_, crate::commands::DaemonCtx>,
    ) -> CommandOutcome {
        CommandOutcome::SessionImportActivation {
            session_path: PathBuf::from("/tmp/imported-overflow"),
            trigger_ids: (0..6).map(|i| format!("trigger-{i}")).collect(),
            cron_ids: vec![],
        }
    }
}

#[tokio::test]
async fn dispatch_web_slash_lists_overflow_import_activation_ids() {
    let mut registry = Registry::new();
    registry.register(Arc::new(OverflowImportStubCommand));
    let built = build_host_with(
        harness_with_input(Vec::new()),
        registry,
        bailing_session_factory(),
        "sess-final",
        None,
    );
    let (mut host, _scratch, _repo) = built.into_parts();

    let mut turn = TurnState::default();
    host.dispatch_web_slash("/overflow-import", &mut turn).await;

    assert!(turn.fut.is_none());
}

#[tokio::test]
async fn dispatch_web_slash_open_model_picker_without_active_model() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();
    host.session.kernel.harness().agent().state().model = None;

    let mut turn = TurnState::default();
    host.dispatch_web_slash("/model", &mut turn).await;

    assert!(turn.fut.is_none());
}

// ── TransportHost trait delegation ──────────────────────────────────────────────

#[tokio::test]
async fn transport_host_trait_delegates_to_turn_host() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();

    let endpoints = theway_transport::host::TransportHost::transport_endpoints(&mut host);
    let latest = endpoints.latest.clone();

    let server_task = tokio::spawn(async {
        tokio::time::sleep(Duration::from_millis(50)).await;
        anyhow::Ok(())
    });
    theway_transport::host::TransportHost::run_transport_loop(
        Box::new(host),
        TransportMode::Grpc,
        endpoints,
        server_task,
    )
    .await
    .unwrap();

    let snapshot = latest.lock().clone();
    assert_eq!(snapshot.session_id, "sess-final");
}

// ── pending stream helper ───────────────────────────────────────────────────────

static PENDING_SENDERS: Mutex<Vec<AssistantMessageEventSender>> = Mutex::new(Vec::new());

fn pending_stream_fn() -> StreamFn {
    Arc::new(|_, _, _| {
        let (stream, sender) = AssistantMessageEventStream::new();
        PENDING_SENDERS.lock().unwrap().push(sender);
        stream
    })
}

fn harness_with_pending_stream() -> Arc<AgentHarness> {
    let mut options = AgentHarnessOptions::new(faux_model(Vec::new()), memory_session());
    options.stream_fn = Some(pending_stream_fn());
    harness_with_options(options)
}
