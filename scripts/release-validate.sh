#!/usr/bin/env bash
# =============================================================================
# release-validate — enforce one tag = one version across every release
# artifact: Cargo workspace (daemon + runtime crates), @theway-ai/sdk, and
# @theway-ai/plugin-sdk must all carry the same version as the `v*` tag.
#
# Usage:
#   scripts/release-validate.sh <tag>   # e.g. v0.1.9
#
# Prints the validated version on stdout. The release workflow uses that
# value as the single version for crates.io and npm publishing.
# =============================================================================
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

TAG="${1:-}"
if [ -z "$TAG" ]; then
  echo "usage: scripts/release-validate.sh <tag>" >&2
  exit 2
fi

WORKSPACE_VERSION="$(awk -F '"' '/^\[workspace.package\]/{found=1; next} found && /^version = /{print $2; exit}' Cargo.toml)"
if [ -z "$WORKSPACE_VERSION" ]; then
  echo "error: workspace version not found in Cargo.toml" >&2
  exit 1
fi

if [ "$TAG" != "v$WORKSPACE_VERSION" ]; then
  echo "error: tag $TAG does not match workspace version $WORKSPACE_VERSION" >&2
  exit 1
fi

# Runtime crates in the release allowlist (including the protocol crates
# theway-transport and theway-contract) must all resolve to the workspace
# version. `version.workspace = true` is the normal form; an explicit equal
# version is accepted but should not be used.
mapfile -t RELEASE_CRATES < <(grep -vE '^\s*(#|$)' scripts/release-crates.txt)
if [ "${#RELEASE_CRATES[@]}" -eq 0 ]; then
  echo "error: scripts/release-crates.txt is empty" >&2
  exit 1
fi

crate_version_decl() {
  awk '/^\[package\]/{in_pkg=1; next} in_pkg && /^\[/{exit} in_pkg && /^version/{print; exit}' "$1"
}

for crate in "${RELEASE_CRATES[@]}"; do
  manifest="crates/${crate}/Cargo.toml"
  if [ ! -f "$manifest" ]; then
    echo "error: release crate manifest missing: $manifest" >&2
    exit 1
  fi
  decl="$(crate_version_decl "$manifest")"
  case "$decl" in
    *version.workspace*true*)
      ;;
    *)
      declared="$(printf '%s' "$decl" | sed -nE 's/^version[ \t]*=[ \t]*"([^"]+)".*/\1/p')"
      if [ "$declared" != "$WORKSPACE_VERSION" ]; then
        echo "error: ${crate} version must equal workspace ${WORKSPACE_VERSION}, got: ${decl}" >&2
        exit 1
      fi
      ;;
  esac
done
echo "aligned ${#RELEASE_CRATES[@]} runtime crates at ${WORKSPACE_VERSION}" >&2

node - "$WORKSPACE_VERSION" <<'NODE'
const fs = require('fs');
const expected = process.argv[2];

for (const dir of ['sdks/client', 'sdks/plugin']) {
  const pkg = JSON.parse(fs.readFileSync(`${dir}/package.json`, 'utf8'));
  const lock = JSON.parse(fs.readFileSync(`${dir}/package-lock.json`, 'utf8'));
  const problems = [
    [pkg.name, pkg.version],
    [pkg.name, lock.version],
    [pkg.name, lock.packages?.['']?.version],
  ].filter(([, version]) => version !== expected);
  if (problems.length > 0) {
    console.error(
      `error: ${pkg.name} versions must equal daemon/workspace ${expected}: ` +
        problems.map(([name, version]) => `${name}=${version}`).join(', ')
    );
    process.exit(1);
  }
}
NODE

echo "$WORKSPACE_VERSION"
