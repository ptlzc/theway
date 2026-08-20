use std::sync::Arc;

use futures::StreamExt;
use serde_json::Value;
use tempfile::tempdir;
use theway_core::agent::runtime_extensions::RuntimeRequestExtensionPort;
use theway_daemon::ts_extensions::RuntimeExtensionHostConfig;
use theway_llm_provider::{
    Api, AssistantMessageEvent, Context, KnownApi, Message, Model, ModelCost, Provider,
    StreamOptions, Tool, UserContent, UserMessage, UserRole, stream,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::oneshot;

use super::support::*;

async fn serve_once(sse: &'static str) -> (String, oneshot::Receiver<Value>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (sender, receiver) = oneshot::channel();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let header_end = loop {
            let mut chunk = [0_u8; 4096];
            let count = socket.read(&mut chunk).await.unwrap();
            request.extend_from_slice(&chunk[..count]);
            if let Some(offset) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                break offset + 4;
            }
        };
        let head = String::from_utf8(request[..header_end].to_vec()).unwrap();
        let content_length = head
            .lines()
            .find_map(|line| {
                line.split_once(':').and_then(|(name, value)| {
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                })
            })
            .unwrap();
        while request.len() - header_end < content_length {
            let mut chunk = [0_u8; 4096];
            let count = socket.read(&mut chunk).await.unwrap();
            request.extend_from_slice(&chunk[..count]);
        }
        sender
            .send(
                serde_json::from_slice(&request[header_end..header_end + content_length]).unwrap(),
            )
            .unwrap();
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            sse.len(),
            sse
        );
        socket.write_all(response.as_bytes()).await.unwrap();
    });
    (format!("http://{address}"), receiver)
}

fn model(api: KnownApi, provider: &str, base_url: String) -> Model {
    Model {
        id: "test-model".into(),
        name: "Anchor format fixture".into(),
        api: Api::known(api),
        provider: Provider(provider.into()),
        base_url,
        reasoning: false,
        thinking_level_map: None,
        input: Vec::new(),
        cost: ModelCost::default(),
        context_window: 128_000,
        max_tokens: 8192,
        headers: None,
        compat: None,
    }
}

fn assert_bootstrap_wire(body: &Value) {
    let serialized = serde_json::to_string(body).unwrap();
    assert!(serialized.contains("ANCHOR BOOTSTRAP"), "{body}");
    assert!(!serialized.contains("BASE SYSTEM"), "{body}");
    assert!(!serialized.contains("retained"), "{body}");
    let tools = body["tools"].as_array().unwrap();
    let names = tools
        .iter()
        .filter_map(|tool| {
            tool.get("name")
                .or_else(|| {
                    tool.get("function")
                        .and_then(|function| function.get("name"))
                })
                .and_then(Value::as_str)
        })
        .collect::<Vec<_>>();
    assert_eq!(names, ["bash", "str_replace_editor"]);
    assert!(
        ["max_tokens", "max_output_tokens", "max_completion_tokens"]
            .iter()
            .any(|field| body.get(*field).and_then(Value::as_u64) == Some(4096)),
        "{body}"
    );
}

async fn capture_format(
    host: &theway_daemon::ts_extensions::SessionPluginHost,
    sequence: u64,
    api: KnownApi,
    provider: &str,
    sse: &'static str,
) -> Value {
    let definitions = merged_tool_definitions(host, Vec::new());
    let batch = RuntimeRequestExtensionPort::invoke_request(
        host,
        request_invocation(sequence, provider, "test-model", definitions, Some(4096)),
    )
    .await
    .unwrap();
    let request = replacement(&batch).unwrap();
    let tools: Vec<Tool> = serde_json::from_value(request["visibleTools"].clone()).unwrap();
    let context = Context {
        system_prompt: request["systemInstructions"].as_str().map(String::from),
        messages: serde_json::from_value(request["messages"].clone()).unwrap(),
        tools: Some(tools),
    };
    serialize_context(api, provider, context, sse).await
}

async fn serialize_context(
    api: KnownApi,
    provider: &str,
    context: Context,
    sse: &'static str,
) -> Value {
    let (base_url, captured) = serve_once(sse).await;
    let options = StreamOptions {
        api_key: Some("fixture-key".into()),
        max_tokens: Some(4096),
        max_retries: Some(0),
        ..Default::default()
    };
    let mut events = stream(&model(api, provider, base_url), &context, Some(&options));
    while let Some(event) = events.next().await {
        if let AssistantMessageEvent::Error { error, .. } = event {
            panic!("provider fixture failed: {:?}", error.error_message);
        }
    }
    captured.await.unwrap()
}

