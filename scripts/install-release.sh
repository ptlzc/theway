#!/usr/bin/env bash
# =============================================================================
# install-release — install/update theway from prebuilt GitHub Release binaries
#
# Usage:
#   scripts/install-release.sh                    # install/update into ~/.cargo/bin
#   scripts/install-release.sh --bin-dir DIR      # install into DIR
#   scripts/install-release.sh --tag v0.1.22      # pin a release (default: latest)
#   scripts/install-release.sh --restart-daemon   # restart the old thewayd afterwards
#   scripts/install-release.sh --dry-run          # print the assets and URLs only
#   scripts/install-release.sh --help
#
# Behavior:
#   - Resolve the latest release tag from the GitHub API (pin it with --tag).
#   - Select x86_64/aarch64 assets for Linux, macOS, or Windows.
#   - Download the theway, thewayd, and tgrep binaries and replace them
#     atomically in the target directory.
#   - Download SHA256SUMS and verify the three installed binaries when
#     sha256sum (Linux) or shasum (macOS) is available; --no-verify skips.
#   - Create the `tw` shorthand (same binary as theway).
#   - Leave a running thewayd alone by default; --restart-daemon switches it
#     immediately.
#
# Dependencies: bash, curl, uname, and sha256sum or shasum.
# This is the fast alternative to scripts/install.sh: it downloads GitHub
# Release artifacts instead of building with Cargo.
# =============================================================================

set -euo pipefail

REPO="ptlzc/theway"
BIN_DIR="${THEWAY_BIN_DIR:-${CARGO_HOME:-$HOME/.cargo}/bin}"
RESTART_DAEMON=0
DRY_RUN=0
SKIP_VERIFY=0
TAG="${THEWAY_RELEASE_TAG:-}"

usage() {
    sed -n '5,9p' "${BASH_SOURCE[0]}"
}

while [ $# -gt 0 ]; do
    case "$1" in
        --bin-dir)
            BIN_DIR="${2:?--bin-dir requires a directory argument}"
            shift 2
            ;;
        --bin-dir=*)
            BIN_DIR="${1#--bin-dir=}"
            shift
            ;;
        --tag)
            TAG="${2:?--tag requires a version argument}"
            shift 2
            ;;
        --tag=*)
            TAG="${1#--tag=}"
            shift
            ;;
        --restart-daemon)
            RESTART_DAEMON=1
            shift
            ;;
        --dry-run)
            DRY_RUN=1
            shift
            ;;
        --no-verify)
            SKIP_VERIFY=1
            shift
            ;;
        -h | --help)
            usage
            exit 0
            ;;
        *)
            echo "install-release.sh: unknown argument: $1" >&2
            usage >&2
            exit 2
            ;;
    esac
done

if [ -z "$TAG" ]; then
    echo "==> Resolving the latest release of $REPO"
    TAG="$(curl -fsSL --retry 3 --retry-delay 2 \
        "https://api.github.com/repos/$REPO/releases/latest" \
        | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -1)"
fi

case "$TAG" in
    v*) ;;
    *)
        echo "install-release.sh: invalid tag: $TAG (expected vX.Y.Z)" >&2
        exit 2
        ;;
esac

# ── Platform mapping ─────────────────────────────────────────────────────────
OS="$(uname -s)"
ARCH="$(uname -m)"
EXE=""
TARGET=""

case "$OS" in
    Linux)
        case "$ARCH" in
            x86_64 | amd64) TARGET="x86_64-unknown-linux-gnu" ;;
            aarch64 | arm64) TARGET="aarch64-unknown-linux-gnu" ;;
        esac
        ;;
    Darwin)
        case "$ARCH" in
            x86_64 | amd64) TARGET="x86_64-apple-darwin" ;;
            aarch64 | arm64) TARGET="aarch64-apple-darwin" ;;
        esac
        ;;
    MINGW* | MSYS* | CYGWIN*)
        EXE=".exe"
        case "$ARCH" in
            x86_64 | amd64) TARGET="x86_64-pc-windows-msvc" ;;
            aarch64 | arm64) TARGET="aarch64-pc-windows-msvc" ;;
        esac
        ;;
esac

if [ -z "$TARGET" ]; then
    echo "install-release.sh: unsupported platform: $OS/$ARCH" >&2
    echo "Supported platforms: Linux, macOS, and Windows on x86_64 or aarch64." >&2
    exit 2
fi

BASE_URL="https://github.com/$REPO/releases/download/$TAG"
ASSETS=(
    "theway-$TAG-$TARGET$EXE"
    "thewayd-$TAG-$TARGET$EXE"
    "tgrep-$TAG-$TARGET$EXE"
)

