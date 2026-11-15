#!/usr/bin/env bash
# Mirror a Thepia/the release to the Bunny CDN served at https://cdn.thepia.com/the/.
#
# Artifacts go to IMMUTABLE versioned paths /the/<version>/... (the `thepia` pull zone forces a 30-day
# edge TTL, so a URL must never be overwritten in place). The mutable pointers /the/latest (the version
# string) and /the/latest.json are re-uploaded and then edge-purged each release.
#
# Usage: scripts/deploy-cdn.sh <version> <artifacts-dir>
#   <version>        e.g. 0.1.0 (no leading v)
#   <artifacts-dir>  dir of files to publish under /the/<version>/ (tar.gz, SHA256SUMS, the.rb, etc.)
#
# Credentials (from CI secrets or locally from branding/.env):
#   BUNNY_UPLOAD_PW  storage-zone AccessKey (uploads)
#   BUNNY_API_KEY    account key (edge cache purge)
# Channel discipline: progress/notices -> stderr; the public base URL -> stdout.
set -euo pipefail

say() { printf '%s\n' "$*" >&2; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

version="${1:-}"
artifacts="${2:-}"
[ -n "$version" ] && [ -n "$artifacts" ] || die "usage: deploy-cdn.sh <version> <artifacts-dir>"
[ -d "$artifacts" ] || die "not a directory: $artifacts"

# Load Bunny credentials from branding/.env when running locally if not already in env.
branding_env="${BRANDING_ENV:-/Volumes/Projects/Thepia/branding/.env}"
if [ -z "${BUNNY_UPLOAD_PW:-}" ] && [ -f "$branding_env" ]; then
  set -a
  # shellcheck disable=SC1090
  . "$branding_env"
  set +a
fi
[ -n "${BUNNY_UPLOAD_PW:-}" ] || die "BUNNY_UPLOAD_PW not set (set secret in CI or provide $branding_env)"

storage="https://storage.bunnycdn.com/thepia"   # zone 'thepia' -> served at cdn.thepia.com/<path>
cdn="https://cdn.thepia.com"
base="the/${version}"

put() {
  local src="$1" path="$2" code
  code="$(curl -fsS -o /dev/null -w '%{http_code}' -X PUT \
    -H "AccessKey: ${BUNNY_UPLOAD_PW}" \
    -H 'Content-Type: application/octet-stream' \
    --data-binary @"$src" "${storage}/${path}")" \
    || die "upload failed: $path"
  case "$code" in
    2*) say "  put ${path} (${code})" ;;
    *)  die "upload $path got HTTP $code" ;;
  esac
}

put_string() {
  local content="$1" path="$2" code
  code="$(curl -fsS -o /dev/null -w '%{http_code}' -X PUT \
    -H "AccessKey: ${BUNNY_UPLOAD_PW}" \
    -H 'Content-Type: text/plain' \
    --data-binary "$content" "${storage}/${path}")" \
    || die "upload failed: $path"
  case "$code" in
    2*) say "  put ${path} (${code})" ;;
    *)  die "upload $path got HTTP $code" ;;
  esac
}

purge() {
  local url="$1"
  [ -n "${BUNNY_API_KEY:-}" ] || { say "  (no BUNNY_API_KEY; skipping purge of $url)"; return 0; }
  curl -fsS -o /dev/null -X POST -H "AccessKey: ${BUNNY_API_KEY}" \
    "https://api.bunny.net/purge?url=${url}&async=false" \
    && say "  purged ${url}" || say "  purge failed: $url"
}

say ">> mirroring the ${version} to ${cdn}/${base}/"

# 1. Ensure SHA256SUMS covers the release archives.
if [ ! -f "$artifacts/SHA256SUMS" ]; then
  sums="$(mktemp)"
  trap 'rm -f "$sums"' EXIT
  ( cd "$artifacts" && shasum -a 256 -- *.tar.gz > "$sums" ) || die "checksum failed"
  cp "$sums" "$artifacts/SHA256SUMS"
fi

# 2. Generate release.json manifest if not already present.
if [ ! -f "$artifacts/release.json" ]; then
  python3 - "$version" "$cdn/$base" "$artifacts/SHA256SUMS" > "$artifacts/release.json" <<'PY'
import datetime, json, sys
from pathlib import Path

version, base_url, sums_path = sys.argv[1], sys.argv[2], Path(sys.argv[3])
assets = []
for line in sums_path.read_text().splitlines():
    parts = line.split()
    if len(parts) == 2:
        sha256, name = parts
        assets.append({
            "name": name,
            "url": f"{base_url}/{name}",
            "sha256": sha256,
        })
print(json.dumps({
    "package": "thepia",
    "version": version,
    "tag": f"v{version}",
    "published_at": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
    "base_url": base_url,
    "assets": assets,
}, indent=2))
PY
fi

# 3. Upload every artifact (including SHA256SUMS, release.json, and the.rb) under /the/<version>/.
for f in "$artifacts"/*; do
  [ -f "$f" ] || continue
  put "$f" "${base}/$(basename "$f")"
done

# 3b. The per-version manifests are the only versioned files Stage 2 (sign+notarize) rewrites to add the
# universal, so purge them at the edge; the tarballs themselves keep immutable, never-overwritten URLs.
for m in SHA256SUMS release.json; do
  [ -f "$artifacts/$m" ] && purge "${cdn}/${base}/${m}"
done

# 4. Update mutable pointers (/the/latest.json and /the/latest) and edge-purge them.
put "$artifacts/release.json" "the/latest.json"
purge "${cdn}/the/latest.json"

put_string "$version" "the/latest"
purge "${cdn}/the/latest"

say ">> done."
printf '%s/%s/\n' "$cdn" "$base"
