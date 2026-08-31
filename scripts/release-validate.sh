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
