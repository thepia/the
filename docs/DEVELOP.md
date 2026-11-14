# Developing & contributing to `thepia` (`the`)

How to build, test, and contribute to `the`. Contributions are welcome — **bugfixes, typos, and
clarifications to user-facing docs** are especially appreciated. For anything larger, open an issue to
discuss first.

## Setup

[`rust-toolchain.toml`](../rust-toolchain.toml) pins the Rust compiler. Run `nix develop` (or use
`rustup`) to enter the pinned environment with `just` and `lychee`.

## Build, test, lint (`just`)

Run `just` (or `just --list`) for all tasks in [`justfile`](../justfile):

| Command | Description |
|---|---|
| `just test-unit` | Fast unit tests (`--lib --bins`), also run by the `.githooks/pre-commit` hook |
| `just test` | Full unit + integration test suite |
| `just clippy` | Clippy across workspace and all targets with `-D warnings` |
| `just fmt` / `just fmt-check` | Format or verify formatting with `rustfmt` |
| `just links` | Offline markdown link check via `lychee` |
| `just gate` | Everything CI checks on Rust: `fmt-check` + `clippy` + `test` |
| `just nix-check` | Build the Nix package and run flake checks |
| `just eval-skills` | Run the agent-skills resolution oracle self-test and live agent verification |
| `just install-hooks` | Point `git config core.hooksPath` to `.githooks` |

## Binaries

The release package `thepia` ships three command names from one compiled binary: **`the`** (primary),
**`thepia`** (full name), and **`th`** (shorthand). [`src/lib.rs`](../crates/the/src/lib.rs) holds the
implementation and exports `run()`; `main.rs`, `bin/the.rs`, and `bin/th.rs` are 3-line entrypoints.
Invoking `the`/`th` re-execs as `thepia`, so `ps`/Activity Monitor show the process as `thepia`.

## Codebase layout

```text
crates/the/
├── Cargo.toml          # Package `thepia`, bins `thepia`, `the`, `th`
├── src/
│   ├── lib.rs          # Entrypoint `run()`, process-name handoff, top-level dispatch
│   ├── main.rs         # `thepia` binary entrypoint
│   ├── bin/
│   │   ├── the.rs      # `the` binary entrypoint
│   │   └── th.rs       # `th` binary entrypoint
│   ├── cli.rs          # Clap CLI surface (`Cli`, `Command`, subcommand args)
│   ├── diag.rs         # `Diagnostic`, `Level`, semantic `Exit` codes
│   ├── output.rs       # `Format` (`Human` | `Json`) and pipe-safe stdout writer
│   ├── style.rs        # TTY & color detection (`Term`) and `anstyle` palette
│   ├── table.rs        # `Report`, `Table`, `Record`, `Cell`, human & JSON renderers
│   └── skills/         # `the skills` subcommand implementation
│       ├── cmd.rs      # `list`, `show`, `dirs` report builders
│       ├── locate.rs   # Per-agent directory search plans (Claude, OpenCode, Goose)
│       ├── scan.rs     # Filesystem traversal, depth rules, symlink tracking
│       ├── parse.rs    # SKILL.md YAML frontmatter parser & Agent Skills format linter
│       ├── resolve.rs  # Per-agent precedence, shadowing, and conflict resolution
│       └── installed.rs# Detection of installed agent CLIs
└── tests/
    └── skills_cli.rs   # End-to-end integration tests in an isolated HOME & git sandbox
```

## Making a change

### Adding a subcommand
1. Define the subcommand variant and arguments in [`src/cli.rs`](../crates/the/src/cli.rs) under
   `enum Command`.
2. Create a module `src/<subcommand>/` that builds a [`table::Report`](../crates/the/src/table.rs) and
   emits any [`Diagnostic`](../crates/the/src/diag.rs)s via `diag.emit(format)`.
3. Wire the variant in [`src/lib.rs`](../crates/the/src/lib.rs).
4. Add hermetic integration tests in `crates/the/tests/` covering both human and `--format json` output,
   diagnostics on `stderr`, and exit codes.

## Conventions

`the` follows strict output discipline: `stdout` carries only the result, `stderr` carries diagnostics,
one global `--format human|json` governs both, and errors are rustc-style (what / why / fix) with stable
kebab codes and semantic exit codes. Commands build a declarative
[`Report`](../crates/the/src/table.rs) rather than printing directly, so formatting stays uniform. Keep
new commands consistent with the existing ones.

## Contributing

Open a PR against `main`. Run `just gate` before pushing and follow the existing commit style. Good
first contributions: **bugfixes, typos, and clarifications to user-facing documentation**.
