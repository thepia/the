# Developing `thepia` (`the`)

This document covers the architecture, CLI design principles, packaging conventions, and development workflow for `thepia`.

---

## Core Principles

### 1. Strict Stream Separation
- **`stdout`** carries **only** the command's primary result. Redirecting `the skills list > skills.txt` or piping `the skills list --format json | jq` is never polluted by warnings or notes.
- **`stderr`** carries all diagnostics (`error`, `warning`, `note`).
- **`--format human|json`** is a single global switch that governs both streams simultaneously:
  - `human` (default): aligned tables/records on `stdout`; `rustc`-styled diagnostics on `stderr`.
  - `json`: a single typed JSON document on `stdout`; one uncolored JSON object per line (JSONL) on `stderr`.

### 2. Terminal vs. Pipe Discipline
 implemented in [`src/style.rs`](../crates/the/src/style.rs) and [`src/table.rs`](../crates/the/src/table.rs):
- **Interactive TTY:** Tables include bold headers, a dim `─` rule, semantic cell tones (`Ok`, `Warn`, `Dim`), and column width truncation to fit the terminal width.
- **Piped or Redirected:** Output remains column-aligned for `awk`/`cut`/`grep`, but strips ANSI escapes, omits the header rule, and never truncates values.
- **Environment Overrides:** Honors `NO_COLOR`, `CLICOLOR=0`, and `CLICOLOR_FORCE` / `FORCE_COLOR` (JSON output is never colored regardless of flags).
- **Broken Pipe Tolerance:** [`output::print`](../crates/the/src/output.rs) treats `ErrorKind::BrokenPipe` (e.g. `the skills list | head -n 5`) as a clean success.

### 3. Render-Independent Command Logic
Commands never format strings or print tables directly. Instead, subcommands construct a declarative [`Report`](../crates/the/src/table.rs) composed of:
- [`Table`](../crates/the/src/table.rs) — columnar data where each column declares whether it appears in `human` output, while `json` output always serializes all columns with full types (`Cell::List` as JSON arrays, `Cell::Empty` as `null`, `Cell::Bool` as booleans).
- [`Record`](../crates/the/src/table.rs) — key-value metadata views (used in `show` commands).
- Body text sections.

This guarantees uniform formatting across every subcommand and keeps rendering logic centralized in [`src/table.rs`](../crates/the/src/table.rs).

### 4. Actionable Diagnostics & Semantic Exit Codes
Every [`Diagnostic`](../crates/the/src/diag.rs) answers three questions under a stable kebab-case `code`:
1. **What** went wrong (`title`)
2. **Why** it happened (`cause` and offending `context` paths/names)
3. **How** to fix it (`help`)

```text
warning[skill-skipped]: opencode and goose skip skill `notes`
  --> ~/.claude/skills/notes/SKILL.md
  = cause: invalid YAML at SKILL.md line 3 column 18: mapping values are not allowed in this context
  = help: fix the YAML; a plain value containing `: ` must be quoted (`description: "..."`) or use a `>-` block
```

Process exit codes are semantic and stable (`Exit` in [`src/diag.rs`](../crates/the/src/diag.rs)):
- `0` — Success
- `1` — Generic runtime error (`Exit::Generic`)
- `2` — Usage / CLI error or ambiguous query (`Exit::Usage`)
- `3` — Requested entity not found (`Exit::NotFound`)

