#!/usr/bin/env bash
# =============================================================================
# wt — theway GitHub 工作流助手 (issue / worktree / push / PR / merge)
#
# 面向本仓库 (theway) 的 issue-first + worktree 协作模式:
#   - 创建 GitHub issue
#   - 为 issue 建独立 worktree (仓库外兄弟目录, 不污染主工作树)
#   - 推送分支 / 直推 main
#   - 创建 / 合并 Pull Request
#
# 依赖: bash (Git Bash/MSYS), git, gh (GitHub CLI, 需已 gh auth login), python3
#
# 用法: wt <子命令> [参数]
#   wt issue <标题> [--labels a,b] [--desc "…"]
#   wt start <标题> [--labels a,b] [--desc "…"]      # issue + worktree + 分支
#   wt wt <issue-id> [--name <slug>]                 # 为已有 issue 建 worktree
#   wt push <issue-id> [--main]                      # 推送分支; --main 直推 main
#   wt mr <issue-id> [--title "…"] [--target <分支>]
#   wt merge <issue-id> [--squash] [--rm-wt]         # 合并 PR (自动删 worktree)
#   wt status                                        # worktrees + open PRs
#   wt close <issue-id>
#   wt cleanup <issue-id>                            # 删已合并 worktree + 分支
# =============================================================================
set -euo pipefail

# ── 配置 (环境变量可覆盖) ──────────────────────────────────────────────────
WT_PARENT="${WT_PARENT:-}"            # worktree 父目录; 默认主工作树上级
WT_PREFIX="${WT_PREFIX:-theway}"      # worktree 目录名前缀

# ── 工具函数 ────────────────────────────────────────────────────────────────
die() { printf '✗ %s\n' "$*" >&2; exit 1; }
info() { printf '✓ %s\n' "$*"; }
warn() { printf '⚠ %s\n' "$*" >&2; }

# gh 可用性 + 认证检查
gh_check() {
  command -v gh >/dev/null 2>&1 || die "未找到 gh (GitHub CLI) — 安装后执行 gh auth login"
  gh auth status >/dev/null 2>&1 || die "gh 未认证 — 先 gh auth login"
}

repo_root() {
  git rev-parse --show-toplevel 2>/dev/null || die "不在 git 仓库内"
}

# gh 输出 (JSON) 解析: gh_json <json> <python-expr>
gh_json() { python -c "import sys,json; print($2)" <<< "$1" 2>/dev/null || true; }

# issue 标题 → 分支 slug (ascii 安全, 中文 fallback 为空)
title_slug() {
  printf '%s' "$1" | tr '[:upper:]' '[:lower:]' \
    | sed -E 's/[^a-z0-9]+/-/g; s/^-+|-+$//g' | cut -c1-40
}

# 由 issue id 取 title (不存在时报错)
issue_title() {
  gh issue view "$1" --json title --jq .title 2>/dev/null \
    || die "issue #$1 不存在"
}

# worktree 目录: 默认 <主工作树上级>/<WT_PREFIX>-<issue-id>
wt_dir() {
  if [ -n "$WT_PARENT" ]; then printf '%s/%s-%s' "$WT_PARENT" "$WT_PREFIX" "$1"; return; fi
  local root
  root="$(repo_root)"
  printf '%s/../%s-%s' "$root" "$WT_PREFIX" "$1"
}

# 分支名: feat/issue-<id>-<slug>
branch_name() {
  local id="$1" slug="${2:-}"
  if [ -n "$slug" ]; then printf 'feat/issue-%s-%s' "$id" "$slug"; else printf 'feat/issue-%s' "$id"; fi
}

# ── 子命令 ──────────────────────────────────────────────────────────────────
cmd_issue() {
  local title="" labels="" desc=""
  title="${1:?用法: wt issue <标题> [--labels a,b] [--desc …]}"
  shift
  while [ $# -gt 0 ]; do
    case "$1" in
      --labels) labels="${2:?}"; shift 2 ;;
      --desc)   desc="${2:?}"; shift 2 ;;
      *) die "未知参数: $1 (见 wt issue --help)" ;;
    esac
  done
  [ -n "$desc" ] || desc="由 wt 脚本创建 — 需求详见对话/commit。"
  local args=(--title "$title" --body "$desc")
  [ -n "$labels" ] && while IFS=',' read -r l; do
    l="${l//[[:space:]]/}"
    [ -n "$l" ] && args+=(--label "$l")
  done <<< "$labels"
  local out
  out="$(gh issue create "${args[@]}" --json number,url)"
  local iid url
  iid="$(gh_json "$out" 'json.load(sys.stdin).get("number","")')"
  url="$(gh_json "$out" 'json.load(sys.stdin).get("url","")')"
  [ -n "$iid" ] || die "创建 issue 失败: $(printf '%s' "$out" | head -c 300)"
  LAST_IID="$iid"
  info "issue #${iid} 已创建: ${url}"
}

