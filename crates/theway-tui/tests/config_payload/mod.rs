//! Tests for `config_payload` — split out of src (see docs/rust-test-files.md).

use super::*;
use clap::Parser as _;

mod assemble;
mod base;
mod provision;
mod reconcile;

    fn cli_from(args: &[&str]) -> Cli {
        Cli::parse_from(args)
    }

    const FULL_TOML: &str = "\
[model]
provider = \"acme\"
model = \"warp-9\"
thinking = \"high\"
base_url = \"http://127.0.0.1:7777/v1\"
api_key = \"sk-file\"
auto_fetch_models = true

[[model.custom]]
id = \"warp-9-local\"

[builtin_skills]
enabled = [\"debugging\", \"code-review\"]

[triggers]
poll_interval_secs = 45

[tui]
max_feed_lines = 8000

[executor]
kind = \"sandbox\"

[tools]
tgrep = false
";
