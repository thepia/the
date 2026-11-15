#!/usr/bin/env bash
# Publish the npm distribution for an already-built release from a dev machine (the manual npm "finish"
# step of the two-stage release). CI builds the per-arch binaries and publishes them to the CDN +
# GitHub Release (Stage 1); a human then runs THIS locally to publish the npm launcher + platform
# packages. Doing it locally is deliberate: `npm login` supplies the 2FA one-time password interactively,
# which a CI token cannot (the CI run EOTP'd), and it keeps the npm publish behind a human sign-off.
#
# The binaries come from the release's OWN Stage-1 tarballs (GitHub Release `v<version>`), so npm ships
# the exact same bytes as the CDN / `curl|sh` channel, never a fresh, possibly-different local build.
#
# Usage: npm-publish-release.sh [version] [--tag <dist-tag>] [--dry-run]
#   version     release to publish (default: the Cargo.toml workspace version)
#   --tag       npm dist-tag (default: latest; use `rc`/`next` to stage an RC, then promote with
#               `npm dist-tag add thepia@<version> latest`)
#   --dry-run   assemble + `npm publish --dry-run` only (no upload, no auth needed)
#
# Requires: gh (authenticated), node, npm (logged in for a real publish: `npm login`).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

VERSION=""
TAG="latest"
DRY_RUN=0
for a in "$@"; do
  case "$a" in
    --tag=*) TAG="${a#--tag=}" ;;
    --tag) ;;                       # value handled below
    --dry-run) DRY_RUN=1 ;;
    -*) echo "npm-publish-release: unknown flag $a" >&2; exit 2 ;;
    *) if [ -z "$VERSION" ]; then VERSION="$a"; fi ;;
  esac
done
# handle `--tag <value>` (space form)
prev=""
for a in "$@"; do [ "$prev" = "--tag" ] && TAG="$a"; prev="$a"; done

if [ -z "$VERSION" ]; then
  VERSION="$(grep -m1 -A2 '\[workspace.package\]' Cargo.toml | grep -m1 -E '^[[:space:]]*version' | sed -E 's/.*"([^"]+)".*/\1/')"
fi
TARGETS=(aarch64-apple-darwin x86_64-apple-darwin x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu)

echo ">> publishing thepia@${VERSION} to npm (tag=${TAG}, dry-run=${DRY_RUN})" >&2

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
mkdir -p "$WORK/dl" "$WORK/bin"

# Fetch the Stage-1 per-arch tarballs from the GitHub Release and extract the `thepia` binary from each.
for t in "${TARGETS[@]}"; do
  tb="thepia-v${VERSION}-${t}.tar.gz"
  echo ">> fetching ${tb}" >&2
  gh release download "v${VERSION}" --pattern "$tb" --dir "$WORK/dl" --clobber
  mkdir -p "$WORK/bin/${t}"
  tar -xzf "$WORK/dl/${tb}" -C "$WORK/dl"
  cp "$WORK/dl/thepia-v${VERSION}-${t}/thepia" "$WORK/bin/${t}/thepia"
  chmod +x "$WORK/bin/${t}/thepia"
done

# Assemble launcher + platform packages at the release version (overrides Cargo.toml so any release can
# be published, not only the one currently checked out).
node scripts/build-npm.mjs assemble --from "$WORK/bin" --version "$VERSION"

publish() {
  local target="$1"
  # All packages are unscoped now, so force the default registry to npmjs.org (in case a local .npmrc
  # changed it). No scope override needed: unscoped names can't be misrouted by a `@thepia:registry`.
  local reg=(--registry https://registry.npmjs.org)
  if [ "$DRY_RUN" -eq 1 ]; then
    npm publish --dry-run --tag "$TAG" "${reg[@]}" "$target"
  else
    npm publish --tag "$TAG" "${reg[@]}" "$target"
  fi
}

if [ "$DRY_RUN" -eq 0 ]; then
  if ! npm whoami >/dev/null 2>&1; then
    echo "npm-publish-release: not logged in to npm. Run 'npm login' first (handles 2FA/OTP)." >&2
    exit 1
  fi
  echo ">> publishing as npm user: $(npm whoami)" >&2
fi

# Platform packages first so the launcher's optionalDependencies already resolve; `./` so npm treats
# each as a folder, not a GitHub `org/repo` shorthand.
for p in npm/platforms/*/; do
  echo ">> publish ${p}" >&2
  publish "./${p}"
done
echo ">> publish npm/thepia" >&2
publish "./npm/thepia"

echo ">> done: thepia@${VERSION} (tag ${TAG})${DRY_RUN:+ }$([ "$DRY_RUN" -eq 1 ] && echo '(dry-run)')" >&2
if [ "$DRY_RUN" -eq 0 ] && [ "$TAG" != "latest" ]; then
  echo ">> staged under '${TAG}'. Promote on sign-off: npm dist-tag add thepia@${VERSION} latest" >&2
fi
