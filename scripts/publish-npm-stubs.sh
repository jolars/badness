#!/usr/bin/env bash
# One-time bootstrap: publish minimal *stub* versions of all npm packages so that
# they exist on the registry. This is required because npm has no pending/
# pre-registration for Trusted Publishing — a trusted publisher can only be
# attached to a package that already exists. After running this, configure
# Trusted Publishing on each package (repo jolars/badness, workflow
# publish-npm.yml, environment release), and let CI publish the first *real*,
# functional release (with binaries) at a HIGHER version via OIDC.
#
# The stubs carry no binary and a throwaway version (default 0.0.0) that sits
# below the first real release, so CI can later publish the genuine packages.
#
# Prereqs:
#   - The @badness org/scope exists (npmjs.com/org/create).
#   - You are logged in to npm. On NixOS where ~/.npmrc is a read-only
#     home-manager symlink, point npm at a writable config first:
#       set -x NPM_CONFIG_USERCONFIG /tmp/badness-npmrc
#       cp ~/.npmrc /tmp/badness-npmrc; chmod 600 /tmp/badness-npmrc
#       npm login
#
# Usage:
#   bash scripts/publish-npm-stubs.sh            # publish stubs
#   STUB_VERSION=0.0.0 bash scripts/publish-npm-stubs.sh
#   DRY_RUN=1 bash scripts/publish-npm-stubs.sh  # assemble + npm --dry-run only
set -euo pipefail

STUB_VERSION="${STUB_VERSION:-0.0.0}"
PUBLISH_ARGS=(--access public)
[[ "${DRY_RUN:-0}" == "1" ]] && PUBLISH_ARGS+=(--dry-run)

REPO_URL="git+https://github.com/jolars/badness.git"
PLATFORMS=(
  linux-x64-gnu
  linux-arm64-gnu
  linux-x64-musl
  linux-arm64-musl
  darwin-x64
  darwin-arm64
  win32-x64
  win32-arm64
)

stage_root="$(mktemp -d)"
trap 'rm -rf "$stage_root"' EXIT

publish_stub() {
  local name="$1" desc="$2"
  local dir="$stage_root/${name//\//_}"
  mkdir -p "$dir"
  cat >"$dir/package.json" <<JSON
{
  "name": "$name",
  "version": "$STUB_VERSION",
  "description": "$desc",
  "license": "MIT",
  "repository": { "type": "git", "url": "$REPO_URL" },
  "homepage": "https://jolars.github.io/badness/"
}
JSON
  echo ">>> publishing $name@$STUB_VERSION"
  ( cd "$dir" && npm publish "${PUBLISH_ARGS[@]}" )
}

# The main `badness` package is published separately (it carries the launcher,
# not a per-platform binary). Only the per-platform binary packages are stubbed
# here. Set INCLUDE_MAIN=1 if `badness` does not yet exist on the registry.
if [[ "${INCLUDE_MAIN:-0}" == "1" ]]; then
  publish_stub "badness" \
    "A language server, formatter, and linter for LaTeX (placeholder release; functional build ships via CI)."
fi

# Per-platform binary packages.
for p in "${PLATFORMS[@]}"; do
  publish_stub "@badness/$p" \
    "Prebuilt badness binary for $p (placeholder). Install \`badness\` instead."
done

echo
echo "Done. Next:"
echo "  1. On npmjs.com, add Trusted Publishing to each of the 9 packages:"
echo "       repo=jolars/badness  workflow=publish-npm.yml  environment=release"
echo "  2. Cut a tagged release (> $STUB_VERSION); CI publishes the real packages via OIDC."
