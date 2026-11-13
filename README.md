# thepia (`the`)

[![CI](https://github.com/thepia/the/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/thepia/the/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)
[![MSRV](https://img.shields.io/badge/rustc-1.88%2B-orange.svg)](rust-toolchain.toml)
[![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Linux-lightgrey.svg)](#install)
[![Nix flake](https://img.shields.io/badge/Nix-flake-5277C3.svg?logo=nixos&logoColor=white)](flake.nix)
[![Security Policy](https://img.shields.io/badge/security-policy-brightgreen.svg)](docs/SECURITY.md)

Keeping your local AI abilities tidy. The `~/.agents` directory becomes a place to put your skills, roles, agents, and other configs so you can apply them where needed.

The release package is **`thepia`**, which installs the **`the`** CLI command (along with **`thepia`** and **`th`**). Alternate names for the command are `thepia` and `th`.

## Subcommands

- **`the skills`** - Inspect, validate, and refine agent skills across Claude Code, Gemini CLI, OpenCode, and Goose ([search & loading rules](docs/skills-search.md), [linting & quality checks](docs/SKILLS-LINT.md)).

Run `the --help` or `the <subcommand> --help` for usage details.

## Why `the skills lint`?

Most skill linters check a single `SKILL.md` file in isolation against basic regexes. `the skills lint` validates skills against how real coding agents discover, route, and execute them (see [docs/SKILLS-LINT.md](docs/SKILLS-LINT.md) for the full design):

1. **Cross-Agent Loadability & Precedence Simulation**
   - *See what every agent actually loads, not just whether the markdown parses.*
   - Simulates the real parent-directory walks, symlink hops, and loader strictness of Claude Code, Gemini CLI, OpenCode, and Goose simultaneously. Catches silent shadowing across scopes, OpenCode concurrent load conflicts (`warning[skill-conflict]`), and strict-vs-lenient loader splits (`claude: loaded` vs. `goose: skipped`).
2. **Agent Skills Spec & Extended Frontmatter Validation**
   - *One zero-dependency validator for both the strict `agentskills.io` spec and real-world agent extensions.*
   - Validates strict `agentskills.io` constraints (`name` format and directory match, `compatibility <= 500` chars, `metadata` string map) alongside agent-specific extensions (`argument-hint`, `user-invocable`, `context`, `agent`, `hooks`) in a single pass.
3. **Fence-Aware Local Link & Asset Integrity**
   - *Catch hallucinated asset paths before an agent hits `ENOENT` mid-task.*
   - Progressive disclosure relies on links to `./references/...` and `./scripts/...`. `the skills lint` strips fenced code blocks and inline code spans before verifying that every local Markdown link resolves on disk and stays within the skill root directory.
4. **Trigger Specificity & Pairwise Routing Overlap** (planned)
   - *Prevent routing collisions and dead skills before they confuse the model.*
   - Agents decide whether to activate a skill almost entirely from its `description`. Checks for explicit trigger conditions (`Use when...`), flags generic openers (`This skill...`), verifies distinctive vocabulary, and computes pairwise overlap across all active skills in your workspace.
5. **Zero-Latency Security Smoke Test**
   - *Instant guardrails against accidental secrets, invisible Trojans, and symlink escapes on every edit.*
   - Runs in-process during normal linting to catch symlink escapes (`error[symlink-escape]`), bidirectional/zero-width Trojan source characters (`error[trojan-source]`), exposed API keys (`error[exposed-secret]`), and unrestricted `Bash` grants (`warning[unrestricted-bash]`). Planned: unverified `curl | sh` pipes (`warning[remote-exec-pipe]`), and `the skills audit` delegating deep static taint analysis.

## Install

With Nix:

```sh
nix run github:Thepia/thepia -- --help          # try it
nix profile install github:Thepia/thepia        # install
```

With Cargo (Rust 1.88 or newer):

```sh
cargo install --git https://github.com/Thepia/the thepia
```

## Development

See [docs/DEVELOP.md](docs/DEVELOP.md) for architecture, design principles, and packaging details.

`nix develop` (or rustup, which reads `rust-toolchain.toml`) gives the pinned toolchain. Common tasks are `just` recipes: `just test`, `just clippy`, `just gate` (everything CI runs on Rust), and `just nix-check`. `just install-hooks` enables the pre-commit gate.

## License

Licensed under either of [Apache License 2.0](docs/LICENSE-APACHE) or [MIT](docs/LICENSE-MIT), at your option.
