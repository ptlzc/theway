#!/usr/bin/env bash
# =============================================================================
# install-release — install/update theway from prebuilt GitHub Release binaries
#
# 用法:
#   scripts/install-release.sh                    # 安装/更新到 ~/.cargo/bin
#   scripts/install-release.sh --bin-dir DIR      # 安装到 DIR
#   scripts/install-release.sh --tag v0.1.22      # 指定版本 (默认 latest release)
#   scripts/install-release.sh --restart-daemon   # 安装后立即重启旧 thewayd
#   scripts/install-release.sh --dry-run          # 只打印将下载的资产和 URL
#   scripts/install-release.sh --help
#
# 行为:
#   - 从 GitHub API 解析 latest release tag (可用 --tag 固定版本)
#   - 按当前 OS/arch 选择 x86_64/aarch64 + linux/darwin/windows 资产
#   - 下载 theway / thewayd / tgrep 三个原始二进制并原子替换安装
#   - 下载 SHA256SUMS 并对本次安装的三个文件做校验 (系统有 sha256sum 或
#     shasum 时; 可用 --no-verify 跳过)
#   - 生成 `tw` 简写 (与 theway 相同)
#   - 默认不打断正在运行的 thewayd; --restart-daemon 保留旧行为并立即切换
#
# 依赖: bash, curl, uname, 以及 sha256sum (Linux) 或 shasum (macOS) 之一。
# 这是 install.sh 的快速替代: 直接下载 GitHub Release 产物, 不做 cargo 编译。
# =============================================================================

set -euo pipefail

REPO="ptlzc/theway"
BIN_DIR="${THEWAY_BIN_DIR:-${CARGO_HOME:-$HOME/.cargo}/bin}"
RESTART_DAEMON=0
DRY_RUN=0
SKIP_VERIFY=0
TAG="${THEWAY_RELEASE_TAG:-}"

usage() {
    sed -n '4,9p' "${BASH_SOURCE[0]}"
}

while [ $# -gt 0 ]; do
    case "$1" in
        --bin-dir)
            BIN_DIR="${2:?--bin-dir 需要一个目录参数}"
            shift 2
            ;;
        --bin-dir=*)
            BIN_DIR="${1#--bin-dir=}"
            shift
            ;;
        --tag)
            TAG="${2:?--tag 需要一个版本参数}"
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
            echo "install-release.sh: 未知参数: $1" >&2
            usage >&2
            exit 2
            ;;
    esac
done

if [ -z "$TAG" ]; then
    echo "==> 查询 $REPO 最新 release"
    TAG="$(curl -fsSL --retry 3 --retry-delay 2 \
        "https://api.github.com/repos/$REPO/releases/latest" \
        | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -1)"
fi

case "$TAG" in
    v*) ;;
    *)
        echo "install-release.sh: 无效的 tag: $TAG (期望 vX.Y.Z)" >&2
        exit 2
        ;;
esac

# ── 平台映射 ─────────────────────────────────────────────────────────────────
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
    echo "install-release.sh: 不支持的平台: $OS/$ARCH" >&2
    echo "支持的平台: x86_64/aarch64 的 Linux、macOS、Windows。" >&2
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

echo "==> 下载 $TAG ($TARGET) 的预编译二进制"

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

# ── SHA256 校验 ──────────────────────────────────────────────────────────────
verify_assets() {
    if [ "$SKIP_VERIFY" -eq 1 ]; then
        echo "==> 跳过 SHA256 校验"
        return 0
    fi
    local sum_cmd=""
    if command -v sha256sum >/dev/null 2>&1; then
        sum_cmd="sha256sum"
    elif command -v shasum >/dev/null 2>&1; then
        sum_cmd="shasum -a 256"
    fi
    if [ -z "$sum_cmd" ]; then
        echo "==> 未找到 sha256sum/shasum, 跳过 SHA256 校验" >&2
        return 0
    fi
    curl -fsSL --retry 3 --retry-delay 2 \
        -o "$TMP_DIR/SHA256SUMS" \
        "$BASE_URL/SHA256SUMS"
    for asset in "${ASSETS[@]}"; do
        local expected
        expected="$(awk -v name="$asset" '$2 == name {print $1; exit}' "$TMP_DIR/SHA256SUMS")"
        if [ -z "$expected" ]; then
            echo "install-release.sh: SHA256SUMS 缺少 $asset" >&2
            return 1
        fi
        local actual
        actual="$($sum_cmd "$TMP_DIR/$asset" | awk '{print $1}')"
        if [ "$expected" != "$actual" ]; then
            echo "install-release.sh: SHA256 校验失败: $asset" >&2
            echo "  expected: $expected" >&2
            echo "  actual:   $actual" >&2
            return 1
        fi
    done
    echo "==> SHA256 校验通过"
}

verify_assets

# ── 原子替换安装 ────────────────────────────────────────────────────────────
install_asset() {
    local product="$1"
    local src="$TMP_DIR/$product-$TAG-$TARGET$EXE"
    local dst="$BIN_DIR/$product$EXE"
    local tmp_dst="$BIN_DIR/.$product.tmp.$$"
    cp "$src" "$tmp_dst"
    chmod +x "$tmp_dst"
    mv -f "$tmp_dst" "$dst"
}

echo "==> 安装到 $BIN_DIR"
install_asset theway
install_asset thewayd
install_asset tgrep

# ── tw 简写 ─────────────────────────────────────────────────────────────────
tmp_tw="$BIN_DIR/.tw.tmp.$$"
cp "$BIN_DIR/theway$EXE" "$tmp_tw"
mv -f "$tmp_tw" "$BIN_DIR/tw$EXE"

# ── 运行中的 daemon 处理 (与 install.sh 一致) ────────────────────────────────
THEWAY_BASE="${THEWAY_DIR:-$HOME/.theway}"
if [ "$RESTART_DAEMON" -eq 1 ]; then
    echo "==> 重启旧版 thewayd 进程 (其他终端的 theway 会话会断开)"
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
        echo "==> 检测到仍在运行的 thewayd (继续服务现有会话, 不受影响):"
        for pid in $(pgrep -x thewayd); do
            echo "     pid $pid — 关闭对应 TUI 后会在数秒内自动退出, 下次启动即用新二进制"
        done
    fi
fi

echo "==> 完成:"
"$BIN_DIR/theway$EXE" --version

case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *)
        echo "提示: $BIN_DIR 不在 PATH 中, 请加入 shell 配置, 例如:" >&2
        echo "  export PATH=\"$BIN_DIR:\$PATH\"" >&2
        ;;
esac
