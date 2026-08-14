#!/usr/bin/env bash
# =============================================================================
# install — build the latest theway release and install it into a bin dir
#
# 用法:
#   scripts/install.sh              # 默认安装到 $CARGO_HOME/bin (~/.cargo/bin)
#   scripts/install.sh --root DIR   # 安装到 DIR/bin (cargo install --root 语义)
#   scripts/install.sh --help
#
# 行为:
#   - cargo install --path crates/theway-tui --force 构建 release 并覆盖安装
#   - cargo install --path crates/theway-daemon --force 同步安装 thewayd
#     (TUI 按需 spawn daemon 时从 theway 同目录或 PATH 找 thewayd, 两者必须配套,
#     否则 discovery 协议错配会表现为冷启动 20s 超时)
#   - 同时生成 `tw` 简写 (与 theway 相同的二进制副本, Makefile 同款约定)
#   - 安装后打印版本; 若目标 bin 目录不在 PATH 中会给出提示
#
# 依赖: bash, cargo (rustup 或系统安装均可)
# =============================================================================

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CARGO="${CARGO:-cargo}"
EXE="${EXE:-}" # Windows 下可设为 .exe (与 Makefile 的 EXE 变量同约定)

usage() {
    sed -n '2,13p' "${BASH_SOURCE[0]}"
}

INSTALL_ROOT=""
while [ $# -gt 0 ]; do
    case "$1" in
        --root)
            INSTALL_ROOT="${2:?--root 需要一个目录参数}"
            shift 2
            ;;
        --root=*)
            INSTALL_ROOT="${1#--root=}"
            shift
            ;;
        -h | --help)
            usage
            exit 0
            ;;
        *)
            echo "install.sh: 未知参数: $1" >&2
            usage >&2
            exit 2
            ;;
    esac
done

if [ -z "$INSTALL_ROOT" ]; then
    # cargo 默认前缀: 优先 $CARGO_HOME, 回退 ~/.cargo
    INSTALL_ROOT="${CARGO_HOME:-$HOME/.cargo}"
fi

BIN_DIR="$INSTALL_ROOT/bin"

echo "==> 构建并安装 theway (release) 到 $BIN_DIR"
mkdir -p "$BIN_DIR"
"$CARGO" install --path "$ROOT/crates/theway-tui" --force --root "$INSTALL_ROOT"

echo "==> 构建并安装 thewayd (release) 到 $BIN_DIR"
"$CARGO" install --path "$ROOT/crates/theway-daemon" --force --root "$INSTALL_ROOT"

echo "==> 生成 tw 简写"
cp "$BIN_DIR/theway$EXE" "$BIN_DIR/tw$EXE"

echo "==> 完成:"
"$BIN_DIR/theway$EXE" --version

case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *)
        echo "提示: $BIN_DIR 不在 PATH 中, 请加入 shell 配置, 例如:" >&2
        echo "  export PATH=\"$BIN_DIR:\$PATH\"" >&2
        ;;
esac
