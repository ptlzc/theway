#!/usr/bin/env bash
# =============================================================================
# crates-publish — publish the release allowlist to crates.io in dependency
# order for one workspace version. Called by .github/workflows/release.yml
# after release-validate.sh confirms the `v*` tag matches the workspace.
#
# Usage:
#   CARGO_REGISTRY_TOKEN=... scripts/crates-publish.sh <version>
#
# The allowlist is the runtime crates that inherit the workspace version,
# excluding theway-probe (repository-local validation binary; repository
# policy forbids uploading it). Vendored/ported crates keep independent
# versions and are released separately.
#
# Idempotent: a version already present on crates.io is skipped, so a failed
# workflow run can be re-run after the partial upload is inspected.
# =============================================================================
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

VERSION="${1:-}"
if [ -z "$VERSION" ]; then
  echo "usage: CARGO_REGISTRY_TOKEN=... scripts/crates-publish.sh <version>" >&2
  exit 2
fi
if [ -z "${CARGO_REGISTRY_TOKEN:-}" ]; then
  echo "error: CARGO_REGISTRY_TOKEN is not set" >&2
  exit 1
fi

# Dependency-ordered allowlist: dev-macro first (runtime crates depend on it
# as a dev-dependency), then leaves, then dependents; daemon last.
CRATES=(
  tests-bridge-macro
  theway-contract
  theway-mcp
  theway-llm-provider
  theway-storage
  theway-transport
  theway-core
  theway-tui
  theway-daemon
)

crate_version_on_registry() {
  local crate="$1"
  local code
  code="$(curl -sS -o /dev/null -w '%{http_code}' \
    -A "theway-release/1.0 (github.com/ptlzc/theway)" \
    "https://crates.io/api/v1/crates/${crate}/${VERSION}")"
  case "$code" in
    200) return 0 ;;
    404) return 1 ;;
    *)
      echo "error: crates.io query for ${crate}@${VERSION} failed (HTTP ${code})" >&2
      exit 1
      ;;
  esac
}

for crate in "${CRATES[@]}"; do
  pkgid="$(cargo pkgid -p "$crate")"
  case "$pkgid" in
    *"#${VERSION}")
      ;;
    *)
      echo "error: ${crate} resolves to ${pkgid}, expected #${VERSION}" >&2
      exit 1
      ;;
  esac

  if crate_version_on_registry "$crate"; then
    echo "[crates-publish] skip ${crate}@${VERSION} (already on crates.io)"
    continue
  fi

  echo "[crates-publish] publish ${crate}@${VERSION}"
  cargo publish -p "$crate" --locked

  if ! crate_version_on_registry "$crate"; then
    echo "error: ${crate}@${VERSION} not visible on crates.io after publish" >&2
    exit 1
  fi
  echo "[crates-publish] published ${crate}@${VERSION}"
done

echo "[crates-publish] done: ${#CRATES[@]} crates at ${VERSION}"
