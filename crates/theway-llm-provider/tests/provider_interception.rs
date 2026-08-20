use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::StreamExt;
use serde_json::Value;
use theway_llm_provider::{
    Api, AssistantMessageEvent, Context, KnownApi, Message, Model, ModelCost, Provider,
    ProviderInterceptionError, ProviderRequestFailure, ProviderRequestHeaders,
    ProviderRequestInterceptor, ProviderRequestInterceptorHandle, ProviderRequestPayload,
    ProviderResponseMetadata, ProviderWireFormat, StreamOptions, UserContent, UserMessage,
    UserRole, stream,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::oneshot;

struct CapturedRequest {
    head: String,
    body: Value,
}

async fn serve_once(sse: &'static str) -> (String, oneshot::Receiver<CapturedRequest>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (capture_tx, capture_rx) = oneshot::channel();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let header_end = loop {
            let mut chunk = [0u8; 4096];
            let count = socket.read(&mut chunk).await.unwrap();
            assert!(count > 0, "request ended before headers");
            request.extend_from_slice(&chunk[..count]);
            if let Some(end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                break end + 4;
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
            .unwrap_or(0);
        while request.len() - header_end < content_length {
            let mut chunk = [0u8; 4096];
            let count = socket.read(&mut chunk).await.unwrap();
            assert!(count > 0, "request ended before body");
            request.extend_from_slice(&chunk[..count]);
        }
        let body =
            serde_json::from_slice(&request[header_end..header_end + content_length]).unwrap();
        let _ = capture_tx.send(CapturedRequest { head, body });
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nX-Test-Response: visible\r\nSet-Cookie: response-secret\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            sse.len(),
            sse
        );
        socket.write_all(response.as_bytes()).await.unwrap();
    });
    (format!("http://{addr}"), capture_rx)
}

#[derive(Default)]
struct InterceptionState {
    order: Vec<&'static str>,
    headers: Option<ProviderRequestHeaders>,
    payload: Option<ProviderRequestPayload>,
    response: Option<ProviderResponseMetadata>,
    failures: Vec<ProviderRequestFailure>,
}

struct RecordingInterceptor {
    state: Mutex<InterceptionState>,
    wrong_payload_format: Option<ProviderWireFormat>,
}

impl RecordingInterceptor {
    fn new() -> Self {
        Self {
            state: Mutex::new(InterceptionState::default()),
            wrong_payload_format: None,
        }
    }
}

#[async_trait]
impl ProviderRequestInterceptor for RecordingInterceptor {
    async fn transform_headers(
        &self,
        mut request: ProviderRequestHeaders,
    ) -> Result<ProviderRequestHeaders, ProviderInterceptionError> {
        let mut state = self.state.lock().unwrap();
        state.order.push("headers");
        state.headers = Some(request.clone());
        request
            .headers
            .insert("x-hook-order".into(), "headers".into());
        Ok(request)
    }

    async fn transform_payload(
        &self,
        mut request: ProviderRequestPayload,
    ) -> Result<ProviderRequestPayload, ProviderInterceptionError> {
        let mut state = self.state.lock().unwrap();
        state.order.push("payload");
        state.payload = Some(request.clone());
        request.payload["intercepted"] = Value::Bool(true);
        if let Some(format) = self.wrong_payload_format {
            request.format = format;
            request.payload["cross_format"] = Value::Bool(true);
        }
        Ok(request)
    }

    async fn observe_response(&self, response: ProviderResponseMetadata) {
        let mut state = self.state.lock().unwrap();
        state.order.push("response");
        state.response = Some(response);
    }

    async fn observe_request_failure(&self, failure: ProviderRequestFailure) {
        let mut state = self.state.lock().unwrap();
        state.order.push("failure");
        state.failures.push(failure);
    }
}

fn context() -> Context {
    Context {
        system_prompt: Some("system".into()),
        messages: vec![Message::User(UserMessage {
            role: UserRole::User,
            content: UserContent::Text("hello".into()),
            timestamp: 0,
        })],
        tools: None,
    }
}

fn model(api: KnownApi, provider: &str, base_url: String) -> Model {
    Model {
        id: "test-model".into(),
        name: "Test Model".into(),
        api: Api::known(api),
        provider: Provider::from(provider),
        base_url,
        reasoning: false,
        thinking_level_map: None,
        input: Vec::new(),
        cost: ModelCost::default(),
        context_window: 128_000,
        max_tokens: 1024,
        headers: None,
        compat: None,
    }
}

