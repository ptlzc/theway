use super::*;
use theway_transport::wire::WireDaemonConfig;

#[tokio::test]
async fn handle_web_command_routes_configure_empty_update() {
    let mut fixture = HostFixture::new().await;
    let host = fixture.host();
    let before = host.runtime.config.read().unwrap().clone();

    host.handle_web_command(
        WireCommand::Configure {
            config: WireDaemonConfig::default(),
        },
        &mut TurnState::default(),
    )
    .await;

    assert_eq!(*host.runtime.config.read().unwrap(), before);
}

#[tokio::test]
async fn handle_configure_applies_feed_history_limit() {
    let mut fixture = HostFixture::new().await;
    let host = fixture.host();

    let mut patch = WireDaemonConfig::default();
    patch.tui_max_feed_lines = Some(42);
    host.handle_configure(patch, &mut TurnState::default()).await;

    assert_eq!(host.runtime.feed_history_limit, Some(42));
    assert_eq!(host.runtime.config.read().unwrap().tui_max_feed_lines, Some(42));
}

#[tokio::test]
async fn handle_configure_registers_custom_models_and_seeds_api_key() {
    let mut fixture = HostFixture::new().await;
    let host = fixture.host();

    let model = theway_llm_provider::Model {
        id: "configure-custom-model".into(),
        name: "Configured custom".into(),
        api: theway_llm_provider::Api::from("openai-completions"),
        provider: theway_llm_provider::Provider::from("configure-test"),
        base_url: "http://127.0.0.1:9/v1".into(),
        reasoning: false,
        thinking_level_map: None,
        input: vec![theway_llm_provider::InputModality::Text],
        cost: theway_llm_provider::ModelCost::default(),
        context_window: 128_000,
        max_tokens: 8_192,
        headers: None,
        compat: None,
    };
    host.handle_configure(
        WireDaemonConfig {
            provider: Some("configure-test".into()),
            model: Some("configure-custom-model".into()),
            models: vec![model.clone()],
            api_key: Some("sk-configured".into()),
            ..Default::default()
        },
        &mut TurnState::default(),
    )
    .await;

    // Issue #136: the descriptor was registered before the pair resolved.
    assert!(theway_llm_provider::get_model(&model.provider, &model.id).is_some());
    // The credential overlay holds the configured key for the provider.
    assert_eq!(
        host.automation
            .services
            .configured_api_keys
            .read()
            .unwrap()
            .get("configure-test")
            .map(String::as_str),
        Some("sk-configured")
    );
    let view = host.runtime.config.read().unwrap().clone();
    assert_eq!(view.api_key.as_deref(), Some("sk-configured"));
    assert_eq!(view.models.len(), 1);
    assert_eq!(view.provider.as_deref(), Some("configure-test"));
    theway_llm_provider::unregister_custom_model(&model.provider, &model.id);
}

#[tokio::test]
async fn handle_configure_auto_fetch_imports_catalog_and_selects_first() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buffer = vec![0u8; 4096];
        let _ = socket.read(&mut buffer).await.unwrap();
        let body = r#"{"data":[{"id":"fetched-model-a"},{"id":"fetched-model-b"}]}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        socket.write_all(response.as_bytes()).await.unwrap();
    });
    let base_url = format!("http://{addr}/v1");

    let mut fixture = HostFixture::new().await;
    let host = fixture.host();
    host.handle_configure(
        WireDaemonConfig {
            provider: Some("fetch-test".into()),
            base_url: Some(base_url.clone()),
            auto_fetch_models: Some(true),
            ..Default::default()
        },
        &mut TurnState::default(),
    )
    .await;

    let view = host.runtime.config.read().unwrap().clone();
    assert_eq!(view.auto_fetch_models, Some(true));
    assert_eq!(view.models.len(), 2);
    assert_eq!(view.models[0].id, "fetched-model-a");
    assert_eq!(view.models[0].base_url, base_url);
    // The unset model was filled from the first catalog entry and applied.
    assert_eq!(view.provider.as_deref(), Some("fetch-test"));
    assert_eq!(view.model.as_deref(), Some("fetched-model-a"));
    for model in &view.models {
        theway_llm_provider::unregister_custom_model(&model.provider, &model.id);
    }
}

