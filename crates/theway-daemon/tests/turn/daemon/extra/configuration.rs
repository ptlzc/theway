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
