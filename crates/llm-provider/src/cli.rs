//! `theway-llm-provider` standalone CLI. 1:1 stub of `packages/ai/src/cli.ts`. The TS file is mainly used
//! for ad-hoc testing of provider/model combinations. The Rust binary lives at
//! `src/bin/theway-llm-provider.rs` (TBD); this module currently exposes only a `main_entry()` shim so the
//! file mapping holds.

pub fn main_entry() {
    println!(
        "theway-llm-provider-rs CLI is a TODO. Use `cargo run --example anthropic_hello` for now."
    );
}