### 5. Minimal, Auditable Dependencies
The workspace avoids heavy dependency trees:
- `clap` (derive) for argument parsing
- `anstyle` (already in `clap`'s tree) for zero-cost terminal styling
- `serde` + `serde_json` for JSON serialization
- `yaml-rust2` for pure-Rust YAML 1.2 frontmatter parsing

### 6. Agent-as-Relay Provenance
`the` is run by short-lived agents that relay its output into a report a human reads. A **read/report
command** (anything whose primary output is information a user would see summarized, e.g. `the skills
list`/`validate`) therefore cites itself as the source, so the user can see where a claim came from and
re-run it:
- **Human (`stderr`):** a relay line addressed to the agent, e.g. `note: if you are an agent, cite the
  command that produced these results when you report them to the user: the skills list --json`.
- **JSON (`stdout`):** a `provenance` array of the exact invocation(s), so the agent relays a value, not
  a reconstruction.

Scope is read/report commands only, never mutating verbs. This is a standing convention shared with the
`thepia` (`cli`) CLI and documented in the workspace `cli-guidance` skill (agent-as-relay, "provenance
relay").

---

## Package & Binary Architecture (`thepia`, `the`, `th`)

The release package is named **`thepia`**, and ships three command names:
- **`the`** — primary CLI command (`#[command(name = "the")]`)
- **`thepia`** — full package binary name
- **`th`** — shorthand command

### Shared Library + Thin Entrypoints
To avoid compiling the crate three times or running unit tests three times:
- [`crates/the/src/lib.rs`](../crates/the/src/lib.rs) contains the full implementation and exports `thepia::run() -> ExitCode`.
- [`crates/the/src/main.rs`](../crates/the/src/main.rs) (`thepia`), [`crates/the/src/bin/the.rs`](../crates/the/src/bin/the.rs) (`the`), and [`crates/the/src/bin/th.rs`](../crates/the/src/bin/th.rs) (`th`) are 3-line entrypoints calling `thepia::run()`.

### Process Name (`thepia`) in Process Lists
When a user invokes `the` or `th`, [`ensure_process_name()`](../crates/the/src/lib.rs) runs at the start of `thepia::run()`:
- **Unix (`macOS` & `Linux`):** If `argv[0]` is not `thepia` and a sibling `thepia` executable exists alongside the current binary, it replaces the current process image in-place via `execv` (`CommandExt::exec`). Because the kernel executes the `thepia` binary path, OS tools (`ps`, `top`, `htop`, macOS Activity Monitor) report the process name as `thepia`.
- **Linux fallback:** Also writes `"thepia\n"` to `/proc/self/comm` so the kernel thread `comm` name is always `thepia` even if invoked standalone.

### Packaging Conventions
- **Nix ([`flake.nix`](../flake.nix)):** `postInstall` links `bin/the -> thepia` and `bin/th -> thepia` so the Nix store path contains a single compiled binary.
- **Homebrew:** `the` does **not** publish a Homebrew formula. On macOS the `thepia` Homebrew identity belongs to the closed `thepia/cli` cask (see "Distribution: Homebrew ownership" below).
- **Cargo:** Installs all `[[bin]]` targets (`thepia`, `the`, `th`) or a specific binary via `--bin`.
- **cdn.thepia.com:** a stable `curl | sh` installer built from `release.json` + `SHA256SUMS`.

---

## Plan: npm Package Distribution (`thepia`)

To let users run `the` via `npx thepia`, `bunx thepia`, or install it with `npm i -g thepia` /
`pnpm add -g thepia` / `bun add -g thepia`, we publish a **single, unscoped, public npmjs.org package**
`thepia` that **bundles the prebuilt native binaries inside the tarball**: a macOS **universal**
(arm64 + x86_64) binary and a Linux **x86_64** binary. A thin launcher execs the right one; an optional
`postinstall` upgrades to direct-exec. No compiler, no runtime download.

> Decided 2026-11-12: **bundle binaries in-package** (not a CDN/postinstall download), **unscoped public
> npmjs.org**, published from CI with an **`NPM_TOKEN`** granular token. `thepia` is unregistered
> (available).

### Why bundled, not downloaded (corporate-gateway reach)

The driving requirement is installing behind corporate egress controls (Sonatype **Nexus** repository
proxies, **Zscaler** SSL-inspection firewalls). Those environments proxy and cache the npm registry
(often through an internal Nexus mirror) but **block or break arbitrary downloads from other hosts**.
So the binary must arrive **as part of the npm tarball** over the registry channel corporate already
allows: no `cdn.thepia.com` fetch, no second TLS endpoint, no postinstall network call to be gated or
firewalled. In-package bytes get the best possible chance through Nexus + Zscaler.

### Bundled target set (minimal, covers the corporate fleet)

| File in package | Rust target(s) | Covers |
|---|---|---|
| `bin/thepia-macos` | `aarch64-apple-darwin` + `x86_64-apple-darwin`, `lipo`-merged **universal** | all Macs (Apple Silicon + Intel) |
| `bin/thepia-linux-x64` | `x86_64-unknown-linux-gnu` | Intel/AMD Linux and WSL2 |

Deliberately minimal (Mac laptops + x86_64 Linux/WSL are the corporate reality). `linux-arm64`, `musl`,
and Windows are **not** bundled; on those the launcher prints a clear build-from-source message
(`cargo install --git https://github.com/Thepia/the thepia`, or the Nix flake). Expand the bundle set
later if a real audience appears; do not add a CDN fallback (it reintroduces the firewall problem).

Size: two stripped+LTO binaries (the macOS one is a fat universal, ~2x). Expect a few tens of MB
unpacked. npm and Nexus handle this; if an internal Nexus enforces a component-size cap, that is the
one knob to check with the platform team.

### 1. Single package `thepia`

`package.json`:

```json
{
  "name": "thepia",
  "version": "0.1.0",
  "description": "Thepia command line for Linux, macOS, and Nix",
  "license": "MIT OR Apache-2.0",
  "repository": { "type": "git", "url": "https://github.com/Thepia/the.git" },
  "bin": { "the": "bin/the.js", "thepia": "bin/the.js", "th": "bin/the.js" },
  "files": ["bin/the.js", "bin/thepia-macos", "bin/thepia-linux-x64", "scripts/postinstall.js", "README.md", "LICENSE-MIT", "LICENSE-APACHE"],
  "scripts": { "postinstall": "node scripts/postinstall.js" }
}
```

The bundled binaries are staged with mode `0755`. No `checksums.json` is needed: the binaries ride
inside the tarball, so npm's own integrity (registry hash / lockfile) already covers them. No
`optionalDependencies`.

### 2. `bin/the.js` launcher (always works, gating-proof, offline)

The `bin` target. It never needs the postinstall and never touches the network:

```js
#!/usr/bin/env node
"use strict";
const path = require("node:path");
const { spawnSync } = require("node:child_process");

function pick() {
  if (process.platform === "darwin") return "thepia-macos";
  if (process.platform === "linux" && process.arch === "x64") return "thepia-linux-x64";
  return null;
}
const name = pick();
if (!name) {
  console.error(
    `thepia: no bundled binary for ${process.platform}-${process.arch}.\n` +
    `  Build from source:  cargo install --git https://github.com/Thepia/the thepia\n` +
    `  or use the Nix flake: nix profile install github:Thepia/the`
  );
  process.exit(1);
}
const bin = path.join(__dirname, name);
// The child IS the native binary, so ps/Activity Monitor show `thepia`.
const res = spawnSync(bin, process.argv.slice(2), { stdio: "inherit" });
if (res.error) { console.error(String(res.error.message || res.error)); process.exit(1); }
process.exit(res.status === null ? 1 : res.status);
```

### 3. `scripts/postinstall.js` (optional optimization only)

Kept, but **not load-bearing** (pnpm v10 and bun gate dependency build scripts; `--ignore-scripts`
disables them). When it does run, it swaps the Node wrapper for the native binary so subsequent calls
skip Node startup: copy the selected `bin/thepia-<platform>` over `bin/the.js` (the path the `.bin`
symlinks already point at), preserving `0755`. On an unsupported platform or any error it is a no-op
and the launcher handles the run. Because the binaries are already present, this step does **no network
and no selection that the launcher can't also do** - it is purely a startup-latency win. Never put
required behavior here.

### 4. Release-pipeline steps ([`.github/workflows/release.yml`](../.github/workflows/release.yml))

1. **Build universal macOS binary (`package` job):** `lipo -create` the `aarch64-apple-darwin` and
   `x86_64-apple-darwin` `thepia` binaries (both already built in the `build` matrix) into one fat
   `thepia-macos`; **Developer ID sign** (ideally notarize) it so Gatekeeper accepts it
   (`release-signing`). Take `x86_64-unknown-linux-gnu`'s `thepia` as `thepia-linux-x64`.
2. **Assemble `dist/npm/thepia/`:** `package.json` (version = `${VERSION}`), `bin/the.js`,
   `bin/thepia-macos` + `bin/thepia-linux-x64` (`chmod 755`), `scripts/postinstall.js`, `README.md`,
   `docs/LICENSE-*`.
3. **Dry-run smoke test (always, offline):** `npm pack`; install the tarball into a temp global prefix
   with **`--ignore-scripts`** (to prove the launcher path works without postinstall) and assert
   `the --version` / `thepia --version` / `th --version`; then install **with** scripts and assert the
   direct-exec path still works. No network in either case.
4. **Publish (`publish` job, only when `dry_run == 'false'`):** `actions/setup-node@v4` with
   `registry-url: https://registry.npmjs.org`, `env NODE_AUTH_TOKEN: ${{ secrets.NPM_TOKEN }}`, then
   `npm publish dist/npm/thepia --access public`. One package, one publish.

