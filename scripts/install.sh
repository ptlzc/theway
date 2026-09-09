#!/usr/bin/env bash
# =============================================================================
# install — build the latest theway release and install it into a bin directory
#
# Usage:
#   scripts/install.sh                    # install into $CARGO_HOME/bin (~/.cargo/bin)
#   scripts/install.sh --root DIR         # install into DIR/bin (cargo install --root semantics)
#   scripts/install.sh --restart-daemon   # restart the old thewayd after install
#   scripts/install.sh --help
#
# Behavior:
#   - `cargo install --path crates/theway-tui --force` builds release and
#     replaces the installed `theway`.
#   - `cargo install --path crates/theway-daemon --force` installs the matching
#     `thewayd`. The TUI discovers the daemon next to `theway` or on PATH, so
#     the two must be built together or discovery can time out at cold start.
#   - `cargo install --path crates/tgrep-cli --force` installs `tgrep`, the
#     optional indexing backend for the built-in grep tool.
#   - A running thewayd is left alone by default: it keeps serving existing
#     sessions and exits a few seconds after its TUI closes; the next start
#     uses the new binary. Pass --restart-daemon to switch immediately.
#   - The `tw` shorthand is created (a copy of the `theway` binary).
#   - The version is printed at the end; a note is shown when the target bin
#     directory is not on PATH.
#
# Dependencies: bash, cargo (rustup or a system install).
# Prefer scripts/install-release.sh when you want prebuilt GitHub Release
# binaries instead of a local Cargo build.
# =============================================================================

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CARGO="${CARGO:-cargo}"
EXE="${EXE:-}" # set .exe on Windows, matching the Makefile convention
RESTART_DAEMON="${RESTART_DAEMON:-}"

usage() {
    sed -n '5,9p' "${BASH_SOURCE[0]}"
}

INSTALL_ROOT=""
while [ $# -gt 0 ]; do
    case "$1" in
        --root)
            INSTALL_ROOT="${2:?--root requires a directory argument}"
            shift 2
            ;;
        --root=*)
            INSTALL_ROOT="${1#--root=}"
            shift
            ;;
        --restart-daemon)
            RESTART_DAEMON=1
            shift
            ;;
        -h | --help)
            usage
            exit 0
            ;;
        *)
            echo "install.sh: unknown argument: $1" >&2
            usage >&2
            exit 2
            ;;
    esac
done

if [ -z "$INSTALL_ROOT" ]; then
    # Cargo's default prefix: $CARGO_HOME first, then ~/.cargo.
    INSTALL_ROOT="${CARGO_HOME:-$HOME/.cargo}"
fi

BIN_DIR="$INSTALL_ROOT/bin"

echo "==> Building and installing theway (release) into $BIN_DIR"
mkdir -p "$BIN_DIR"
# --locked keeps the workspace dependency set: a fresh resolution previously
# produced an incompatible oxc_transformer + oxc-browserslist combination.
"$CARGO" install --path "$ROOT/crates/theway-tui" --force --locked --root "$INSTALL_ROOT"

echo "==> Building and installing thewayd (release) into $BIN_DIR"
"$CARGO" install --path "$ROOT/crates/theway-daemon" --force --locked --root "$INSTALL_ROOT"

echo "==> Building and installing tgrep (release) into $BIN_DIR"
"$CARGO" install --path "$ROOT/crates/tgrep-cli" --force --locked --root "$INSTALL_ROOT"

# ── Default configuration (issue #123) ───────────────────────────────────────
# Seed the controller-owned default config.toml on first install only; an
# existing file is never overwritten.
THEWAY_BASE="${THEWAY_DIR:-$HOME/.theway}"
CONFIG_FILE="$THEWAY_BASE/config.toml"
if [ ! -e "$CONFIG_FILE" ]; then
    mkdir -p "$THEWAY_BASE"
    cat > "$CONFIG_FILE" <<'EOF'
# theway default configuration.
# Missing values fall back to built-in defaults; delete this file to reset.

[executor]
kind = "local"

# Built-in tool backends (issue #135). Uncomment to disable the
# tgrep-accelerated `grep` path; `grep` then always walks the tree.
# [tools]
# tgrep = false

# Example model defaults (DeepSeek official, commented out). Uncomment
# provider/model/thinking and replace with your own values; environment API
# keys still win over `api_key`.
# [model]
# provider = "deepseek"
# model = "deepseek-v4-flash"
# thinking = "medium"
# api_key = "sk-xxxxxx"

# Local OpenAI-compatible server: the daemon imports the catalog from
# `<base_url>/models`, so `model` may stay unset.
# [model]
# provider = "ds4"
# base_url = "http://127.0.0.1:8000/v1"
# auto_fetch_models = true

# Custom model descriptors (optional; override a catalog entry by id).
# [[model.custom]]
# id = "deepseek-v4-flash"
# api = "openai-responses"
# context_window = 100000
# max_tokens = 384000
EOF
    echo "==> Initialized default config at $CONFIG_FILE"
fi

# ── Running daemon handling ──────────────────────────────────────────────────
# Default: do not interrupt a running thewayd. Replacing the binary on disk
# does not affect the already-loaded process image; the daemon exits a few
# seconds after its TUI closes and the next start uses the new binary.
# --restart-daemon keeps the old behavior and switches immediately.
if [ -n "$RESTART_DAEMON" ]; then
    echo "==> Restarting the old thewayd process (other terminal sessions will disconnect)"
    pkill -TERM -x thewayd 2>/dev/null || true
    for _ in 1 2 3 4 5; do
        pgrep -x thewayd >/dev/null 2>&1 || break
        sleep 1
    done
    pkill -KILL -x thewayd 2>/dev/null || true
    # Remove stale port files; a new daemon writes its own.
    rm -f "$THEWAY_BASE"/daemon-port "$THEWAY_BASE"/daemon-port-*
else
    # Remove port files whose pid is gone or no longer thewayd; keep live
    # daemon entries (the daemon cleans them up when it exits).
    for f in "$THEWAY_BASE"/daemon-port-*; do
        [ -e "$f" ] || continue
        pid=$(awk '{print $2}' "$f" 2>/dev/null || true)
        if [ -n "$pid" ] && ! ps -p "$pid" -o comm= 2>/dev/null | grep -qx thewayd; then
            rm -f "$f"
        fi
    done
    if pgrep -x thewayd >/dev/null 2>&1; then
        echo "==> A running thewayd is still serving existing sessions and is unaffected:"
        for pid in $(pgrep -x thewayd); do
            cwd=""
            if [ -r "/proc/$pid/cmdline" ]; then
                cwd=$(tr '\0' ' ' < "/proc/$pid/cmdline" 2>/dev/null \
                    | sed -n 's/.*--cwd \([^ ]*\).*/\1/p')
            fi
            echo "     pid $pid${cwd:+ (cwd $cwd)} — it exits a few seconds after its TUI closes; the next start uses the new binary"
        done
    fi
fi

echo "==> Creating the tw shorthand"
# Copy to a temporary file first and rename: replacing a binary that is
# currently running can hit ETXTBSY on Linux, and rename is atomic.
tmp_tw="$BIN_DIR/.tw.tmp.$$"
cp "$BIN_DIR/theway$EXE" "$tmp_tw"
mv -f "$tmp_tw" "$BIN_DIR/tw$EXE"

echo "==> Done:"
"$BIN_DIR/theway$EXE" --version

case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *)
        echo "Note: $BIN_DIR is not on PATH; add it to your shell profile, for example:" >&2
        echo "  export PATH=\"$BIN_DIR:\$PATH\"" >&2
        ;;
esac
