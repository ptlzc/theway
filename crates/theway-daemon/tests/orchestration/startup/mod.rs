//! Tests for startup model resolution.

use crate::test_env::{ENV_LOCK, EnvGuard};
use tempfile::TempDir;

use super::{
    canonical_work_dir, monitor_controller_storage, resolve_startup_model,
    supervise_controller_storage,
};

#[tokio::test]
async fn controller_backing_keeps_user_custom_models_available() {
    let _lock = ENV_LOCK.lock().unwrap();
    let base = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    let _theway_dir = EnvGuard::set("THEWAY_DIR", base.path());
    let provider = "controller-custom-model-test";
    let model_id = "model-a";
    std::fs::write(
        base.path().join("models.json"),
        format!(
            r#"{{
  "models": [{{
    "id": "{model_id}",
    "name": "Controller Custom Model",
    "api": "openai-responses",
    "provider": "{provider}",
    "baseUrl": "http://127.0.0.1:9/v1",
    "reasoning": false,
    "input": ["text"],
    "cost": {{"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0}},
    "contextWindow": 128000,
    "maxTokens": 8192
  }}]
}}"#
        ),
    )
    .unwrap();

    let mut startup = crate::startup_config::StartupConfig::default();
    startup.load_local_sources = false;
    startup.storage_service_addr = Some("http://controller-storage".into());

    let model = resolve_startup_model(cwd.path(), Some(provider), Some(model_id), None, &startup)
        .await
        .unwrap()
        .expect("explicit provider/model should resolve a model");

    assert_eq!(model.provider.0, provider);
    assert_eq!(model.id, model_id);
    assert_eq!(model.api.0, "openai-responses");
    theway_llm_provider::unregister_custom_model(&model.provider, &model.id);
}

#[tokio::test]
async fn controller_storage_monitor_exits_after_consecutive_dead_probes() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    drop(listener);

    monitor_controller_storage(
        &addr,
        std::time::Duration::from_millis(5),
        std::time::Duration::from_millis(20),
        2,
    )
    .await
    .unwrap();
}

#[test]
fn canonical_work_dir_canonicalizes_existing_directory() {
    let temp = TempDir::new().unwrap();

    let canonical = canonical_work_dir(temp.path()).unwrap();

    assert_eq!(canonical, temp.path().canonicalize().unwrap());
}

#[test]
fn canonical_work_dir_rejects_missing_directory() {
    let temp = TempDir::new().unwrap();
    let missing = temp.path().join("missing");

    let err = canonical_work_dir(&missing).unwrap_err();

    assert!(format!("{err:#}").contains("cd into"), "{err:#}");
}

#[test]
fn canonical_work_dir_rejects_non_directory_path() {
    let temp = TempDir::new().unwrap();
    let file = temp.path().join("file");
    std::fs::write(&file, "not a directory").unwrap();

    let err = canonical_work_dir(&file).unwrap_err();

    assert!(format!("{err:#}").contains("not a directory"), "{err:#}");
}

#[test]
fn canonical_work_dir_does_not_change_process_cwd() {
    let before = std::env::current_dir().unwrap();
    let temp = TempDir::new().unwrap();

    canonical_work_dir(temp.path()).unwrap();

    assert_eq!(std::env::current_dir().unwrap(), before);
}

#[tokio::test]
async fn supervise_controller_storage_without_addr_awaits_server() {
    let result = supervise_controller_storage(None, async { Ok(()) }).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn supervise_controller_storage_server_wins_before_monitor() {
    let result = supervise_controller_storage(Some("127.0.0.1:1"), async { Ok(()) }).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn supervise_controller_storage_monitor_can_shutdown_server() {
    let result = supervise_controller_storage(Some("127.0.0.1:1"), async {
        std::future::pending::<anyhow::Result<()>>().await
    })
    .await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn resolve_startup_model_uses_startup_default_when_no_cli_override() {
    let _lock = ENV_LOCK.lock().unwrap();
    let base = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    let _theway_dir = EnvGuard::set("THEWAY_DIR", base.path());

    let mut startup = crate::startup_config::StartupConfig::default();
    startup.model_default = Some(theway_transport::config::ModelDefault {
        provider: "anthropic".into(),
        model: "claude-haiku-4-5".into(),
    });

    let model = resolve_startup_model(cwd.path(), None, None, None, &startup)
        .await
        .unwrap()
        .expect("startup default should resolve a model");

    assert_eq!(model.provider.0, "anthropic");
    assert_eq!(model.id, "claude-haiku-4-5");
}

#[tokio::test]
async fn resolve_startup_model_applies_cli_base_url() {
    let _lock = ENV_LOCK.lock().unwrap();
    let base = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    let _theway_dir = EnvGuard::set("THEWAY_DIR", base.path());

    let model = resolve_startup_model(
        cwd.path(),
        Some("anthropic"),
        Some("claude-haiku-4-5"),
        Some("http://example.test/v1"),
        &crate::startup_config::StartupConfig::default(),
    )
    .await
    .unwrap()
    .expect("explicit provider/model should resolve a model");

    assert_eq!(model.base_url, "http://example.test/v1");
}

#[tokio::test]
async fn resolve_startup_model_returns_none_without_credentials() {
    // Model is session-level (injected by the client per-session via `SetModel`),
    // so startup no longer auto-detects from environment variables or fails when
    // no credential is present: it starts model-less and returns `None`.
    let _lock = ENV_LOCK.lock().unwrap();
    let base = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    let _theway_dir = EnvGuard::set("THEWAY_DIR", base.path());
    let _guards = [
        EnvGuard::remove("ANTHROPIC_API_KEY"),
        EnvGuard::remove("OPENAI_API_KEY"),
        EnvGuard::remove("DS4_API_KEY"),
        EnvGuard::remove("OPENROUTER_API_KEY"),
        EnvGuard::remove("GROQ_API_KEY"),
        EnvGuard::remove("MISTRAL_API_KEY"),
        EnvGuard::remove("GEMINI_API_KEY"),
        EnvGuard::remove("GOOGLE_API_KEY"),
    ];

    let model = resolve_startup_model(
        cwd.path(),
        None,
        None,
        None,
        &crate::startup_config::StartupConfig::default(),
    )
    .await
    .unwrap();

    assert!(model.is_none(), "model-less startup must return None, got {model:?}");
}

#[tokio::test]
async fn resolve_startup_model_rejects_unknown_override() {
    let _lock = ENV_LOCK.lock().unwrap();
    let base = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    let _theway_dir = EnvGuard::set("THEWAY_DIR", base.path());

    let err = resolve_startup_model(
        cwd.path(),
        Some("definitely-not-a-provider"),
        Some("does-not-exist"),
        None,
        &crate::startup_config::StartupConfig::default(),
    )
    .await
    .unwrap_err();

    let msg = format!("{err:#}");
    assert!(
        msg.contains("model provider not found") || msg.contains("model not found"),
        "{msg}"
    );
}
