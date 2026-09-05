use super::*;

#[tokio::test]
async fn handle_web_command_routes_submit_to_start_turn() {
    let mut fixture = HostFixture::new().await;
    let host = fixture.host();
    let mut turn = TurnState::default();

    host.handle_web_command(
        WireCommand::Submit {
            session_id: "sess-extra".into(),
            text: "hello".into(),
            images: Vec::new(),
            interrupt: false,
        },
        &mut turn,
    )
    .await;

    assert!(turn.fut.is_some());
    assert!(host.session.busy);
}

#[tokio::test]
async fn handle_web_command_routes_trigger_rule_now_to_start_turn() {
    let mut fixture = HostFixture::new().await;
    let host = fixture.host();
    let rule = triggers::global_registry()
        .add_rule("condition", "action")
        .unwrap();

    let mut turn = TurnState::default();
    host.handle_web_command(
        WireCommand::TriggerRuleNow { id: rule.id.clone() },
        &mut turn,
    )
    .await;

    assert!(turn.fut.is_some());
    assert!(host.session.busy);

    triggers::global_registry().remove_rule(&rule.id).unwrap();
}
