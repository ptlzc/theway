#!/usr/bin/env bash
# =============================================================================
# npm-publish — publish both npm SDK packages for one aligned release
# version. Called by .github/workflows/release.yml after release-validate.sh
# confirms the package versions match the daemon/workspace version.
#
# Usage (inside the release workflow, with npm registry credentials):
#   scripts/npm-publish.sh <version>
#
# Idempotent: a package version already present on the official npm registry
# is skipped, so a failed workflow run can be re-run after partial upload.
# =============================================================================
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

VERSION="${1:-}"
if [ -z "$VERSION" ]; then
  echo "usage: scripts/npm-publish.sh <version>" >&2
  exit 2
fi

REGISTRY="https://registry.npmjs.org/"
PACKAGE_DIRS=(sdks/client sdks/plugin)

for dir in "${PACKAGE_DIRS[@]}"; do
  name="$(node -p "require('./${dir}/package.json').name")"
  actual="$(node -p "require('./${dir}/package.json').version")"
  if [ "$actual" != "$VERSION" ]; then
    echo "error: ${name} version ${actual} does not match release ${VERSION}" >&2
    exit 1
  fi

  if npm view "${name}@${VERSION}" version --registry "$REGISTRY" >/dev/null 2>&1; then
    echo "[npm-publish] skip ${name}@${VERSION} (already on npm)"
    continue
  fi

  echo "[npm-publish] build ${name}@${VERSION}"
  npm ci --prefix "$dir" --no-audit --no-fund

  # Client SDK regenerates from checked-in protos; the diff check rejects
  # generated drift. Plugin SDK runs build + typecheck + tests.
  npm --prefix "$dir" run prepublishOnly
  git diff --exit-code -- "$dir"

  echo "[npm-publish] inspect ${name}@${VERSION}"
  (cd "$dir" && npm pack --dry-run --json)

  echo "[npm-publish] publish ${name}@${VERSION}"
  (cd "$dir" && npm publish --access public --tag latest)
done

echo "[npm-publish] done: ${PACKAGE_DIRS[*]} at ${VERSION}"
