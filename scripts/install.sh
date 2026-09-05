#!/usr/bin/env bash
# =============================================================================
# install — build the latest theway release and install it into a bin dir
#
# 用法:
#   scripts/install.sh                   # 默认安装到 $CARGO_HOME/bin (~/.cargo/bin)
#   scripts/install.sh --root DIR        # 安装到 DIR/bin (cargo install --root 语义)
#   scripts/install.sh --restart-daemon  # 安装后立即重启旧 thewayd (会断开现有会话)
#   scripts/install.sh --help
#
# 行为:
#   - cargo install --path crates/theway-tui --force 构建 release 并覆盖安装
#   - cargo install --path crates/theway-daemon --force 同步安装 thewayd
#     (TUI 按需 spawn daemon 时从 theway 同目录或 PATH 找 thewayd, 两者必须配套,
#     否则 discovery 协议错配会表现为冷启动 20s 超时)
#   - 默认不动正在运行的 thewayd: 它们继续服务现有会话, TUI 关闭后由 controller
#     存储看门狗在数秒内自动退出 (issue #136), 下次启动即用新二进制; 只清理
#     进程已不存在的残留端口文件。需要旧 daemon 立即切换新二进制时加
#     --restart-daemon (先 SIGTERM 优雅退出, 再 SIGKILL 兜底)
#   - 内置扩展包无需在此复制: theway-extensions crate 把官方插件嵌入 thewayd
#     二进制, daemon 启动时自举到 $THEWAY_DIR/extensions-managed/ (issue #91)
#   - 同时生成 `tw` 简写 (与 theway 相同的二进制副本, Makefile 同款约定)
#   - 安装后打印版本; 若目标 bin 目录不在 PATH 中会给出提示
#
# 依赖: bash, cargo (rustup 或系统安装均可)
# =============================================================================

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CARGO="${CARGO:-cargo}"
EXE="${EXE:-}" # Windows 下可设为 .exe (与 Makefile 的 EXE 变量同约定)
RESTART_DAEMON="${RESTART_DAEMON:-}"

usage() {
    sed -n '5,9p' "${BASH_SOURCE[0]}"
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
        --restart-daemon)
            RESTART_DAEMON=1
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
# --locked: cargo install 默认忽略 workspace Cargo.lock 重新解析依赖, 曾解析出
# oxc_transformer 0.75.1 + oxc-browserslist 2.3.1 的破坏性组合 (Version 第三字段
# u32→u16 不兼容); 锁定后与 workspace 构建同一依赖集.
"$CARGO" install --path "$ROOT/crates/theway-tui" --force --locked --root "$INSTALL_ROOT"

echo "==> 构建并安装 thewayd (release) 到 $BIN_DIR"
"$CARGO" install --path "$ROOT/crates/theway-daemon" --force --locked --root "$INSTALL_ROOT"

# ── 运行中的 daemon 处理 ───────────────────────────────────────────────────
# 默认不打断: 正在运行的 thewayd 继续服务现有会话 (Linux 上覆盖运行中二进制的
# 磁盘文件不影响已加载的进程映像), 关闭对应 TUI 后看门狗会在数秒内让它自动
# 退出并清理自己的端口文件, 下次启动即用新二进制。--restart-daemon 保留旧行为:
# 立即停掉所有 thewayd (其他终端的 theway 会话会断开)。
THEWAY_BASE="${THEWAY_DIR:-$HOME/.theway}"
if [ -n "$RESTART_DAEMON" ]; then
    echo "==> 重启旧版 thewayd 进程 (其他终端的 theway 会话会断开)"
    pkill -TERM -x thewayd 2>/dev/null || true
    for _ in 1 2 3 4 5; do
        pgrep -x thewayd >/dev/null 2>&1 || break
        sleep 1
    done
    pkill -KILL -x thewayd 2>/dev/null || true
    # 移除旧全局端口文件 + 残留 per-cwd 条目 (新 daemon 启动时会写自己的).
    rm -f "$THEWAY_BASE"/daemon-port "$THEWAY_BASE"/daemon-port-*
else
    # 清理死进程的残留端口文件: 只删 pid 已不存在或已不是 thewayd 的条目,
    # 活 daemon 的条目原样保留 (daemon 退出时会自己清理).
    for f in "$THEWAY_BASE"/daemon-port-*; do
        [ -e "$f" ] || continue
        pid=$(awk '{print $2}' "$f" 2>/dev/null || true)
        if [ -n "$pid" ] && ! ps -p "$pid" -o comm= 2>/dev/null | grep -qx thewayd; then
            rm -f "$f"
        fi
    done
    if pgrep -x thewayd >/dev/null 2>&1; then
        echo "==> 检测到仍在运行的 thewayd (继续服务现有会话, 不受影响):"
        for pid in $(pgrep -x thewayd); do
            cwd=""
            if [ -r "/proc/$pid/cmdline" ]; then
                cwd=$(tr '\0' ' ' < "/proc/$pid/cmdline" 2>/dev/null \
                    | sed -n 's/.*--cwd \([^ ]*\).*/\1/p')
            fi
            echo "     pid $pid${cwd:+ (cwd $cwd)} — 关闭对应 TUI 后会在数秒内自动退出, 下次启动即用新二进制"
        done
    fi
fi

echo "==> 生成 tw 简写"
# cp 原地覆盖正在执行的二进制会 ETXTBSY (tw 常被用作 TUI 启动入口, 运行中
# 的进程仍持有该 inode); 先拷到同目录临时文件再 mv (rename 原子替换), 旧
# inode 留给运行中的进程, 新启动的 tw 指向新二进制.
tmp_tw="$BIN_DIR/.tw.tmp.$$"
cp "$BIN_DIR/theway$EXE" "$tmp_tw"
mv -f "$tmp_tw" "$BIN_DIR/tw$EXE"

echo "==> 完成:"
"$BIN_DIR/theway$EXE" --version

case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *)
        echo "提示: $BIN_DIR 不在 PATH 中, 请加入 shell 配置, 例如:" >&2
        echo "  export PATH=\"$BIN_DIR:\$PATH\"" >&2
        ;;
esac
