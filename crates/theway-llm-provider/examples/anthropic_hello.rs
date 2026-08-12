//! End-to-end smoke test against Anthropic Messages. Requires `ANTHROPIC_API_KEY`.
//!
//! Run: `cargo run --example anthropic_hello --features anthropic`.

use futures::StreamExt;
use theway_llm_provider::{
    Api, Context, KnownApi, Message, Model, ModelCost, Provider, StreamOptions, UserContent,
    UserMessage, UserRole, stream,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if std::env::var("ANTHROPIC_API_KEY").is_err() {
        eprintln!("ANTHROPIC_API_KEY not set — skipping live call.");
        return Ok(());
    }

    let model = Model {
        id: "claude-3-5-haiku-20241022".into(),
        name: "Claude 3.5 Haiku".into(),
        api: Api::known(KnownApi::AnthropicMessages),
        provider: Provider::from("anthropic"),
        base_url: "https://api.anthropic.com".into(),
        reasoning: false,
        thinking_level_map: None,
        input: vec![theway_llm_provider::InputModality::Text],
        cost: ModelCost::default(),
        context_window: 200_000,
        max_tokens: 1024,
        headers: None,
        compat: None,
    };

    let context = Context {
        system_prompt: Some("You are a terse assistant. Reply with one sentence.".into()),
        messages: vec![Message::User(UserMessage {
            role: UserRole::User,
            content: UserContent::Text("Say hi in one short sentence.".into()),
            timestamp: 0,
        })],
        tools: None,
    };

    let opts = StreamOptions {
        max_tokens: Some(64),
        ..Default::default()
    };
    let mut s = stream(&model, &context, Some(&opts));

    while let Some(ev) = s.next().await {
        match ev {
            theway_llm_provider::AssistantMessageEvent::TextDelta { delta, .. } => {
                print!("{}", delta);
            }
            theway_llm_provider::AssistantMessageEvent::Done { message, .. } => {
                println!("\n--- done ---");
                println!(
                    "usage: in={} out={}",
                    message.usage.input, message.usage.output
                );
                break;
            }
            theway_llm_provider::AssistantMessageEvent::Error { error, .. } => {
                eprintln!("error: {:?}", error.error_message);
                break;
            }
            _ => {}
        }
    }
    Ok(())
}
