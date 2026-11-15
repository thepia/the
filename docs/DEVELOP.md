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

## Build cache (sccache)

`the`'s debug and release builds (macOS and Linux, local and CI) run through
[sccache](https://github.com/mozilla/sccache) sharing the same Bunny S3 bucket as `cli`
(`thepia-cli-build`); sccache keys are content-hashed, so the repos coexist. Locally the `just`
build/test recipes and the pre-commit hook source [`scripts/sccache-env.sh`](../scripts/sccache-env.sh),
which reads the `SCCACHE_BUNNY_SECRET` from the macOS Keychain
(`com.thepia.SCCACHE_BUNNY_SECRET`) or env; with no secret it falls back to sccache's local disk cache,
and with no sccache installed it is a no-op (uncached). CI installs `mozilla-actions/sccache-action` and
points at the same bucket when the `SCCACHE_BUNNY_SECRET` secret is present. This is the platform-wide
policy (see the spec repo's Rust-build-cache policy). `cargo install` / Nix builds by external users are
unaffected (they do not use the `just` recipes). Store the secret once per Mac:
`security add-generic-password -U -s com.thepia.SCCACHE_BUNNY_SECRET -a thepia-cli-build -w '<secret>'`.

## Binaries

The release package `thepia` ships three command names from one compiled binary: **`the`** (primary),
**`thepia`** (full name), and **`th`** (shorthand). [`src/lib.rs`](../crates/the/src/lib.rs) holds the
implementation and exports `run()`; `main.rs`, `bin/the.rs`, and `bin/th.rs` are 3-line entrypoints.
Invoking `the`/`th` re-execs as `thepia`, so `ps`/Activity Monitor show the process as `thepia`.

## npm distribution (`npx thepia`)

`the` is also published to npm as the unscoped package [`thepia`](https://www.npmjs.com/package/thepia),
so `npx thepia skills lint` works with only Node installed. Per **ADR-0004** the binaries ship over the
npm registry channel (no CDN, no postinstall download), so it installs behind registry-proxying corporate
firewalls. Layout under [`npm/`](../npm/):

- `npm/thepia/` — the tracked launcher package. It carries **no binary**: just `bin/the.js` (a Node
  shim) and a `package.json` whose `bin` maps `thepia`/`the`/`th` to the shim and whose
  `optionalDependencies` list the per-platform packages.
- `npm/platforms/<pkg>/` — generated, git-ignored. One package per target
  (`@thepia/cli-{darwin-arm64,darwin-x64,linux-x64,linux-arm64}`), each declaring matching `os`/`cpu`
  and carrying the prebuilt binary (named `thepia`).

npm installs **only** the platform package whose `os`/`cpu` match the machine (optional-dep gating), so a
user downloads just their platform's binary. `bin/the.js` resolves that binary with `require.resolve` and
execs it, passing argv/stdio/exit through. Unsupported platforms (Windows, musl/Alpine, other CPUs) print
a `cargo` / Nix install message.

[`scripts/build-npm.mjs`](../scripts/build-npm.mjs) is the single source of truth for the target set and
pins every version to the `Cargo.toml` workspace version:

| Command | Description |
|---|---|
| `just npm-sync` | Pin the launcher version + `optionalDependencies` to `Cargo.toml` |
| `just npm-pack` | Release-build, assemble the host platform package, and `npm pack` into `npm/dist` |
| `just npm-smoke` | Publish to a throwaway local verdaccio and run `npx thepia@<pre> skills list` against it (nothing touches npmjs.org) |

For a public beta before claiming `latest`, stamp a prerelease and publish under a dist-tag:
`node scripts/build-npm.mjs assemble --from <dir> --pre beta.0` then `npm publish --tag beta` each package;
users test with `npx thepia@beta skills lint`.
| `node scripts/build-npm.mjs platform <triple> <binary>` | Build one platform package from a built binary |
| `node scripts/build-npm.mjs assemble --from <dir>` | Build every platform whose `<dir>/<triple>/thepia` exists |

CI assembles and publishes in [`release.yml`](../.github/workflows/release.yml) (the `npm` job): it
dry-runs the publish on every release run and publishes the platform packages first, then the launcher,
when `NPM_TOKEN` is set. A later move to OIDC Trusted Publishing can drop the token (github-policy Phase 7).

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
