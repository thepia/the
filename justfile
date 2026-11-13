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
    cargo test --workspace --lib --bins

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

# Eval the agent-skills resolution model: harness self-test, then the live check against all installed agents.
# Supports `agent="all|auto|claude|gemini|opencode"` and optional `model` (zero API calls on Gemini/OpenCode, 1 Haiku call on Claude).
# The live check skips cleanly when an agent binary is missing or rate-limited, so this is non-blocking, not in `gate`.
eval-skills agent="all" *args:
    python3 scripts/skills/eval-resolution.py --self-test --agent {{agent}}
    python3 scripts/skills/eval-resolution.py --agent {{agent}} {{args}}

# Lightweight local trigger eval (1 batched call across all 20 queries in trigger-eval.json; switches claude/gemini by model).
eval-trigger model="haiku" *args:
    python3 scripts/skills/eval-trigger.py --model {{model}} {{args}}

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

# Use the repo's git hooks (.githooks/).
install-hooks:
    git config core.hooksPath .githooks
