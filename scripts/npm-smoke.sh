#!/usr/bin/env bash
# Private end-to-end smoke test of the `thepia` npm distribution against a throwaway local verdaccio
# registry. It builds the host binary, assembles the launcher + this platform's package at a prerelease
# version pointed at verdaccio, publishes them, then runs `npx thepia@<ver> skills list` from a clean
# project resolving through verdaccio, and asserts it works. Nothing touches npmjs.org; everything is
# torn down on exit.
#
# Scope: proves the real resolution path (optionalDependencies `os`/`cpu` gating, `require.resolve`,
# launcher exec) for THIS host platform only. The other targets' binaries come from CI, not a local
# cross-build, so their packages are legitimately absent here (npm skips them as optional).
#
# Requires: cargo (pinned toolchain), node, and npx (verdaccio is fetched via `npx verdaccio`).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

PRE="${THEPIA_NPM_PRE:-smoke}"
# npm refuses to publish a prerelease version to `latest` without an explicit dist-tag.
TAG="${THEPIA_NPM_TAG:-smoke}"
PORT="${VERDACCIO_PORT:-4873}"
REGISTRY="http://localhost:${PORT}"
VERDACCIO_VERSION="${VERDACCIO_VERSION:-6}"

case "$(uname -s)-$(uname -m)" in
  Darwin-arm64)  TRIPLE=aarch64-apple-darwin ;;
  Darwin-x86_64) TRIPLE=x86_64-apple-darwin ;;
  Linux-x86_64)  TRIPLE=x86_64-unknown-linux-gnu ;;
  Linux-aarch64) TRIPLE=aarch64-unknown-linux-gnu ;;
  *) echo "npm-smoke: unsupported host $(uname -s)-$(uname -m)" >&2; exit 1 ;;
esac

WORK="$(mktemp -d)"
VERDACCIO_PID=""
cleanup() {
  [ -n "$VERDACCIO_PID" ] && kill "$VERDACCIO_PID" 2>/dev/null || true
  rm -rf "$WORK"
}
trap cleanup EXIT

# ---- assemble the launcher + this platform's package, pinned at verdaccio + a prerelease version ----
echo ">> building host thepia binary ($TRIPLE)" >&2
cargo build --bin thepia
BIN="$ROOT/target/debug/thepia"
[ -x "$BIN" ] || { echo "npm-smoke: binary not found at $BIN" >&2; exit 1; }

rm -rf "$ROOT/npm/platforms" "$ROOT/npm/dist"   # drop stale/renamed platform dirs before building
node scripts/build-npm.mjs platform "$TRIPLE" "$BIN" --pre "$PRE" --registry "$REGISTRY"
VERSION="$(node -e "process.stdout.write(require('./npm/thepia/package.json').version)")"
echo ">> assembled thepia@${VERSION} -> ${REGISTRY}" >&2

# ---- start a throwaway verdaccio that allows anonymous publish of our names ----
cat > "$WORK/verdaccio.yaml" <<YAML
storage: ${WORK}/storage
auth:
  htpasswd:
    file: ${WORK}/htpasswd
    max_users: -1
uplinks:
  npmjs:
    url: https://registry.npmjs.org/
packages:
  '@thepia/*':
    access: \$all
    publish: \$all
    unpublish: \$all
  'thepia':
    access: \$all
    publish: \$all
    unpublish: \$all
  '@*/*':
    access: \$all
    publish: \$all
    proxy: npmjs
  '**':
    access: \$all
    publish: \$all
    proxy: npmjs
log: { type: stdout, format: pretty, level: warn }
YAML

echo ">> starting verdaccio@${VERDACCIO_VERSION} on ${REGISTRY}" >&2
npx --yes "verdaccio@${VERDACCIO_VERSION}" --config "$WORK/verdaccio.yaml" --listen "$PORT" > "$WORK/verdaccio.log" 2>&1 &
VERDACCIO_PID=$!

# Wait for it to accept connections (up to ~30s).
for _ in $(seq 1 60); do
  if curl -fsS "${REGISTRY}/-/ping" >/dev/null 2>&1 || curl -fsS "${REGISTRY}/" >/dev/null 2>&1; then
    break
  fi
  sleep 0.5
done
curl -fsS "${REGISTRY}/" >/dev/null 2>&1 || { echo "npm-smoke: verdaccio did not start" >&2; cat "$WORK/verdaccio.log" >&2; exit 1; }

# A dummy token satisfies the npm client; verdaccio accepts it because publish is allowed for $all.
PUBRC="$WORK/.npmrc-publish"
cat > "$PUBRC" <<RC
registry=${REGISTRY}
@thepia:registry=${REGISTRY}
//localhost:${PORT}/:_authToken=smoke-test-token
RC

# ---- publish platform packages first, then the launcher (so optionalDependencies resolve) ----
# Prefix ./ so npm treats these as local folders, not GitHub `org/repo` shorthands (e.g. `npm/thepia`).
for p in npm/platforms/*/; do
  echo ">> publish ${p}" >&2
  npm publish --userconfig "$PUBRC" --tag "$TAG" "./${p}"
done
echo ">> publish npm/thepia" >&2
npm publish --userconfig "$PUBRC" --tag "$TAG" ./npm/thepia

# ---- consume: a clean project runs npx against verdaccio ----
CONSUMER="$WORK/consumer"
mkdir -p "$CONSUMER"
cat > "$CONSUMER/.npmrc" <<RC
registry=${REGISTRY}
@thepia:registry=${REGISTRY}
RC
( cd "$CONSUMER" && npm init -y >/dev/null 2>&1 )

echo ">> npx --registry ${REGISTRY} thepia@${VERSION} skills list" >&2
OUT="$WORK/out.txt"
( cd "$CONSUMER" && npx --yes --registry "${REGISTRY}" "thepia@${VERSION}" skills list ) > "$OUT" 2>"$WORK/err.txt" || {
  echo "npm-smoke: FAILED to run thepia@${VERSION} via npx" >&2
  echo "--- stderr ---" >&2; cat "$WORK/err.txt" >&2
  exit 1
}

if [ ! -s "$OUT" ]; then
  echo "npm-smoke: thepia ran but produced no stdout (expected a skills report)" >&2
  exit 1
fi

echo ">> OK: npx thepia@${VERSION} skills list produced $(wc -l < "$OUT" | tr -d ' ') rows of output" >&2
echo ">> npm-smoke passed for ${TRIPLE} (verdaccio, nothing published to npmjs.org)" >&2
