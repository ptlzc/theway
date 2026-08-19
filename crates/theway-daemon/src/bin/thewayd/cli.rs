#[derive(Parser, Debug)]
#[command(
    name = "thewayd",
    version,
    about = "theway headless daemon (gRPC/HTTP/MCP server)"
)]
struct Cli {
    /// Serve gRPC (default).
    #[arg(long, conflicts_with_all = ["http", "mcp"])]
    grpc: bool,
    /// Serve the HTTP/WS UI instead of gRPC.
    #[arg(long, conflicts_with_all = ["grpc", "mcp"])]
    http: bool,
    /// Serve MCP over stdio instead of gRPC/HTTP.
    #[arg(long, conflicts_with_all = ["grpc", "http"])]
    mcp: bool,
    /// Bind host (loopback recommended). Defaults to 127.0.0.1.
    #[arg(long = "host", default_value = "127.0.0.1")]
    host: String,
    /// Bind port. Defaults to 44777; 0 = random free port (published to the
    /// port file so clients can find it).
    #[arg(long = "port", default_value = "44777")]
    port: u16,
    /// Working directory for the daemon (session repo + tool execution). Defaults
    /// to the current directory.
    #[arg(long)]
    cwd: Option<std::path::PathBuf>,
    /// User home directory (user-level `.agents` / `.claude` config + skill
    /// roots). Defaults to `$HOME` (issue #66: resolved once at this boundary).
    #[arg(long)]
    home: Option<std::path::PathBuf>,
    /// Extra skill directory to load skills from. Repeatable.
    #[arg(long = "skills-dir")]
    skills_dir: Vec<std::path::PathBuf>,
    /// Provider id (anthropic, openai, openrouter, …). When unset, auto-detected from env.
    #[arg(long)]
    provider: Option<String>,
    /// Model id within the provider's catalog.
    #[arg(long)]
    model: Option<String>,
    /// Override the selected model's base URL.
    #[arg(long)]
    base_url: Option<String>,
    /// Thinking level.
    #[arg(long, default_value = "off")]
    thinking: String,
    /// Resume a specific session by id (full UUIDv7 or unique prefix).
    #[arg(long)]
    resume_id: Option<String>,
    /// Continue the most recent session for this cwd.
    #[arg(long, short = 'c')]
    continue_: bool,
    /// Auto-approve control-plane prompts.
    #[arg(long)]
    yes: bool,
    /// Auto-approve every approval prompt, including control-plane writes.
    #[arg(long)]
    always_allow: bool,
    /// Show LLM call debug logs in the conversation feed.
    #[arg(long)]
    debug: bool,
    /// Poll interval for local dynamic trigger checks, in seconds.
    #[arg(long)]
    trigger_poll_secs: Option<u64>,
    /// Enable built-in skills by name. Repeatable.
    #[arg(long)]
    builtin_skill: Vec<String>,
    /// Controller StorageService endpoint (`host:port`) for controller-backed
    /// runtime storage (issue #85). When unset, the daemon uses local storage.
    #[arg(long = "storage-service-addr")]
    storage_service_addr: Option<String>,
}
