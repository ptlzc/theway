//! Shell-output truncation helpers used by tool results that wrap process output.
//! TODO: 1:1 port of `packages/agent/src/harness/utils/shell-output.ts` (~143 lines).

/// Stub passthrough — replace with the real ANSI-aware, head-and-tail truncator.
pub fn truncate_shell_output(stdout: &str, _stderr: &str, max_chars: usize) -> String {
    crate::harness::utils::truncate::truncate_text(stdout, max_chars)
}