cmd_wt() {
  local id="${1:?用法: wt wt <issue-id> [--name <slug>] [--dir <path>]}"
  shift
  local name="" dir=""
  while [ $# -gt 0 ]; do
    case "$1" in
      --name) name="${2:?}"; shift 2 ;;
      --dir)  dir="${2:?}"; shift 2 ;;
      *) die "未知参数: $1" ;;
    esac
  done
  local title
  title="$(issue_title "$id")"
  [ -n "$name" ] || name="$(title_slug "$title")"
  local branch dirpath
  branch="$(branch_name "$id" "$name")"
  dirpath="${dir:-$(wt_dir "$id")}"
  # 分支已存在 → 找它的 worktree (porcelain 绝对路径), 已挂载则复用, 否则报错
  if git -C "$(repo_root)" rev-parse --verify --quiet "$branch" >/dev/null; then
    local wtpath
    wtpath="$(git -C "$(repo_root)" worktree list --porcelain \
      | grep -F -B2 "branch refs/heads/${branch}" | grep '^worktree ' | cut -d' ' -f2- || true)"
    if [ -n "$wtpath" ]; then
      info "worktree 已存在: ${wtpath} (分支 ${branch})"
    else
      die "分支 ${branch} 已存在但无对应 worktree — 用 --dir 指定其他路径或手动处理"
    fi
  else
    git -C "$(repo_root)" worktree add -b "$branch" "$dirpath" origin/main >/dev/null 2>&1 \
      || die "创建 worktree 失败 (分支 ${branch} 可能已存在? 用 --dir 指定其他路径)"
    info "worktree: ${dirpath}"
    info "分支: ${branch} (基于 origin/main)"
  fi
  info "进入: cd ${dirpath}"
}

cmd_start() {
  local title="${1:?用法: wt start <标题> [--labels …] [--desc …]}"
  shift
  local labels="" desc=""
  while [ $# -gt 0 ]; do
    case "$1" in
      --labels) labels="${2:?}"; shift 2 ;;
      --desc)   desc="${2:?}"; shift 2 ;;
      *) die "未知参数: $1" ;;
    esac
  done
  cmd_issue "$title" ${labels:+--labels "$labels"} ${desc:+--desc "$desc"}
  local iid
  iid="$LAST_IID"
  [ -n "$iid" ] || die "未能取得新建 issue 编号"
  cmd_wt "$iid"
}

cmd_push() {
  local id="${1:?用法: wt push <issue-id> [--main]}"
  shift
  local to_main=0
  while [ $# -gt 0 ]; do
    case "$1" in
      --main) to_main=1; shift ;;
      *) die "未知参数: $1" ;;
    esac
  done
  local dir branch
  dir="$(wt_dir "$id")"
  [ -d "$dir" ] || die "worktree 不存在: ${dir} (先 wt wt $id)"
  branch="$(git -C "$dir" branch --show-current)"
  if [ "$to_main" -eq 1 ]; then
    info "直推 main: ${branch} → main"
    git -C "$dir" push origin "HEAD:main"
  else
    info "推送分支 ${branch} → origin"
    git -C "$dir" push -u origin "$branch"
  fi
}

cmd_mr() {
  local id="${1:?用法: wt mr <issue-id> [--title …] [--target <分支>]}"
  shift
  local title="" target="main"
  while [ $# -gt 0 ]; do
    case "$1" in
      --title)  title="${2:?}"; shift 2 ;;
      --target) target="${2:?}"; shift 2 ;;
      *) die "未知参数: $1" ;;
    esac
  done
  local dir branch issue_title_txt
  dir="$(wt_dir "$id")"
  [ -d "$dir" ] || die "worktree 不存在: ${dir}"
  branch="$(git -C "$dir" branch --show-current)"
  issue_title_txt="$(issue_title "$id")"
  [ -n "$title" ] || title="Resolves #${id}: ${issue_title_txt}"

  # 同源分支已有 open PR 时直接提示, 不重复创建
  local existing
  existing="$(gh pr list --head "$branch" --state open --json number --jq '.[0].number // ""' 2>/dev/null || true)"
  if [ -n "$existing" ]; then
    local eurl
    eurl="$(gh pr view "$existing" --json url --jq .url)"
    info "PR #${existing} 已存在: ${eurl}"
    return
  fi

  # 确保分支已推送
  git -C "$dir" ls-remote --exit-code origin "$branch" >/dev/null 2>&1 \
    || git -C "$dir" push -u origin "$branch" 2>&1 | grep -vE "WARNING|vulnerable|openssh|Authorized|monitored" || true
  local out pnum url
  out="$(gh pr create --base "$target" --head "$branch" \
    --title "$title" --body "Closes #${id}" --json number,url)"
  pnum="$(gh_json "$out" 'json.load(sys.stdin).get("number","")')"
  url="$(gh_json "$out" 'json.load(sys.stdin).get("url","")')"
  [ -n "$pnum" ] || die "创建 PR 失败: $(printf '%s' "$out" | head -c 300)"
  info "PR #${pnum} 已创建: ${url}"
}

