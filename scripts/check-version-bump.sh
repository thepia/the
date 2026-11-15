#!/usr/bin/env bash
# check-version-bump.sh — fail unless the package version is strictly higher
# than the one on the base branch (default origin/main). Enforces the rule:
# "don't push/merge without a version bump; at minimum a patch."
#
# Reused in two places:
#   - the pre-push git hook (local, fast feedback before a push leaves)
#   - the PR CI workflow (authoritative gate on the pull request)
#
# Usage: check-version-bump.sh [base-ref]      e.g. check-version-bump.sh origin/main
#
# Safe by design: if the repo has no recognized manifest, or the manifest is
# new on this branch, it exits 0 (nothing to compare). Supports package.json
# (npm/pnpm) and Cargo.toml (cargo); extend as needed.
#
# Bootstrap (the-specific): the FIRST release of a version is legitimate even
# when the branch version equals main's, as long as that version was never
# released (no `v<base>` tag). The release workflow tags each release `v{version}`,
# so once a version ships, main carries a tagged version and the NEXT PR must
# exceed it — which is exactly the per-PR bump we want going forward.
set -euo pipefail

BASE_REF="${1:-origin/main}"

json_ver()      { jq -r '.version // empty' "$1" 2>/dev/null || true; }
json_ver_ref()  { git show "$BASE_REF:$1" 2>/dev/null | jq -r '.version // empty' 2>/dev/null || true; }
toml_ver()      { grep -m1 -E '^[[:space:]]*version[[:space:]]*=' "$1" 2>/dev/null | sed -E 's/.*=[[:space:]]*"([^"]+)".*/\1/' || true; }
toml_ver_ref()  { git show "$BASE_REF:$1" 2>/dev/null | grep -m1 -E '^[[:space:]]*version[[:space:]]*=' | sed -E 's/.*=[[:space:]]*"([^"]+)".*/\1/' || true; }

version_released() {
  # True if a release tag `v<version>` exists locally or on origin.
  local v="$1"
  git rev-parse -q --verify "refs/tags/v${v}" >/dev/null 2>&1 && return 0
  git ls-remote --exit-code --tags origin "refs/tags/v${v}" >/dev/null 2>&1 && return 0
  return 1
}

if [ -f package.json ]; then
  manifest=package.json; new=$(json_ver package.json); base=$(json_ver_ref package.json)
elif [ -f Cargo.toml ]; then
  manifest=Cargo.toml;  new=$(toml_ver Cargo.toml);  base=$(toml_ver_ref Cargo.toml)
else
  echo "version-check: no package.json or Cargo.toml — nothing to check."; exit 0
fi

[ -z "$new" ]  && { echo "version-check: could not read version from $manifest — skipping."; exit 0; }
[ -z "$base" ] && { echo "version-check: no $manifest on $BASE_REF (new package?) — OK."; exit 0; }

if [ "$new" = "$base" ] && ! version_released "$base"; then
  echo "version-check: $new not yet released (no tag v$base) — OK (bootstrap / first release)."; exit 0
fi

highest=$(printf '%s\n%s\n' "$base" "$new" | sort -V | tail -1)
if [ "$new" = "$base" ] || [ "$highest" != "$new" ]; then
  echo "version-check: $manifest version '$new' is NOT higher than $BASE_REF ('$base')." >&2
  echo "Bump the version (at least a patch) before pushing or merging." >&2
  exit 1
fi
echo "version-check: $new > $base ($BASE_REF) — OK."
