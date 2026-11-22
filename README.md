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

- **`the skills`** - Inspect, validate, and refine agent skills across Claude Code, Gemini CLI, OpenCode, and Goose. See [docs/skills.md](docs/skills.md).

Run `the --help` or `the <subcommand> --help` for usage details.

### Why lint your skills

A skill that is wrong does not error loudly; it is silently ignored, mis-routed, or quietly misbehaves mid-task. `the skills` catches that before it costs you a run:

- **Catch mistakes early.** Invalid frontmatter, out-of-range field values, duplicate keys, and broken links are flagged at author time, not discovered when the agent fails to load or use the skill.
- **Guard against injections and unsafe skills.** Hidden or bidirectional Unicode, exposed secrets, and over-broad shell permissions are flagged, with an opt-in deep security audit for skills you did not write.
- **Improve your coding assistant's performance.** Description and trigger quality plus a token budget keep the agent routing to the *right* skill instead of burning context on a vague or bloated one.
- **Works across every agent your team uses.** `the` models each agent's real discovery, so it tells you whether Claude Code, Gemini, OpenCode, or Goose will actually load a skill, or shadow, conflict with, or skip it, rather than assuming the one you tested.
- **Stays portable and self-contained.** Links, assets, and paths are checked so a skill resolves on a teammate's machine, not just where it was written.
- **Clear, fixable diagnostics.** Every finding states what is wrong, why it matters, and the exact fix, with no opaque letter grade.

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

From a GitHub release tarball (no package manager): download the bundle for your OS/CPU from the
[releases page](https://github.com/Thepia/the/releases), then run the bundled `install.sh`. It installs
the binary under `~/.thepia/bin` and links `thepia` / `the` / `th` onto your PATH if a standard bin
directory is writable, falling back to a one-line PATH instruction otherwise (no `sudo` is forced).

```sh
tar xzf thepia-v*-*.tar.gz && ./thepia-v*-*/install.sh
```

## Development

See [docs/DEVELOP.md](docs/DEVELOP.md) for architecture, design principles, and packaging details.

`nix develop` (or rustup, which reads `rust-toolchain.toml`) gives the pinned toolchain. Common tasks are `just` recipes: `just test`, `just clippy`, `just gate` (everything CI runs on Rust), and `just nix-check`. `just install-hooks` enables the pre-commit gate.

## License

Licensed under either of [Apache License 2.0](docs/LICENSE-APACHE) or [MIT](docs/LICENSE-MIT), at your option.
