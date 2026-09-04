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