async fn run_case(
    api: KnownApi,
    provider: &str,
    format: ProviderWireFormat,
    sse: &'static str,
) -> (CapturedRequest, Arc<RecordingInterceptor>) {
    let (base_url, captured) = serve_once(sse).await;
    let interceptor = Arc::new(RecordingInterceptor::new());
    let mut headers = HashMap::new();
    headers.insert("x-public-option".into(), "public".into());
    headers.insert("x-secret-token".into(), "configured-secret".into());
    let options = StreamOptions {
        api_key: Some("provider-secret".into()),
        headers: Some(headers),
        max_retries: Some(0),
        request_interceptor: Some(ProviderRequestInterceptorHandle::new(interceptor.clone())),
        ..Default::default()
    };
    let mut events = stream(&model(api, provider, base_url), &context(), Some(&options));
    let mut saw_terminal = false;
    while let Some(event) = events.next().await {
        match event {
            AssistantMessageEvent::Done { .. } => saw_terminal = true,
            AssistantMessageEvent::Error { error, .. } => {
                panic!("unexpected provider error: {:?}", error.error_message)
            }
            _ => {}
        }
    }
    assert!(saw_terminal);
    let captured = captured.await.unwrap();
    let state = interceptor.state.lock().unwrap();
    assert_eq!(state.order, ["headers", "payload", "response"]);
    let seen_headers = state.headers.as_ref().unwrap();
    assert_eq!(seen_headers.format, format);
    assert!(!seen_headers.headers.keys().any(|name| {
        name.contains("authorization")
            || name.contains("api-key")
            || name.contains("token")
            || name.contains("secret")
    }));
    let response = state.response.as_ref().unwrap();
    assert_eq!(response.format, format);
    assert_eq!(response.status, 200);
    assert_eq!(
        response.headers.get("x-test-response").map(String::as_str),
        Some("visible")
    );
    assert!(!response.headers.contains_key("set-cookie"));
    assert!(state.failures.is_empty());
    drop(state);
    assert!(
        captured
            .head
            .to_ascii_lowercase()
            .contains("x-hook-order: headers")
    );
    assert_eq!(captured.body["intercepted"], true);
    (captured, interceptor)
}

#[cfg(feature = "openai-completions")]
#[tokio::test]
async fn openai_chat_serialization_runs_header_raw_and_response_hooks_in_order() {
    let sse = "data: {\"id\":\"chat_1\",\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
    let (captured, _) = run_case(
        KnownApi::OpenAICompletions,
        "openai",
        ProviderWireFormat::OpenAiChatCompletions,
        sse,
    )
    .await;
    assert!(captured.body["messages"].is_array());
}

#[cfg(feature = "openai-responses")]
#[tokio::test]
async fn openai_responses_serialization_runs_header_raw_and_response_hooks_in_order() {
    let sse = "event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_1\"}}\n\nevent: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}}\n\n";
    let (captured, _) = run_case(
        KnownApi::OpenAIResponses,
        "openai",
        ProviderWireFormat::OpenAiResponses,
        sse,
    )
    .await;
    assert!(captured.body["input"].is_array());
}

#[cfg(feature = "anthropic")]
#[tokio::test]
async fn anthropic_serialization_runs_header_raw_and_response_hooks_in_order() {
    let sse = "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\nevent: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":1}}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";
    let (captured, _) = run_case(
        KnownApi::AnthropicMessages,
        "anthropic",
        ProviderWireFormat::AnthropicMessages,
        sse,
    )
    .await;
    assert!(captured.body["messages"].is_array());
    assert_eq!(captured.body["max_tokens"], 1024);
}

#[cfg(feature = "openai-completions")]
#[tokio::test]
async fn raw_payload_from_another_format_is_rejected_without_partial_replacement() {
    let sse = "data: {\"id\":\"chat_1\",\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
    let (base_url, captured) = serve_once(sse).await;
    let interceptor = Arc::new(RecordingInterceptor {
        state: Mutex::new(InterceptionState::default()),
        wrong_payload_format: Some(ProviderWireFormat::OpenAiResponses),
    });
    let options = StreamOptions {
        api_key: Some("provider-secret".into()),
        max_retries: Some(0),
        request_interceptor: Some(ProviderRequestInterceptorHandle::new(interceptor)),
        ..Default::default()
    };
    let mut events = stream(
        &model(KnownApi::OpenAICompletions, "openai", base_url),
        &context(),
        Some(&options),
    );
    while events.next().await.is_some() {}

    let captured = captured.await.unwrap();
    assert!(captured.body.get("cross_format").is_none());
    assert!(captured.body.get("intercepted").is_none());
    assert!(captured.body["messages"].is_array());
}

#[cfg(feature = "openai-completions")]
#[tokio::test]
async fn transport_failure_is_observed_without_a_synthetic_response() {
    let interceptor = Arc::new(RecordingInterceptor::new());
    let options = StreamOptions {
        api_key: Some("provider-secret".into()),
        max_retries: Some(0),
        request_interceptor: Some(ProviderRequestInterceptorHandle::new(interceptor.clone())),
        ..Default::default()
    };
    let mut events = stream(
        &model(
            KnownApi::OpenAICompletions,
            "openai",
            "http://[invalid".into(),
        ),
        &context(),
        Some(&options),
    );
    let mut saw_error = false;
    while let Some(event) = events.next().await {
        saw_error |= matches!(event, AssistantMessageEvent::Error { .. });
    }

    assert!(saw_error);
    let state = interceptor.state.lock().unwrap();
    assert_eq!(state.order, ["headers", "payload", "failure"]);
    assert!(state.response.is_none());
    assert_eq!(state.failures.len(), 1);
    assert_eq!(state.failures[0].code, "http_transport");
    assert!(!state.failures[0].message.contains("provider-secret"));
}