**Secret:** `gh secret set NPM_TOKEN -R thepia/the` (granular token scoped to publish `thepia`; record
expiry in `cli/docs/build-network-credentials.md`). First publish claims the name; a later switch to
OIDC Trusted Publishing (github-policy Phase 7) can drop the token once the name exists.

### 5. Consistency notes

- The npm `thepia` is the **OSS CLI binary** (same artifact as `the`/`th` and the `curl | sh`
  installer), never the closed `.app`. npm / Nix / cargo / `curl | sh` are the open-CLI channels; the
  macOS full product stays the signed `.app` cask (Homebrew decision A).
- Signing the universal macOS binary serves both the npm bundle and the `curl | sh` channel (same
  bare-binary Developer ID signing).
- If airgapped installs need `linux-arm64`/`musl`/Windows too, add those as additional bundled files
  (bigger tarball) rather than a download - keep everything in-package for gateway reach.

---

## Plan: Compacting the `AGENTS` Column (`*`, Abbreviations, and `…`)

As more coding assistants are supported (`claude`, `gemini`, `opencode`, `goose`, etc.), a comma-separated list in the `AGENTS` column can grow to 31+ characters (`claude, gemini, opencode, goose`), squeezing the `DESCRIPTION` column on 80–100 column terminals.

While `--format json` must always emit the full array of full agent names (`["claude", "gemini", "opencode"]`), human table rendering can shorten the `AGENTS` column using three complementary techniques:

