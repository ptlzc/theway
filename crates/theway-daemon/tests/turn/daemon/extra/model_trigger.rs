use super::*;

// ── model selection / trigger rule / submit text ─────────────────────────────────

#[tokio::test]
async fn set_model_from_spec_switches_to_supported_catalog_model() {
    let mut fixture = HostFixture::new().await;
    let host = fixture.host();
    let model = theway_llm_provider::list_models()
        .into_iter()
        .find(|m| SUPPORTED_APIS.contains(&m.api.0.as_str()))
        .expect("a supported model should exist in the catalog");
    let spec = format!("{}:{}", model.provider.0, model.id);

    host.set_model_from_spec(&spec).await;

    assert_eq!(current_model_label(host.session.kernel.harness()), spec);
}

#[tokio::test]
async fn set_model_from_spec_resolves_unique_bare_id_with_base_url() {
    let mut fixture = HostFixture::new().await;
    let host = fixture.host();
    let Some(model) = theway_llm_provider::list_models().into_iter().find(|model| {
        SUPPORTED_APIS.contains(&model.api.0.as_str())
            && !model.base_url.is_empty()
            && theway_llm_provider::list_models()
                .iter()
                .filter(|candidate| candidate.id == model.id)
                .count()
                == 1
    }) else {
        // The test requires a catalog with a unique base-URL-pinned model;
        // environments with a reduced catalog (e.g. only faux) skip it.
        return;
    };
    host.runtime.config.write().unwrap().base_url = Some(model.base_url.clone());

    host.set_model_from_spec(&model.id).await;

    assert_eq!(
        current_model_label(host.session.kernel.harness()),
        format!("{}:{}", model.provider.0, model.id)
    );
}

#[tokio::test]
async fn trigger_web_rule_now_starts_turn_for_known_rule() {
    let mut fixture = HostFixture::new().await;
    let host = fixture.host();
    let rule = triggers::global_registry()
        .add_rule("condition", "action")
        .unwrap();

    let mut turn = TurnState::default();
    host.trigger_web_rule_now(rule.id.clone(), &mut turn);

    assert!(turn.fut.is_some());
    assert!(host.session.busy);

    triggers::global_registry().remove_rule(&rule.id).unwrap();
}

#[tokio::test]
async fn submit_web_text_interrupt_queues_when_turn_is_running() {
    let mut fixture = HostFixture::new().await;
    let host = fixture.host();
    let mut turn = sample_turn_with_future();

    host.submit_web_text("hello".into(), Vec::new(), true, &mut turn)
        .await;

    assert!(turn.aborted);
    assert_eq!(host.session.queue.len(), 1);
}

#[tokio::test]
async fn submit_web_text_slash_input_dispatches_without_starting_turn() {
    let mut fixture = HostFixture::new().await;
    let host = fixture.host();
    let mut turn = TurnState::default();

    host.submit_web_text("/model".into(), Vec::new(), false, &mut turn)
        .await;

    assert!(turn.fut.is_none());
}