#[tokio::test]
async fn handle_configure_auto_fetch_without_base_url_is_reported_not_applied() {
    let mut fixture = HostFixture::new().await;
    let host = fixture.host();

    host.handle_configure(
        WireDaemonConfig {
            provider: Some("fetch-test".into()),
            auto_fetch_models: Some(true),
            ..Default::default()
        },
        &mut TurnState::default(),
    )
    .await;

    // No base_url: the fetch is reported as an error and never enters the view
    // (the seeded view keeps the startup default).
    assert_eq!(
        host.runtime.config.read().unwrap().auto_fetch_models,
        Some(false)
    );
}

#[tokio::test]
async fn handle_configure_rejections_do_not_publish_unapplied_values() {
    let mut fixture = HostFixture::new().await;
    let host = fixture.host();
    let before = host.runtime.config.read().unwrap().clone();

    host.handle_configure(
        WireDaemonConfig {
            provider: Some("missing-provider".into()),
            model: Some("missing-model".into()),
            trigger_poll_secs: Some(0),
            tui_max_feed_lines: Some(0),
            storage_service_addr: Some("http://startup-only".into()),
            executor_kind: Some("sandbox".into()),
            tgrep: Some(false),
            ..Default::default()
        },
        &mut TurnState::default(),
    )
    .await;

    assert_eq!(*host.runtime.config.read().unwrap(), before);
}

#[tokio::test]
async fn handle_configure_unknown_clear_rejects_the_whole_patch() {
    let mut fixture = HostFixture::new().await;
    let host = fixture.host();
    let before = host.runtime.config.read().unwrap().clone();

    host.handle_configure(
        WireDaemonConfig {
            tui_max_feed_lines: Some(42),
            clear_fields: vec!["not_a_field".into()],
            ..Default::default()
        },
        &mut TurnState::default(),
    )
    .await;

    assert_eq!(host.runtime.feed_history_limit, before.tui_max_feed_lines);
    assert_eq!(*host.runtime.config.read().unwrap(), before);
}

#[tokio::test]
async fn handle_configure_clear_and_set_follow_patch_precedence() {
    let mut fixture = HostFixture::new().await;
    let host = fixture.host();

    host.handle_configure(
        WireDaemonConfig {
            thinking: Some(true),
            tui_max_feed_lines: Some(42),
            clear_fields: vec!["thinking".into(), "tui_max_feed_lines".into()],
            ..Default::default()
        },
        &mut TurnState::default(),
    )
    .await;

    let view = host.runtime.config.read().unwrap().clone();
    assert_eq!(view.thinking, Some(true));
    assert_eq!(view.tui_max_feed_lines, Some(42));
    assert_eq!(host.runtime.feed_history_limit, Some(42));

    host.handle_configure(
        WireDaemonConfig {
            clear_fields: vec!["thinking".into(), "tui_max_feed_lines".into()],
            ..Default::default()
        },
        &mut TurnState::default(),
    )
    .await;

    let view = host.runtime.config.read().unwrap().clone();
    assert_eq!(view.thinking, None);
    assert_eq!(view.tui_max_feed_lines, None);
    assert_eq!(host.runtime.feed_history_limit, None);
}

#[tokio::test]
async fn handle_configure_provisions_skill_catalog_and_reload_keeps_it() {
    let mut fixture = HostFixture::new().await;
    let host = fixture.host();

    let mut patch = WireDaemonConfig::default();
    patch.skills = vec![
        theway_transport::wire::WireProvisionedSkill {
            name: "provisioned-skill".into(),
            description: "provisioned by the controller".into(),
            content: "body".into(),
            file_path: "/tmp/provisioned-skill/SKILL.md".into(),
            source: "user".into(),
            disable_model_invocation: false,
        },
        theway_transport::wire::WireProvisionedSkill {
            name: "project-skill".into(),
            description: "project layer".into(),
            content: "body".into(),
            file_path: "/tmp/project-skill/SKILL.md".into(),
            source: "project".into(),
            disable_model_invocation: false,
        },
    ];
    host.handle_configure(patch.clone(), &mut TurnState::default())
        .await;

    let skills = host.session.kernel.harness().skills();
    let provisioned = skills
        .iter()
        .find(|skill| skill.name == "provisioned-skill")
        .expect("provisioned skill must land in the harness catalog");
    assert_eq!(provisioned.content, "body");
    assert!(matches!(provisioned.source, theway_core::SkillSource::User));
    let project = skills
        .iter()
        .find(|skill| skill.name == "project-skill")
        .expect("project skill must land in the harness catalog");
    assert!(matches!(
        project.source,
        theway_core::SkillSource::Project
    ));

    // The shared slot mirrors the catalog and the config view echoes it.
    assert_eq!(host.runtime.provisioned_skills.read().unwrap().len(), 2);
    assert_eq!(host.runtime.config.read().unwrap().skills, patch.skills);
}