cmd_merge() {
  local id="${1:?用法: wt merge <issue-id> [--squash] [--rm-wt]}"
  shift
  local squash=0 rm_wt=0
  while [ $# -gt 0 ]; do
    case "$1" in
      --squash) squash=1; shift ;;
      --rm-wt)  rm_wt=1; shift ;;
      *) die "未知参数: $1" ;;
    esac
  done
  local dir branch pr
  dir="$(wt_dir "$id")"
  branch="$(git -C "$dir" branch --show-current 2>/dev/null || true)"
  [ -n "$branch" ] || branch="$(git branch -a | grep -oE "feat/issue-${id}-[a-z0-9-]+" | head -1 || true)"
  [ -n "$branch" ] || die "找不到 issue #${id} 的分支"
  pr="$(gh pr list --head "$branch" --state open --json number --jq '.[0].number // ""' 2>/dev/null || true)"
  [ -n "$pr" ] || die "找不到 ${branch} 的 open PR (先 wt mr $id)"
  if [ "$squash" -eq 1 ]; then
    gh pr merge "$pr" --squash --delete-branch >/dev/null
  else
    gh pr merge "$pr" --merge --delete-branch >/dev/null
  fi
  local state base
  state="$(gh pr view "$pr" --json state --jq .state)"
  base="$(gh pr view "$pr" --json baseRefName --jq .baseRefName)"
  [ "$state" = "MERGED" ] || die "合并失败: PR #${pr} 当前状态 ${state}"
  info "PR #${pr} (${branch}) 已合并 → ${base}"
  if [ "$rm_wt" -eq 1 ]; then cmd_cleanup "$id"; fi
}

cmd_status() {
  echo "── worktrees ──"
  git -C "$(repo_root)" worktree list
  echo
  echo "── open PRs ──"
  local prs
  prs="$(gh pr list --state open --limit 20 --json number,headRefName,baseRefName,title)"
  printf '%s' "$prs" | python -c '
import sys,json
d=json.load(sys.stdin)
if not d: print("(无)")
for p in d:
    print("  #%s %s → %s [%s]" % (p["number"], p["headRefName"], p["baseRefName"], p["title"][:60]))
'
}

cmd_close() {
  local id="${1:?用法: wt close <issue-id>}"
  gh issue close "$id" >/dev/null
  info "issue #${id} 已关闭"
}

cmd_cleanup() {
  local id="${1:?用法: wt cleanup <issue-id>}"
  local dir branch
  dir="$(wt_dir "$id")"
  branch="$(git -C "$dir" branch --show-current 2>/dev/null || true)"
  if [ -d "$dir" ]; then
    # 仅当分支已合并到 main (或远端已删除) 才移除, 否则警告
    git -C "$(repo_root)" fetch origin --prune >/dev/null 2>&1 || true
    local merged
    merged="$(git -C "$(repo_root)" branch -r --merged origin/main 2>/dev/null | grep -c "${branch}" || true)"
    if [ "$merged" -eq 0 ]; then
      warn "分支 ${branch} 未合并到 origin/main — 仅删除本地 worktree, 分支保留"
      git -C "$(repo_root)" worktree remove --force "$dir"
      info "worktree ${dir} 已移除 (分支 ${branch} 保留)"
    else
      git -C "$(repo_root)" worktree remove "$dir"
      git -C "$(repo_root)" branch -D "$branch" >/dev/null 2>&1 || true
      info "worktree ${dir} 与分支 ${branch} 已清理"
    fi
  else
    warn "worktree 不存在: ${dir}"
  fi
}

usage() {
  sed -n '2,24p' "$0" | sed 's/^# \{0,1\}//'
  exit "${1:-0}"
}

# ── 入口 ────────────────────────────────────────────────────────────────────
LAST_IID=""
gh_check

case "${1:-}" in
  ""|-h|--help|help) usage 0 ;;
  issue)   shift; cmd_issue "$@" ;;
  start)   shift; cmd_start "$@" ;;
  wt)      shift; cmd_wt "$@" ;;
  push)    shift; cmd_push "$@" ;;
  mr)      shift; cmd_mr "$@" ;;
  merge)   shift; cmd_merge "$@" ;;
  status)  cmd_status ;;
  close)   shift; cmd_close "$@" ;;
  cleanup) shift; cmd_cleanup "$@" ;;
  *) die "未知子命令: ${1:-} (wt help 查看用法)" ;;
esac
