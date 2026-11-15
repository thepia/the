#!/usr/bin/env bash
# SPIKE (WORK-0020 / WORK-0022): dev-machine Stage 2 for `the`. Take an RC's already-built, UNSIGNED
# per-arch macOS binaries from its GitHub Release, lipo them into a universal, Developer-ID sign,
# notarize, repackage, and publish the signed universal to the CDN + GitHub Release. No rebuild: it
# consumes the exact Stage-1 bytes.
#
# Reuses the dev machine's existing cli signing material (shared, provisioned per the release-signing
# skill): the "Developer ID Application" identity, the `thepia-signing` keychain, and the `thepia-notary`
# notarytool keychain profile. `the` is a plain CLI (no iCloud/keychain), so it is signed WITHOUT
# entitlements and with the hardened runtime, which is all notarization needs.
#
# Usage: finish-sign.sh [version] [--dry-run] [--no-notarize] [--no-cdn]
#   version        release to finish, no leading v (default: latest GitHub release tag)
#   --dry-run      download + lipo + sign + verify only; NO notarize, NO publish
#   --no-notarize  sign + publish without notarization (e.g. while the Apple agreement is being renewed)
#   --no-cdn       publish only to the GitHub Release; skip the Bunny CDN upload
#
# Caveat (by design): a bare Mach-O cannot have a notarization ticket *stapled* (only .app/.dmg/.pkg can).
# The universal binaries are notarized (ticket server-side; Gatekeeper checks online), which is fine for
# the non-quarantined `curl|sh`/CDN channel. Offline-stapled distribution would need a .pkg/.dmg wrapper.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
say() { printf '%s\n' "$*" >&2; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }
# Edge-purge versioned files we overwrote (deploy-cdn only purges latest.*). Needs BUNNY_API_KEY in env
# (source thepia-app-spec/.env or branding/.env before running); no-op without it.
purge_edge() {
  [ -n "${BUNNY_API_KEY:-}" ] || { say "   (no BUNNY_API_KEY; skipping edge purge)"; return 0; }
  for u in "$@"; do
    curl -fsS -o /dev/null -X POST -H "AccessKey: ${BUNNY_API_KEY}" \
      "https://api.bunny.net/purge?url=${u}&async=false" && say "   purged ${u}" || say "   purge failed: ${u}"
  done
}

VERSION=""
DRY_RUN=0 NO_NOTARIZE=0 NO_CDN=0
for a in "$@"; do
  case "$a" in
    --dry-run) DRY_RUN=1 ;;
    --no-notarize) NO_NOTARIZE=1 ;;
    --no-cdn) NO_CDN=1 ;;
    -*) die "unknown flag $a" ;;
    *) [ -z "$VERSION" ] && VERSION="$a" ;;
  esac
done
command -v gh >/dev/null || die "gh not found"
[ -z "$VERSION" ] && VERSION="$(gh release view --json tagName -q .tagName | sed 's/^v//')"

SIGN_KEYCHAIN="${THEPIA_SIGN_KEYCHAIN:-thepia-signing.keychain-db}"
NOTARY_PROFILE="${THEPIA_NOTARY_PROFILE:-thepia-notary}"
SIGNID="$(security find-identity -v -p codesigning 2>/dev/null | awk '/Developer ID Application/ {print $2; exit}')"
[ -n "$SIGNID" ] || die "no Developer ID Application identity (run on your provisioned signing Mac)"

BINS=(the th thepia)
MAC_TARGETS=(x86_64-apple-darwin aarch64-apple-darwin)
WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT

say ">> finishing thepia v${VERSION} (universal, Developer ID ${SIGNID}; dry-run=${DRY_RUN}, notarize=$((1-NO_NOTARIZE)))"

# 1. Fetch the Stage-1 per-arch macOS tarballs and extract each binary.
for t in "${MAC_TARGETS[@]}"; do
  tb="thepia-v${VERSION}-${t}.tar.gz"
  say ">> fetch ${tb}"
  gh release download "v${VERSION}" --pattern "$tb" --dir "$WORK/dl" --clobber
  tar -xzf "$WORK/dl/${tb}" -C "$WORK/dl"
done

# 2. lipo each of the/th/thepia into a universal.
mkdir -p "$WORK/universal"
for b in "${BINS[@]}"; do
  lipo -create \
    "$WORK/dl/thepia-v${VERSION}-x86_64-apple-darwin/${b}" \
    "$WORK/dl/thepia-v${VERSION}-aarch64-apple-darwin/${b}" \
    -output "$WORK/universal/${b}"
  say "   lipo ${b} -> $(lipo -archs "$WORK/universal/${b}")"
done

# 3. Developer-ID sign (hardened runtime + secure timestamp; no entitlements for the plain CLI).
for b in "${BINS[@]}"; do
  codesign --force --options runtime --timestamp --keychain "$SIGN_KEYCHAIN" --sign "$SIGNID" "$WORK/universal/${b}"
  codesign --verify --strict --verbose=2 "$WORK/universal/${b}" 2>&1 | sed 's/^/   /' >&2