### 1. Star (`*`) for "All Installed Assistants"
When a skill is loaded by **every** installed assistant on the machine (with ≥ 2 assistants installed):
- Render `*` (styled with `Tone::Ok`) instead of listing every assistant name.
- Since most portable skills in `~/.agents/skills` or `~/.claude/skills` load across all installed assistants, the common case shrinks from `claude, gemini, opencode` (24 chars) to `*` (1 char).
- Partial coverage remains immediately scannable because any row *not* showing `*` stands out visually.

### 2. Two-Letter Agent Abbreviations
Give each [`Agent`](../crates/the/src/skills/mod.rs) a canonical 2-character short code (`Agent::short_str()`):

| Agent | Full Name | Short Code |
|---|---|---|
| Claude Code | `claude` | `cl` |
| Gemini CLI | `gemini` | `gm` |
| OpenCode | `opencode` | `oc` |
| Goose | `goose` | `gs` |

- **Single agent:** fits in 6–8 chars (`claude`, `gemini`, `opencode`, `goose`) — already near the 6-char `AGENTS` header width, so it can stay full or use `cl`/`gm`/`oc`/`gs`.
- **Multiple agents (subset of installed):** joining short codes (`cl, oc` = 6 chars; `cl, gm, oc` = 10 chars) keeps the `AGENTS` column compact without losing which specific subset loaded the skill.

