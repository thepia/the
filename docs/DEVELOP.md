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
- **Homebrew (`Thepia/tap`):** Installs `bin/thepia` and conditionally symlinks `the` and `th` if those command names are not already present:
  ```ruby
  bin.install "thepia"
  bin.install_symlink "thepia" => "the" unless (HOMEBREW_PREFIX/"bin/the").exist?
  bin.install_symlink "thepia" => "th" unless (HOMEBREW_PREFIX/"bin/th").exist?
  ```
- **Cargo:** Installs all `[[bin]]` targets (`thepia`, `the`, `th`) or a specific binary via `--bin`.

---

## Plan: npm Package Distribution (`thepia`)

To let users install or run `the` via `npm install -g thepia`, `pnpm add -g thepia`, `bunx thepia`, or `npx thepia`, we will publish prebuilt native binaries using the `optionalDependencies` platform-package pattern (no compiler required on user machines, works with `--ignore-scripts`).

### 1. Package Structure
Publish four platform-specific binary packages alongside the main `thepia` package:

| Package | Rust Target | `os` | `cpu` | Contents |
|---|---|---|---|---|
| `@thepia/cli-darwin-arm64` | `aarch64-apple-darwin` | `["darwin"]` | `["arm64"]` | `bin/thepia` |
| `@thepia/cli-darwin-x64` | `x86_64-apple-darwin` | `["darwin"]` | `["x64"]` | `bin/thepia` |
| `@thepia/cli-linux-x64-gnu` | `x86_64-unknown-linux-gnu` | `["linux"]` | `["x64"]` | `bin/thepia` |
| `@thepia/cli-linux-arm64-gnu` | `aarch64-unknown-linux-gnu` | `["linux"]` | `["arm64"]` | `bin/thepia` |
| **`thepia`** | *(meta-package)* | — | — | `bin/the.js`, `README.md`, `optionalDependencies` |

### 2. `package.json` & Binary Entrypoints (`the`, `thepia`, `th`)
The root `thepia` package exposes all three CLI names in its `bin` map:

```json
{
  "name": "thepia",
  "version": "0.1.0",
  "description": "Thepia command line for Linux, macOS, and Nix",
  "license": "MIT OR Apache-2.0",
  "repository": {
    "type": "git",
    "url": "https://github.com/Thepia/the.git"
  },
  "bin": {
    "the": "bin/the.js",
    "thepia": "bin/the.js",
    "th": "bin/the.js"
  },
  "optionalDependencies": {
    "@thepia/cli-darwin-arm64": "0.1.0",
    "@thepia/cli-darwin-x64": "0.1.0",
    "@thepia/cli-linux-x64-gnu": "0.1.0",
    "@thepia/cli-linux-arm64-gnu": "0.1.0"
  }
}
```

### 3. Launcher & Process Name Behaviour
1. **Zero-overhead postinstall link (when lifecycle scripts are enabled):**
   An optional `postinstall` script replaces or links the shim with the resolved native `thepia` binary from the matching `@thepia/cli-<os>-<cpu>` package so subsequent runs invoke the Rust binary directly without starting Node.
2. **Fallback JS shim (`bin/the.js` for `--ignore-scripts` / `npx`):**
   Resolves `@thepia/cli-${platform}-${arch}/bin/thepia` via `require.resolve`, spawns it with `stdio: "inherit"`, and forwards the exit code and signals. Because the child process executes the `bin/thepia` binary, the running CLI process still appears as `thepia` in OS process lists.

### 4. Release Pipeline Steps ([`.github/workflows/release.yml`](../.github/workflows/release.yml))
1. **Stage npm packages (`package` job):**
   - Reuse the 4 native `thepia` binaries built in the `build` matrix job (`aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`).
   - Generate `dist/npm/cli-<os>-<cpu>/package.json` with `"version": "${VERSION}"`, copy the target's `thepia` binary into `bin/thepia` (`chmod 755`), and run `npm pack --dry-run` during the dry-run gate.
   - Generate `dist/npm/thepia/package.json` with matching `optionalDependencies` versions, `bin/the.js`, `README.md`, and `docs/LICENSE-*`.
2. **Smoke-test (`package` job):**
   - Pack tarballs with `npm pack` and test installing them into a temporary prefix (`npm install -g ./thepia-*.tgz`) to verify `the --version`, `thepia --version`, and `th --version`.
3. **Publish to npm (`publish` job):**
   - Authenticate with `NPM_TOKEN` and GitHub Actions OIDC (`--provenance`).
   - Publish all 4 `@thepia/cli-*` platform packages first (`npm publish --provenance --access public`), then publish the root `thepia` package.

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