if [ "$DRY_RUN" -eq 1 ]; then
    echo "tag:     $TAG"
    echo "target:  $TARGET"
    echo "bin dir: $BIN_DIR"
    for asset in "${ASSETS[@]}"; do
        echo "$BASE_URL/$asset"
    done
    exit 0
fi

mkdir -p "$BIN_DIR"
TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/theway-release.XXXXXX")"
trap 'rm -rf "$TMP_DIR"' EXIT

echo "==> Downloading prebuilt $TAG binaries for $TARGET"

download_asset() {
    local asset="$1"
    curl -fL --retry 3 --retry-delay 2 \
        -o "$TMP_DIR/$asset" \
        "$BASE_URL/$asset"
    chmod +x "$TMP_DIR/$asset"
}

for asset in "${ASSETS[@]}"; do
    download_asset "$asset"
done

# ── SHA256 verification ──────────────────────────────────────────────────────
verify_assets() {
    if [ "$SKIP_VERIFY" -eq 1 ]; then
        echo "==> Skipping SHA256 verification"
        return 0
    fi
    local sum_cmd=""
    if command -v sha256sum >/dev/null 2>&1; then
        sum_cmd="sha256sum"
    elif command -v shasum >/dev/null 2>&1; then
        sum_cmd="shasum -a 256"
    fi
    if [ -z "$sum_cmd" ]; then
        echo "==> sha256sum/shasum not found; skipping SHA256 verification" >&2
        return 0
    fi
    curl -fsSL --retry 3 --retry-delay 2 \
        -o "$TMP_DIR/SHA256SUMS" \
        "$BASE_URL/SHA256SUMS"
    for asset in "${ASSETS[@]}"; do
        local expected
        expected="$(awk -v name="$asset" '$2 == name {print $1; exit}' "$TMP_DIR/SHA256SUMS")"
        if [ -z "$expected" ]; then
            echo "install-release.sh: SHA256SUMS is missing $asset" >&2
            return 1
        fi
        local actual
        actual="$($sum_cmd "$TMP_DIR/$asset" | awk '{print $1}')"
        if [ "$expected" != "$actual" ]; then
            echo "install-release.sh: SHA256 verification failed for $asset" >&2
            echo "  expected: $expected" >&2
            echo "  actual:   $actual" >&2
            return 1
        fi
    done
    echo "==> SHA256 verification passed"
}

verify_assets

# ── Atomic install ───────────────────────────────────────────────────────────
install_asset() {
    local product="$1"
    local src="$TMP_DIR/$product-$TAG-$TARGET$EXE"
    local dst="$BIN_DIR/$product$EXE"
    local tmp_dst="$BIN_DIR/.$product.tmp.$$"
    cp "$src" "$tmp_dst"
    chmod +x "$tmp_dst"
    mv -f "$tmp_dst" "$dst"
}

echo "==> Installing into $BIN_DIR"
install_asset theway
install_asset thewayd
install_asset tgrep

# ── tw shorthand ─────────────────────────────────────────────────────────────
tmp_tw="$BIN_DIR/.tw.tmp.$$"
cp "$BIN_DIR/theway$EXE" "$tmp_tw"
mv -f "$tmp_tw" "$BIN_DIR/tw$EXE"

# ── Running daemon handling (same policy as scripts/install.sh) ─────────────
THEWAY_BASE="${THEWAY_DIR:-$HOME/.theway}"
if [ "$RESTART_DAEMON" -eq 1 ]; then
    echo "==> Restarting the old thewayd process (other terminal sessions will disconnect)"
    pkill -TERM -x thewayd 2>/dev/null || true
    for _ in 1 2 3 4 5; do
        pgrep -x thewayd >/dev/null 2>&1 || break
        sleep 1
    done
    pkill -KILL -x thewayd 2>/dev/null || true
    rm -f "$THEWAY_BASE"/daemon-port "$THEWAY_BASE"/daemon-port-*
else
    for f in "$THEWAY_BASE"/daemon-port-*; do
        [ -e "$f" ] || continue
        pid="$(awk '{print $2}' "$f" 2>/dev/null || true)"
        if [ -n "$pid" ] && ! ps -p "$pid" -o comm= 2>/dev/null | grep -qx thewayd; then
            rm -f "$f"
        fi
    done
    if pgrep -x thewayd >/dev/null 2>&1; then
        echo "==> A running thewayd is still serving existing sessions and is unaffected:"
        for pid in $(pgrep -x thewayd); do
            echo "     pid $pid — it exits a few seconds after its TUI closes; the next start uses the new binary"
        done
    fi
fi

echo "==> Done:"
"$BIN_DIR/theway$EXE" --version

case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *)
        echo "Note: $BIN_DIR is not on PATH; add it to your shell profile, for example:" >&2
        echo "  export PATH=\"$BIN_DIR:\$PATH\"" >&2
        ;;
esac
