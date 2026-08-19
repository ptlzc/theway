use std::process::Command;

#[test]
fn help_lists_thinking_possible_values() {
    let output = Command::new(env!("CARGO_BIN_EXE_theway"))
        .arg("--help")
        .output()
        .expect("run theway --help");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("[possible values: off, minimal, low, medium, high, xhigh]"),
        "help should list accepted --thinking values:\n{stdout}"
    );
}

#[test]
fn help_lists_model_catalog_entry_points() {
    let output = Command::new(env!("CARGO_BIN_EXE_theway"))
        .arg("--help")
        .output()
        .expect("run theway --help");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Model catalog:"), "{stdout}");
    assert!(stdout.contains("Supported providers"), "{stdout}");
    assert!(
        stdout.contains("Provider names are model/credential namespaces"),
        "{stdout}"
    );
    assert!(stdout.contains("openai-completions"), "{stdout}");
    assert!(stdout.contains("openai-responses"), "{stdout}");
    assert!(stdout.contains("anthropic-messages"), "{stdout}");
    assert!(stdout.contains("anthropic("), "{stdout}");
    assert!(stdout.contains("openai("), "{stdout}");
    assert!(stdout.contains("~/.theway/models.json"), "{stdout}");
    assert!(stdout.contains("<cwd>/.theway/models.json"), "{stdout}");
    assert!(
        stdout.contains("/model list") || stdout.contains("model list"),
        "{stdout}"
    );
    assert!(!stdout.contains("auth.json"), "{stdout}");
    assert!(!stdout.contains("API_KEY"), "{stdout}");
}

#[test]
fn help_lists_control_plane_yes_flag() {
    let output = Command::new(env!("CARGO_BIN_EXE_theway"))
        .arg("--help")
        .output()
        .expect("run theway --help");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--yes"), "{stdout}");
    assert!(
        stdout.contains("Auto-approve control-plane prompts"),
        "{stdout}"
    );
    assert!(stdout.contains("--always-allow"), "{stdout}");
    assert!(
        stdout.contains("Auto-approve every approval prompt"),
        "{stdout}"
    );
}

#[test]
fn help_lists_tui_flag_and_no_transport_flags() {
    let output = Command::new(env!("CARGO_BIN_EXE_theway"))
        .arg("--help")
        .output()
        .expect("run theway --help");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--tui"), "{stdout}");
    assert!(
        stdout.contains("Explicitly run the terminal UI (the default on a TTY)"),
        "{stdout}"
    );
    // Transport modes moved to the `thewayd` daemon binary — the TUI binary must not
    // offer to start a web/gRPC/MCP server.
    assert!(!stdout.contains("--http"), "{stdout}");
    assert!(!stdout.contains("--grpc"), "{stdout}");
    assert!(!stdout.contains("--mcp"), "{stdout}");
}

#[test]
fn session_import_help_shows_subcommand_options() {
    let output = Command::new(env!("CARGO_BIN_EXE_theway"))
        .args(["session", "import", "--help"])
        .output()
        .expect("run theway session import --help");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Import a `.theway-session` archive"),
        "{stdout}"
    );
    assert!(stdout.contains("--cwd"), "{stdout}");
    assert!(stdout.contains("--activate-triggers"), "{stdout}");
    assert!(stdout.contains("ask is reserved"), "{stdout}");
    assert!(!stdout.contains("Model catalog:"), "{stdout}");
}

#[test]
fn invalid_thinking_value_reports_candidates() {
    let output = Command::new(env!("CARGO_BIN_EXE_theway"))
        .args(["--thinking", "turbo", "--list-sessions"])
        .output()
        .expect("run theway with invalid thinking value");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("invalid value 'turbo'"), "{stderr}");
    assert!(
        stderr.contains("[possible values: off, minimal, low, medium, high, xhigh]"),
        "{stderr}"
    );
}