fn provider_cases() -> [(KnownApi, &'static str, &'static str); 3] {
    [
        (
            KnownApi::OpenAICompletions,
            "openai",
            "data: {\"id\":\"chat_1\",\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
        ),
        (
            KnownApi::OpenAIResponses,
            "openai",
            "event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_1\"}}\n\nevent: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}}\n\n",
        ),
        (
            KnownApi::AnthropicMessages,
            "anthropic",
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\nevent: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":1}}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
        ),
    ]
}

#[tokio::test]
async fn normalized_bootstrap_serializes_equivalently_across_supported_formats() {
    let project = tempdir().unwrap();
    let base = tempdir().unwrap();
    install_anchor(project.path(), &enabled_config());
    let (host, _) = start_anchor(
        project.path(),
        base.path(),
        Arc::new(MemoryStatePort::default()),
        false,
        RuntimeExtensionHostConfig::default(),
    )
    .await;
    let mut bodies = Vec::new();
    for (sequence, (api, provider, sse)) in provider_cases().into_iter().enumerate() {
        bodies.push(capture_format(&host, sequence as u64 + 1, api, provider, sse).await);
    }
    for body in bodies {
        assert_bootstrap_wire(&body);
    }
    host.shutdown().await;
}

#[tokio::test]
async fn promoted_context_and_full_catalog_serialize_across_supported_formats() {
    let project = tempdir().unwrap();
    let base = tempdir().unwrap();
    install_anchor(project.path(), &enabled_config());
    let (host, _) = start_anchor(
        project.path(),
        base.path(),
        Arc::new(MemoryStatePort::default()),
        false,
        RuntimeExtensionHostConfig::default(),
    )
    .await;
    let mut base_tools = vec![compatible_bash()];
    base_tools.push(Arc::new(TestTool::new(
        "unrelated",
        serde_json::json!({"type": "object"}),
    )));
    let definitions = merged_tool_definitions(&host, base_tools);
    RuntimeRequestExtensionPort::invoke_request(
        &*host,
        request_invocation(
            1,
            "deepseek",
            "deepseek-chat",
            definitions.clone(),
            Some(4096),
        ),
    )
    .await
    .unwrap();
    theway_core::agent::runtime_extensions::RuntimeMessageExtensionPort::invoke_message(
        &*host,
        assistant_invocation(2, "deepseek", "deepseek-chat", "ready"),
    )
    .await
    .unwrap();

    for (sequence, (api, provider, sse)) in provider_cases().into_iter().enumerate() {
        let result = RuntimeRequestExtensionPort::invoke_request(
            &*host,
            request_invocation(
                sequence as u64 + 3,
                provider,
                "test-model",
                definitions.clone(),
                Some(4096),
            ),
        )
        .await
        .unwrap();
        assert!(replacement(&result).is_none());
        let projection = host.model_context_projection();
        let mut draft = theway_core::agent::model_request::NormalizedModelRequestDraft {
            provider: provider.into(),
            model: "test-model".into(),
            system_instructions: Some("BASE SYSTEM".into()),
            messages: vec![Message::User(UserMessage {
                role: UserRole::User,
                content: UserContent::Text("retained".into()),
                timestamp: 0,
            })],
            visible_tools: definitions.clone(),
            executable_tool_names: definitions.iter().map(|tool| tool.name.clone()).collect(),
            generation_options: Default::default(),
        };
        projection.apply_to_request(&mut draft);
        let body = serialize_context(
            api,
            provider,
            Context {
                system_prompt: draft.system_instructions,
                messages: draft.messages,
                tools: Some(draft.visible_tools),
            },
            sse,
        )
        .await;
        let serialized = serde_json::to_string(&body).unwrap();
        assert!(serialized.contains("BASE SYSTEM"), "{body}");
        assert!(serialized.contains(RESTORED_CONTEXT), "{body}");
        assert!(serialized.contains("retained"), "{body}");
        assert!(!serialized.contains("ANCHOR BOOTSTRAP"), "{body}");
        assert_eq!(body["tools"].as_array().unwrap().len(), 3);
    }
    host.shutdown().await;
}
