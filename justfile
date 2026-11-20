# `the` dev tasks. Run `just` (or `just --list`) to see everything.
#
# Toolchain: pinned in rust-toolchain.toml. On Nix, `nix develop` provides it; elsewhere rustup reads
# the pin. Prefer these targets over hand-rolled cargo lines.

# List recipes (default).
default:
    @just --list

# Debug build.
build:
    #!/usr/bin/env bash
    set -euo pipefail
    . scripts/sccache-env.sh
    cargo build --workspace

# Release build.
build-release:
    #!/usr/bin/env bash
    set -euo pipefail
    . scripts/sccache-env.sh
    cargo build --workspace --release

# Run `the` with arguments, e.g. `just run skills list`.
run *args:
    #!/usr/bin/env bash
    set -euo pipefail
    . scripts/sccache-env.sh
    cargo run --quiet --bin the -- {{args}}

# Unit tests only (the pre-commit hook gate).
test-unit:
    #!/usr/bin/env bash
    set -euo pipefail
    . scripts/sccache-env.sh
    cargo test --workspace --lib --bins

# Full test suite (unit + integration).
test:
    #!/usr/bin/env bash
    set -euo pipefail
    . scripts/sccache-env.sh
    cargo test --workspace

# Lints, warnings as errors.
clippy:
    #!/usr/bin/env bash
    set -euo pipefail
    . scripts/sccache-env.sh
    cargo clippy --workspace --all-targets -- -D warnings

# Format the code.
fmt:
    cargo fmt --all

# Check formatting without changing files.
fmt-check:
    cargo fmt --all -- --check

# Offline doc link check (needs lychee).
links:
    lychee --offline --no-progress "*.md" "docs/**/*.md"

# Everything CI checks on Rust: fmt-check + clippy + full tests.
gate: fmt-check clippy test

# Nix: build the package and run the flake checks (fmt, clippy, tests).
nix-check:
    nix flake check -L

# Local release dry-run: gate, release binaries (`thepia`, `the`, `th`), and cargo package check.
release-dry-run: gate build-release
    cargo package --list -p thepia --allow-dirty
    ./target/release/thepia --version
    ./target/release/the --version
    ./target/release/th --version

# Build the host-platform npm platform package from a release build, then pack the launcher + it into
# npm/dist (local verification of the `npx thepia` channel; see ADR-0004 / docs/DEVELOP.md).
npm-pack: build-release
    node scripts/build-npm.mjs assemble
    node scripts/build-npm.mjs pack

# Pin the launcher's version + optionalDependencies to the Cargo.toml workspace version.
npm-sync:
    node scripts/build-npm.mjs sync

# Private end-to-end smoke test: publish the launcher + this platform's package to a throwaway local
# verdaccio and run `npx thepia@<pre> skills list` against it. Nothing touches npmjs.org. Needs node/npx.
npm-smoke:
    bash scripts/npm-smoke.sh

# Dev-machine npm publish of an already-built release: download the RC's Stage-1 binaries from the
# GitHub Release and publish the launcher + platform packages to npmjs.org (local `npm login` supplies
# the 2FA OTP that CI can't). e.g. `just npm-publish-release 0.1.0` or `... 0.1.0 --tag rc --dry-run`.
npm-publish-release *args:
    bash scripts/npm-publish-release.sh {{args}}

# SPIKE (WORK-0020): dev-machine Stage 2. Sign (+ notarize) the latest RC's macOS binaries into a
# universal and publish it to the GitHub Release + CDN. Reuses the cli Developer-ID identity + notary
# profile. e.g. `just finish-sign`, `just finish-sign 0.1.0 --dry-run`, `... 0.1.0 --no-notarize`.
finish-sign *args:
    bash scripts/finish-sign.sh {{args}}

# Use the repo's git hooks (.githooks/).
install-hooks:
    git config core.hooksPath .githooks

# Measure test coverage (cargo-llvm-cov; needs `rustup component add llvm-tools-preview`
# and `cargo install cargo-llvm-cov`). Per-file line/region/function coverage.
coverage:
    cargo llvm-cov --summary-only