### 3. Item-Boundary Ellipsis (`…`) on Terminal Width Overflow
- [`Column::max_width`](../crates/the/src/table.rs) already truncates cells with a trailing `…` on interactive terminals (`h.term.tty == true`) while keeping piped output (`the skills list | ...`) untruncated.
- For `Cell::List`, enhance `table::fit()` (or list rendering) to truncate at **item boundaries** (`claude, gemini, …` or `cl, gm, …`) rather than slicing an agent name mid-word (`claude, gemi…`).

### 4. Implementation Steps in [`table.rs`](../crates/the/src/table.rs) and [`skills/cmd.rs`](../crates/the/src/skills/cmd.rs)
1. **Add `Agent::short_str(self) -> &'static str`** in [`crates/the/src/skills/mod.rs`](../crates/the/src/skills/mod.rs).
2. **Decouple human display from JSON array in `Cell`:**
   Add `Cell::DisplayList { human: String, items: Vec<String>, tone: Option<Tone> }` (or a helper constructor on `Cell`) in [`crates/the/src/table.rs`](../crates/the/src/table.rs) so `Cell::human()` renders the compact representation (`*`, `cl, oc`, or `claude, …`) while `Cell::json()` still serializes `Value::Array` of the full canonical strings (`["claude", "opencode"]`).
3. **Format rule in `skills/cmd.rs` (`format_agents_cell`):**
   - `loaded.is_empty()` → `Cell::Empty` (`-` dimmed in human, `[]` in JSON)
   - `loaded.len() == installed.len() && installed.len() > 1` → `*` (with `Tone::Ok`)
   - `loaded.len() == 1` → full name (`claude`, `gemini`, `opencode`, `goose`), which is at most 8 chars
   - `loaded.len() >= 2` (subset) → short codes (`cl, oc`, `cl, gm, oc`) capped with `Column::new("agents", "AGENTS").max_width(12)` using `…` on TTY.

---

## Codebase Layout

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

### Adding a New Subcommand
1. Define the subcommand variant and arguments in [`crates/the/src/cli.rs`](../crates/the/src/cli.rs) under `enum Command`.
2. Create a module `crates/the/src/<subcommand>/` that builds a [`table::Report`](../crates/the/src/table.rs) and emits any [`Diagnostic`](../crates/the/src/diag.rs)s via `diag.emit(format)`.
3. Wire the variant in [`crates/the/src/lib.rs`](../crates/the/src/lib.rs).
4. Add hermetic integration tests in `crates/the/tests/` verifying both human and `--format json` outputs, diagnostics on `stderr`, and exit codes.

### Reserved `the skills` Subcommands (`add`, `update`, `remove`) and Scope Boundary
- **`the skills add`**, **`the skills update`**, and **`the skills remove`** are **reserved subcommand names**.
- We do **not** plan to implement them by delegating to `skills.sh` (`npx skills`). `skills.sh` is a third-party GitHub/registry installer and lockfile manager (`skills-lock.json`), whereas `the` is built for **refining, verifying, and organizing self-authored AI abilities** across `~/.agents` and project workspaces.
- For how `the skills lint` and `the skills audit` check quality, security, and token budgets (both natively and via targeted delegation to `LichAmnesia/skill-lint` and `@haasstefan/skills-lint`), see [`docs/SKILLS-LINT.md`](SKILLS-LINT.md).

---

## Development & Verification

### Toolchain
[`rust-toolchain.toml`](../rust-toolchain.toml) pins the Rust compiler version. Run `nix develop` (or use `rustup`) to enter the pinned environment with `just` and `lychee`.

### Common `just` Recipes
Run `just` (or `just --list`) to see all available tasks in [`justfile`](../justfile):

