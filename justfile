# `the` dev tasks. Run `just` (or `just --list`) to see everything.
#
# Toolchain: pinned in rust-toolchain.toml. On Nix, `nix develop` provides it; elsewhere rustup reads
# the pin. Prefer these targets over hand-rolled cargo lines.

# List recipes (default).
default:
    @just --list

# Debug build.
build:
    cargo build --workspace

# Release build.
build-release:
    cargo build --workspace --release

# Run `the` with arguments, e.g. `just run skills list`.
run *args:
    cargo run --quiet --bin the -- {{args}}

# Unit tests only (the pre-commit hook gate).
test-unit:
    cargo test --workspace --bins

# Full test suite (unit + integration).
test:
    cargo test --workspace

# Lints, warnings as errors.
clippy:
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

# Use the repo's git hooks (.githooks/).
install-hooks:
    git config core.hooksPath .githooks
