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

# Eval the agent-skills resolution model: harness self-test, then the live check against real agents.
# Supports `agent="auto|claude|gemini"` and optional `model` (defaults to zero-API `gemini skills list` or 1-call `haiku`).
# The live check skips cleanly when no usable agent binary is on PATH, so this is non-blocking, not in `gate`.
eval-skills agent="auto" *args:
    python3 .claude/skills/agent-skills/scripts/eval-resolution.py --self-test --agent claude
    python3 .claude/skills/agent-skills/scripts/eval-resolution.py --self-test --agent gemini
    python3 .claude/skills/agent-skills/scripts/eval-resolution.py --agent {{agent}} {{args}}

# Lightweight local trigger eval (1 batched call across all 20 queries in trigger-eval.json; switches claude/gemini by model).
eval-trigger model="haiku" *args:
    python3 .claude/skills/agent-skills/scripts/eval-trigger.py --model {{model}} {{args}}

# Everything CI checks on Rust: fmt-check + clippy + full tests.
gate: fmt-check clippy test

# Nix: build the package and run the flake checks (fmt, clippy, tests).
nix-check:
    nix flake check -L

# Use the repo's git hooks (.githooks/).
install-hooks:
    git config core.hooksPath .githooks