#[tokio::test]
async fn handle_configure_clear_skills_empties_the_catalog() {
    let mut fixture = HostFixture::new().await;
    let host = fixture.host();

    let mut patch = WireDaemonConfig::default();
    patch.skills = vec![theway_transport::wire::WireProvisionedSkill {
        name: "ephemeral-skill".into(),
        description: "temp".into(),
        content: "body".into(),
        file_path: "/tmp/ephemeral/SKILL.md".into(),
        source: "user".into(),
        disable_model_invocation: false,
    }];
    host.handle_configure(patch, &mut TurnState::default()).await;
    assert!(
        host.session
            .kernel
            .harness()
            .skills()
            .iter()
            .any(|skill| skill.name == "ephemeral-skill")
    );

    host.handle_configure(
        WireDaemonConfig {
            clear_fields: vec!["skills".into()],
            ..Default::default()
        },
        &mut TurnState::default(),
    )
    .await;

    assert!(
        host.session
            .kernel
            .harness()
            .skills()
            .iter()
            .all(|skill| skill.name != "ephemeral-skill"),
        "cleared skills must leave the catalog"
    );
    assert!(host.runtime.provisioned_skills.read().unwrap().is_empty());
    assert!(
        host.runtime.config.read().unwrap().skills.is_empty(),
        "the config view reflects the cleared catalog"
    );
}

#[tokio::test]
async fn handle_configure_provisions_template_catalog_and_echoes_it() {
    let mut fixture = HostFixture::new().await;
    let host = fixture.host();

    let mut patch = WireDaemonConfig::default();
    patch.templates = vec![
        theway_transport::wire::WireProvisionedTemplate {
            name: "provisioned-template".into(),
            description: "provisioned by the controller".into(),
            content: "template body".into(),
            file_path: "/tmp/provisioned-template.md".into(),
        },
        theway_transport::wire::WireProvisionedTemplate {
            name: "plain-template".into(),
            description: "   ".into(),
            content: "plain body".into(),
            file_path: "/tmp/plain-template.md".into(),
        },
    ];
    host.handle_configure(patch.clone(), &mut TurnState::default())
        .await;

    let templates = host.session.kernel.harness().templates();
    let provisioned = templates
        .iter()
        .find(|template| template.name == "provisioned-template")
        .expect("provisioned template must land in the harness catalog");
    assert_eq!(provisioned.content, "template body");
    assert_eq!(provisioned.file_path, "/tmp/provisioned-template.md");
    assert_eq!(
        provisioned.description.as_deref(),
        Some("provisioned by the controller")
    );
    let plain = templates
        .iter()
        .find(|template| template.name == "plain-template")
        .expect("plain template must land in the harness catalog");
    assert_eq!(
        plain.description, None,
        "a whitespace-only description maps to None"
    );

    // The shared slot mirrors the catalog and the config view echoes it.
    assert_eq!(host.runtime.provisioned_templates.read().unwrap().len(), 2);
    assert_eq!(host.runtime.config.read().unwrap().templates, patch.templates);
}

#[tokio::test]
async fn handle_configure_clear_templates_empties_the_catalog() {
    let mut fixture = HostFixture::new().await;
    let host = fixture.host();

    let mut patch = WireDaemonConfig::default();
    patch.templates = vec![theway_transport::wire::WireProvisionedTemplate {
        name: "ephemeral-template".into(),
        description: "temp".into(),
        content: "body".into(),
        file_path: "/tmp/ephemeral-template.md".into(),
    }];
    host.handle_configure(patch, &mut TurnState::default()).await;
    assert!(
        host.session
            .kernel
            .harness()
            .templates()
            .iter()
            .any(|template| template.name == "ephemeral-template")
    );

    host.handle_configure(
        WireDaemonConfig {
            clear_fields: vec!["templates".into()],
            ..Default::default()
        },
        &mut TurnState::default(),
    )
    .await;

    assert!(
        host.session
            .kernel
            .harness()
            .templates()
            .iter()
            .all(|template| template.name != "ephemeral-template"),
        "cleared templates must leave the catalog"
    );
    assert!(host.runtime.provisioned_templates.read().unwrap().is_empty());
    assert!(
        host.runtime.config.read().unwrap().templates.is_empty(),
        "the config view reflects the cleared catalog"
    );
}
