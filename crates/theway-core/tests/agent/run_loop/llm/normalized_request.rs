use super::*;
use crate::agent::model_request::NormalizedModelRequestDraft;
use crate::types::{TransformContext, TransformModelRequest};

type CapturedRequest = (
    PiContext,
    Option<f32>,
    Option<u32>,
    Option<theway_llm_provider::ThinkingLevel>,
);

fn request_tool(name: &str) -> Arc<dyn crate::types::AgentTool> {
    Arc::new(SysPromptTool {
        def: theway_llm_provider::Tool {
            name: name.into(),
            description: format!("{name} description"),
            parameters: serde_json::json!({"type": "object"}),
        },
    })
}

fn capture_stream(captured: Arc<Mutex<Option<CapturedRequest>>>) -> StreamFn {
    Arc::new(move |_, context, options| {
        let options = options.expect("normalized request options");
        *captured.lock().unwrap() = Some((
            context.clone(),
            options.base.temperature,
            options.base.max_tokens,
            options.reasoning,
        ));
        done_stream("ok")
    })
}

#[tokio::test]
async fn normalized_request_transform_runs_after_context_and_before_serialization() {
    let captured = Arc::new(Mutex::new(None));
    let transform_context: TransformContext = Arc::new(|mut messages, _| {
        Box::pin(async move {
            messages.push(user_message("ephemeral"));
            messages
        })
    });
    let transform_model_request: TransformModelRequest = Arc::new(
        |mut request: NormalizedModelRequestDraft, _| {
            Box::pin(async move {
                assert_eq!(request.messages.len(), 2);
                request.system_instructions = Some("request-local system".into());
                request
                    .visible_tools
                    .retain(|tool| tool.name == "allowed");
                request.visible_tools[0].description = "patched schema metadata".into();
                request
                    .executable_tool_names
                    .retain(|name| name == "allowed");
                request.generation_options.temperature = Some(0.25);
                request.generation_options.max_tokens = Some(512);
                request.generation_options.reasoning =
                    Some(theway_llm_provider::ThinkingLevel::High);
                request
            })
        },
    );
    let mut state = AgentState::default();
    state.model = Some(faux_model());
    state.system_prompt = "base system".into();
    state.messages = vec![user_message("persisted")];
    state.tools = vec![request_tool("allowed"), request_tool("hidden")];
    let agent = Agent::new(AgentOptions {
        initial_state: Some(state),
        transform_context: Some(transform_context),
        transform_model_request: Some(transform_model_request),
        stream_fn: Some(capture_stream(Arc::clone(&captured))),
        ..Default::default()
    });

    let call = call_llm(
        &agent.inner,
        &CancellationToken::new(),
        &CancellationToken::new(),
    )
    .await
    .unwrap();

    let captured = captured.lock().unwrap().clone().unwrap();
    assert_eq!(captured.0.system_prompt.as_deref(), Some("request-local system"));
    assert_eq!(captured.0.messages.len(), 2);
    let tools = captured.0.tools.unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "allowed");
    assert_eq!(tools[0].description, "patched schema metadata");
    assert_eq!(captured.1, Some(0.25));
    assert_eq!(captured.2, Some(512));
    assert_eq!(
        captured.3,
        Some(theway_llm_provider::ThinkingLevel::High)
    );
    assert_eq!(call.executable_tools.len(), 1);
    assert_eq!(call.executable_tools[0].definition().name, "allowed");
    assert_eq!(agent.state().tools.len(), 2);
    assert_eq!(agent.state().system_prompt, "base system");
}

#[tokio::test]
async fn invalid_normalized_request_patch_keeps_the_complete_base_request() {
    let captured = Arc::new(Mutex::new(None));
    let transform_model_request: TransformModelRequest = Arc::new(|mut request, _| {
        Box::pin(async move {
            request.provider = "spoofed-provider".into();
            request.system_instructions = Some("must not leak".into());
            request.visible_tools.clear();
            request.executable_tool_names.clear();
            request.generation_options.max_tokens = Some(1);
            request
        })
    });
    let mut state = AgentState::default();
    state.model = Some(faux_model());
    state.system_prompt = "base system".into();
    state.tools = vec![request_tool("one"), request_tool("two")];
    let agent = Agent::new(AgentOptions {
        initial_state: Some(state),
        transform_model_request: Some(transform_model_request),
        stream_fn: Some(capture_stream(Arc::clone(&captured))),
        ..Default::default()
    });

    let call = call_llm(
        &agent.inner,
        &CancellationToken::new(),
        &CancellationToken::new(),
    )
    .await
    .unwrap();

    let captured = captured.lock().unwrap().clone().unwrap();
    assert_eq!(captured.0.system_prompt.as_deref(), Some("base system"));
    assert_eq!(captured.0.tools.unwrap().len(), 2);
    assert_eq!(captured.2, None);
    assert_eq!(call.executable_tools.len(), 2);
}