| Command | Description |
|---|---|
| `just test-unit` | Fast unit tests (`--lib --bins`), also run by the `.githooks/pre-commit` hook |
| `just test` | Full unit + integration test suite |
| `just clippy` | Clippy across workspace and all targets with `-D warnings` |
| `just fmt` / `just fmt-check` | Format or verify formatting with `rustfmt` |
| `just links` | Offline markdown link check via `lychee` |
| `just gate` | Everything CI checks on Rust: `fmt-check` + `clippy` + `test` |
| `just nix-check` | Build the Nix package and run flake checks (`fmt`, `clippy`, `thepia`) |
| `just eval-skills` | Run the agent-skills resolution oracle self-test and live agent verification |
| `just release-dry-run` | Run `gate`, build release binary, and check `cargo package --list -p thepia` |
| `just install-hooks` | Point `git config core.hooksPath` to `.githooks` |

## Repo Health: CI, Badges & Security

The README badge row must only ever show **truthful** badges: never add a badge before the thing it
reports exists (no decorative crates.io/coverage/security badges for machinery we have not wired).

### Present (honest today)
- **CI** -> [`ci.yml`](../.github/workflows/ci.yml) status on `main` (Rust fmt/clippy/test on
  ubuntu+macOS, Nix flake, doc links, skill-eval).
- **License** `MIT OR Apache-2.0`, **MSRV** `rustc 1.88`, **Platform** macOS/Linux, **Nix flake**.
- **Security policy** -> [`SECURITY.md`](SECURITY.md) (GitHub recognizes it in `docs/`).

### To add, each gated on wiring its backing first
| Badge | Needs | Notes |
|---|---|---|
| **Dependabot** | a `.github/dependabot.yml` (`cargo` + `github-actions`, grouped, weekly) | Standing Thepia dependabot policy; pure config, zero CI risk. **Next quick win.** |
| **CodeQL** | a `codeql.yml` workflow | Free on a public repo; confirm GitHub CodeQL's Rust support runs green before adding the badge (do not add a red badge). github-policy Phase 8. |
| **OpenSSF Scorecard** | `scorecard.yml` + `id-token: write` | Canonical security-health badge (`api.securityscorecards.dev`); badge appears after the first scheduled run. |
| **OpenSSF Best Practices (CII)** | manual enrollment at bestpractices.dev | Manual, optional. |
| **crates.io version + docs.rs** | `cargo publish` to crates.io | Only once published; today install is `cargo install --git`, so neither badge is truthful yet. |
| **Latest release** (`github/v/release`) | a cut GitHub Release (the [release workflow](../.github/workflows/release.yml) does this) | Add once `v0.1.0` is released; before that it renders "no releases". |
| **Codecov** | a coverage job (e.g. `cargo-llvm-cov`) uploading to Codecov | Optional; adds a CI dependency. |

Order of adoption (free + highest-signal first): `dependabot.yml` -> `codeql.yml` -> Scorecard, then
crates.io/docs.rs + the release badge once the crate is actually published and released.

## Distribution: Homebrew ownership (`the` vs `cli`) — decision A

The public `the` CLI and the closed `thepia/cli` product both install a binary named `thepia`, so
they cannot both be the Homebrew `thepia`: one tap holds only one package named `thepia`, and only
one installed formula/cask can own `bin/thepia`.

**Decision (A):** on macOS the `thepia` Homebrew identity is owned by the **closed `thepia/cli`**,
shipped as a **cask** (it must be a signed/notarized `.app` to carry the iCloud-keychain provisioning
profile anyway; a bare brew binary can't). `the` therefore does **not** publish a Homebrew formula.

`the` ships via **Nix** (`nix run`/`nix profile install`), **cargo** (`cargo install --git`), and the
**cdn.thepia.com `curl | sh`** installer built from `release.json` + `SHA256SUMS`. The Homebrew
formula generation and the `Thepia/homebrew-tap` push were removed from
[`release.yml`](../.github/workflows/release.yml) accordingly, and `the` needs no `HOMEBREW_TAP_TOKEN`.

If an OSS-CLI-on-brew audience ever appears, publish it under the distinct name `the` installing only
`the`/`th` (never `thepia`), to avoid the binary clash. Ties to the two-bundle distribution and the
shared-subcommand pattern (cli backlog).
