//! Hello-world against GPT-5.5 via the OpenAI Responses API.
//!
//! Run: `OPENAI_API_KEY=sk-... cargo run --example gpt55_hello --features openai-responses`

use futures::StreamExt;
use theway_llm_provider::{
    AssistantMessageEvent, Context, Message, Provider, StreamOptions, UserContent, UserMessage,
    UserRole, get_model, stream,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if std::env::var("OPENAI_API_KEY").is_err() {
        eprintln!("OPENAI_API_KEY not set — skipping live call.");
        return Ok(());
    }

    // Pull the model record straight from the embedded catalog.
    let model = get_model(&Provider::from("openai"), "gpt-5.5")
        .ok_or_else(|| anyhow::anyhow!("gpt-5.5 not in the model catalog"))?;

    let context = Context {
        system_prompt: Some("You are terse. Reply with one short sentence.".into()),
        messages: vec![Message::User(UserMessage {
            role: UserRole::User,
            content: UserContent::Text("Say hello world.".into()),
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
            AssistantMessageEvent::TextDelta { delta, .. } => {
                use std::io::Write;
                print!("{delta}");
                std::io::stdout().flush().ok();
            }
            AssistantMessageEvent::Done { message, .. } => {
                println!("\n--- done ---");
                println!(
                    "model: {}   usage: in={} out={} cacheRead={}",
                    message.model,
                    message.usage.input,
                    message.usage.output,
                    message.usage.cache_read
                );
                break;
            }
            AssistantMessageEvent::Error { error, .. } => {
                eprintln!("error: {:?}", error.error_message);
                break;
            }
            _ => {}
        }
    }
    Ok(())
}
