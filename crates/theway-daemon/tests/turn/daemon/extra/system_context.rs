use super::*;

/// The authoritative snapshot publishes the full rendered system prompt the
/// next request will use — the same value held by `AgentState.system_prompt`.
#[tokio::test]
async fn wire_snapshot_publishes_live_system_context() {
    let mut fixture = HostFixture::new().await;
    let host = fixture.host();

    let prompt = "<harness>test harness</harness>\n<tools>read, write</tools>";
    host.session
        .kernel
        .harness()
        .agent()
        .state()
        .system_prompt = prompt.to_string();

    let snapshot = host.wire_snapshot();
    assert_eq!(snapshot.system_context, prompt);
}
