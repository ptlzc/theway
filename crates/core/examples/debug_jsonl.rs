use theway_core::{AgentMessage, JsonlSessionRepo};

#[tokio::main]
async fn main() {
    let dir = tempfile::tempdir().unwrap();
    let repo = JsonlSessionRepo::new(dir.path());
    let session = repo.create("/some/cwd").await.unwrap();
    let msg = AgentMessage::Llm(theway_llm_provider::Message::User(
        theway_llm_provider::UserMessage {
            role: theway_llm_provider::UserRole::User,
            content: theway_llm_provider::UserContent::Text("hello".into()),
            timestamp: 12345,
        },
    ));
    session.append_message(msg).await.unwrap();
    let files = repo.list().await.unwrap();
    let content = std::fs::read_to_string(&files[0]).unwrap();
    println!("{}", content);
}
