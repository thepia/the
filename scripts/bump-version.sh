#!/usr/bin/env bash
# Bump the workspace version and the internal `the-*` path-dependency pins in lockstep.
#
# Every crate inherits the workspace version (`version.workspace = true`), and the internal path deps
# pin that same version (`the-output = { path = "../the-output", version = "X" }`), so a bump must move
# both or cargo's path-dep requirement no longer matches the published version.
#
# Usage: scripts/bump-version.sh patch|minor|major
set -euo pipefail
cd "$(dirname "$0")/.."

level="${1:-}"
case "$level" in
  patch | minor | major) ;;
  *)
    echo "usage: $0 patch|minor|major" >&2
    exit 2
    ;;
esac

old="$(grep -m1 '^version = "' Cargo.toml | sed -E 's/^version = "([^"]+)".*/\1/')"
if [ -z "$old" ]; then
  echo "could not read the workspace version from Cargo.toml" >&2
  exit 1
fi
major="${old%%.*}"
rest="${old#*.}"
minor="${rest%%.*}"
patch="${rest##*.}"

case "$level" in
  major)
    major=$((major + 1))
    minor=0
    patch=0
    ;;
  minor)
    minor=$((minor + 1))
    patch=0
    ;;
  patch) patch=$((patch + 1)) ;;
esac
new="$major.$minor.$patch"

# The workspace version (root), then the internal path-dep requirements in every member crate.
sed -i '' "s/^version = \"$old\"/version = \"$new\"/" Cargo.toml
for f in crates/*/Cargo.toml; do
  sed -i '' -E "/path = \"\.\.\// s/version = \"$old\"/version = \"$new\"/" "$f"
done

echo "bumped workspace version $old -> $new"
echo "remaining pins of the old version (should be none):"
grep -rn "\"$old\"" Cargo.toml crates/*/Cargo.toml || echo "  none"