done

# 4. Notarize (zip submission; bare binaries can't be stapled, so Gatekeeper checks online).
if [ "$DRY_RUN" -eq 0 ] && [ "$NO_NOTARIZE" -eq 0 ]; then
  ( cd "$WORK/universal" && zip -q "$WORK/notarize.zip" "${BINS[@]}" )
  say ">> submitting to notarytool (profile ${NOTARY_PROFILE})"
  xcrun notarytool submit "$WORK/notarize.zip" --keychain-profile "$NOTARY_PROFILE" --wait
else
  say ">> notarization skipped ($([ "$DRY_RUN" -eq 1 ] && echo dry-run || echo --no-notarize))"
fi

# 5. Repackage the signed universal tarball (+ sha256), mirroring the per-arch layout.
stage="thepia-v${VERSION}-universal-apple-darwin"
mkdir -p "$WORK/dist/${stage}"
cp "$WORK/universal/the" "$WORK/universal/th" "$WORK/universal/thepia" "$WORK/dist/${stage}/"
for extra in README.md LICENSE-MIT LICENSE-APACHE; do
  cp "$WORK/dl/thepia-v${VERSION}-aarch64-apple-darwin/${extra}" "$WORK/dist/${stage}/" 2>/dev/null || true
done
tar -C "$WORK/dist" -czf "$WORK/dist/${stage}.tar.gz" "${stage}"
rm -rf "$WORK/dist/${stage}"                       # keep only uploadable files in dist/
( cd "$WORK/dist" && shasum -a 256 "${stage}.tar.gz" > "${stage}.tar.gz.sha256" )
usum="$(awk '{print $1}' "$WORK/dist/${stage}.tar.gz.sha256")"
say ">> built ${usum}  ${stage}.tar.gz"

# 5b. Refresh the release manifests to INCLUDE the universal, keeping the per-arch building blocks
# (npm-publish-release + the combine step consume them). Start from the release's existing SHA256SUMS,
# drop any stale universal line, add ours, regenerate release.json. package=thepia (matches release.yml).
mkdir -p "$WORK/manifest"
gh release download "v${VERSION}" --pattern SHA256SUMS --dir "$WORK/manifest" --clobber
grep -v 'universal-apple-darwin' "$WORK/manifest/SHA256SUMS" > "$WORK/dist/SHA256SUMS" || true
printf '%s  %s\n' "$usum" "${stage}.tar.gz" >> "$WORK/dist/SHA256SUMS"
sort -o "$WORK/dist/SHA256SUMS" "$WORK/dist/SHA256SUMS"
python3 - "$VERSION" "https://cdn.thepia.com/the/${VERSION}" "$WORK/dist/SHA256SUMS" > "$WORK/dist/release.json" <<'PY'
import datetime, json, sys
from pathlib import Path
version, base, sums = sys.argv[1], sys.argv[2], Path(sys.argv[3])
assets = []
for line in sums.read_text().splitlines():
    p = line.split()
    if len(p) == 2:
        assets.append({"name": p[1], "url": f"{base}/{p[1]}", "sha256": p[0]})
print(json.dumps({
    "package": "thepia", "version": version, "tag": f"v{version}",
    "published_at": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
    "base_url": base, "assets": assets,
}, indent=2))
PY
say ">> refreshed manifests: $(grep -c . "$WORK/dist/SHA256SUMS") assets, package=thepia"

if [ "$DRY_RUN" -eq 1 ]; then
  say ">> dry-run: not publishing. Staged in $WORK/dist (removed on exit):"
  ( cd "$WORK/dist" && ls -1 ) | sed 's/^/     /' >&2
  exit 0
fi

# 6. Publish: GitHub Release (universal + refreshed manifests) + CDN mirror, then edge-purge the
# versioned files we overwrote. deploy-cdn purges latest.* and the two manifests; the universal tarball
# shares the versioned path and changes bytes on a re-sign, so purge it here too.
say ">> uploading to GitHub Release v${VERSION}"
gh release upload "v${VERSION}" \
  "$WORK/dist/${stage}.tar.gz" "$WORK/dist/${stage}.tar.gz.sha256" \
  "$WORK/dist/SHA256SUMS" "$WORK/dist/release.json" --clobber
if [ "$NO_CDN" -eq 0 ]; then
  say ">> uploading to cdn.thepia.com/the/${VERSION}/"
  scripts/deploy-cdn.sh "$VERSION" "$WORK/dist"
  purge_edge "https://cdn.thepia.com/the/${VERSION}/${stage}.tar.gz" \
             "https://cdn.thepia.com/the/${VERSION}/${stage}.tar.gz.sha256"
fi
say ">> done: signed universal + refreshed manifests published for v${VERSION}$([ "$NO_NOTARIZE" -eq 1 ] && echo ' (NOT notarized)')"
