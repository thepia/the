# shellcheck shell=sh
# Sourceable: turn on the shared sccache -> Bunny S3 build cache when it is available, so `the` warm
# builds (debug and release, macOS and Linux) are served from the same cache CI populates. Sourced by
# the cache-using `just` recipes and the pre-commit hook so they share ONE cache config (and the same
# CARGO_INCREMENTAL=0, so alternating between them never thrashes the shared target/). This is the same
# bucket the `cli` repo uses; sccache keys are content-hashed, so the two repos coexist safely.
#
# Secret source (platform idiom): the macOS Keychain generic-password item
#   service com.thepia.SCCACHE_BUNNY_SECRET   (store once, see below)
# falling back to the SCCACHE_BUNNY_SECRET env var (CI sets it). Store it once per Mac (no sudo):
#   security add-generic-password -U -s com.thepia.SCCACHE_BUNNY_SECRET -a thepia-cli-build -w '<secret>'
#
# When sccache is installed but no secret is reachable, fall back to sccache's LOCAL disk cache (still a
# real speedup across local rebuilds). No-op (plain uncached cargo) only when sccache itself is absent.
# Prints one status line to stderr. Safe under `set -e`, in both sh and bash.

if command -v sccache >/dev/null 2>&1; then
    export RUSTC_WRAPPER=sccache CARGO_INCREMENTAL=0

    _sccache_secret="${SCCACHE_BUNNY_SECRET:-}"
    if [ -z "$_sccache_secret" ] && command -v security >/dev/null 2>&1; then
        _sccache_secret="$(security find-generic-password -s com.thepia.SCCACHE_BUNNY_SECRET -w 2>/dev/null || true)"
    fi

    if [ -n "$_sccache_secret" ]; then
        # SCCACHE_IDLE_TIMEOUT=0 -> the server must NOT idle-shut-down during a long single compile
        # (a release build with LTO) and strand the rest of the build on local misses (mozilla/sccache#2069).
        export SCCACHE_BUCKET=thepia-cli-build SCCACHE_ENDPOINT=de-s3.storage.bunnycdn.com \
            SCCACHE_REGION=de SCCACHE_S3_USE_SSL=true SCCACHE_S3_KEY_PREFIX=sccache \
            AWS_ACCESS_KEY_ID=thepia-cli-build AWS_SECRET_ACCESS_KEY="$_sccache_secret" \
            SCCACHE_IDLE_TIMEOUT=0
        echo ">> build cache: sccache -> Bunny S3 (thepia-cli-build)" >&2
    else
        echo ">> build cache: sccache local disk (no Bunny secret in Keychain/env)" >&2
    fi
    unset _sccache_secret
else
    echo ">> build cache: OFF (sccache not installed; brew install sccache to enable)" >&2
fi
