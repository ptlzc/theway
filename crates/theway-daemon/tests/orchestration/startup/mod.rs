//! Tests for startup model resolution.

use crate::test_env::{ENV_LOCK, EnvGuard};
use tempfile::TempDir;

use super::{monitor_controller_storage, resolve_startup_model};

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

    let model = resolve_startup_model(
        cwd.path(),
        Some(provider),
        Some(model_id),
        None,
        &startup,
    )
    .await
    .unwrap();

    assert_eq!(model.provider.0, provider);
    assert_eq!(model.id, model_id);
    assert_eq!(model.api.0, "openai-responses");
    theway_llm_provider::unregister_custom_model(&model.provider, &model.id);
}

#[tokio::test]
async fn controller_storage_monitor_exits_after_consecutive_dead_probes() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap();
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
