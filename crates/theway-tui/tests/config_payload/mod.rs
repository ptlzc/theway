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

[builtin_skills]
enabled = [\"debugging\", \"code-review\"]

[triggers]
poll_interval_secs = 45

[tui]
max_feed_lines = 8000
";
