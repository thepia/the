---
name: skills-pkgs
description: >
  Code-verified reference and evaluation of npm packages and CLI tools for linting, checking,
  quality-scoring, security-scanning, and managing Agent Skills (SKILL.md), including skill-check,
  skill-lint, @elonmust1/skillcheck, @swarmclawai/agent-skills-lint, @effectorhq/skill-lint,
  @haasstefan/skills-lint, skills, skillpm, and skills-ref. Use when evaluating, comparing, or
  choosing third-party skill linters; designing lint, auto-fix, quality-score, or security-scan
  features in `the skills`; or checking what failures existing npm packages catch, how they parse
  and score SKILL.md files, what they auto-fix, and where their implementations break.
---

# Agent Skills npm Packages: Implementation and Evaluation

This skill records what the npm ecosystem packages for `SKILL.md` linting, verification, quality
scoring, security scanning, and package management actually do in their source code, how they
implement each check or auto-fix, and how reliable they are in practice.

## Package Landscape and the `skill-lint` Naming Collision

Before running or recommending any package, note a critical naming collision on npm:

- **`skill-lint` on npm (`skill-lint@0.2.0`, repo `LichAmnesia/skill-lint`)** is a **security and
  toxic-pattern scanner** (`R01` through `R10`: prompt injection, base64 RCE, credential exfiltration,
  persistence tampering). Running `npx skill-lint` downloads this security scanner.
- **`himself65/skill-lint` on GitHub** is a **spec and Claude.ai/Claude Code frontmatter validator**
  primarily used as a GitHub Action (`himself65/skill-lint@v3`). Its README instructs users to run
  `npx skill-lint`, which actually executes `LichAmnesia/skill-lint` from the npm registry.

| Package | Version Inspected | Primary Role | Parser Used | Has Auto-Fix? | Overall Grade |
|---|---|---|---|---|---|
| `skill-check` | `1.2.0` | Spec linter, link checker, quality scorer, body splitter | `yaml` (`2.8.2`) + regex | Yes (`--fix`, `split-body`) | Good linter; flawed `--fix` |
| `skill-lint` (`LichAmnesia`) | `0.2.0` | Static security and supply-chain scanner (OWASP AST01-04) | `yaml` (`2.6.0`) + regex + taint | No | Strong static security scanner |
| `@elonmust1/skillcheck` | `0.1.0` | Zero-dep trigger-quality and Jaccard overlap linter | Hand-rolled line regex | No | Good trigger heuristics; fragile YAML parser |
| `@swarmclawai/agent-skills-lint` | `0.1.0` | Multi-agent flavor validator, installer, index generator | `gray-matter` + `zod` | No | Broken directory discovery (`*.md`) |
| `@effectorhq/skill-lint` | `0.2.0` | OpenClaw / ClawHub ecosystem `SKILL.md` validator | `@effectorhq/core` | No | Niche (OpenClaw-specific) |
| `@haasstefan/skills-lint` | `0.1.2` | Per-model BPE token budget linter (native binary wrapper) | Native binary (`o200k_base`, `cl100k_base`) | No | Good for strict token budgets |
| `skills` (Vercel Labs) | `1.8.1` | Multi-agent skill installer, lockfile manager (`skills-lock.json`) | `yaml` + SHA-256 tree hash | No | Standard installer (`skills.sh`) |
| `skillpm` | `2.0.0` | `node_modules` skill scanner and npm publish wrapper | `gray-matter`, shells out to `skills-ref` | No | Thin wrapper around `npm` and `skills-ref` |

---

## 1. `skill-check` (`1.2.0`, `thedaviddias/skill-check`)

### How It Works (`dist/core/`, `dist/rules/core/`)
- **Discovery (`discovery.js`):** Uses `fast-glob` with default include `['**/SKILL.md']` and ignores
  `node_modules`, `.git`, `dist`, `build`, `.next`, `coverage`, `fixtures`.
- **Frontmatter Parsing (`frontmatter.js`):** Matches `/^---\r?\n([\s\S]*?)\r?\n---(?:\r?\n|$)/` and
  parses with `yaml` (`YAML.parse`). Rejects non-objects or arrays.

### What Failures It Catches and How
1. **Frontmatter (`rules/core/frontmatter.js`):**
   - `frontmatter.required` (`error`): missing or unparsable YAML block.
   - `frontmatter.name_required` (`error`), `frontmatter.description_required` (`error`).
   - `frontmatter.name_matches_directory` (`error`): compares normalized `name` against `path.basename(path.dirname(filePath))`.
   - `frontmatter.name_slug_format` (`error`): regex `/^[a-z0-9]+(?:-[a-z0-9]+)*$/` (properly rejects leading, trailing, and consecutive hyphens).
   - `frontmatter.name_max_length` (`error`): default 64 chars.
   - `frontmatter.field_order` (`error`): checks `frontmatterRaw.indexOf('name:') < frontmatterRaw.indexOf('description:')`. **Flaw:** uses raw substring `indexOf` on unparsed YAML text, so a comment or nested key containing `description:` above `name:` triggers a false error.
   - `frontmatter.unknown_fields` (`warn`): allows only `name`, `description`, `license`, `compatibility`, `metadata`, `allowed-tools`.
   - `frontmatter.compatibility_max_length` (`warn`): max 500 chars.
   - `frontmatter.metadata_string_values` (`warn`): requires `metadata` to be a flat map where every value has `typeof val === 'string'` (flags integers like `version: 2`).
   - `frontmatter.allowed_tools_format` (`warn`): requires `typeof value === 'string'` (flags YAML arrays).
2. **Description (`rules/core/description.js`):**
   - `description.non_empty` (`error`), `description.max_length` (`error`, default 1024).
   - `description.use_when_phrase` (`warn`): regex `/\buse\s+when\b/i`.
   - `description.min_recommended_length` (`warn`): default 50 chars.
3. **Body (`rules/core/body.js`):**
   - `body.max_lines` (`error`): default 500 lines (`skill.body.split(/\r?\n/).length`).
   - `body.max_tokens` (`warn`): default 5000 tokens. **Flaw:** `estimateTokens` is just `text.split(/\s+/).filter(Boolean).length` (word count, not actual tokenization; code blocks and URLs undercount heavily).
4. **Links (`rules/core/links.js`, `core/links.js`):**
   - `links.local_markdown_resolves` (`warn`) and `links.references_resolve` (`warn`): tracks fenced code blocks (` ``` ` and `~~~`) so links inside code blocks are ignored, strips query/fragment (`#`, `?`) and markdown link titles, skips `/`, `#`, `{...}`, and external schemes, and checks `fs.existsSync(path.resolve(dirname(filePath), cleanTarget))`.
5. **File & Cross-Skill Duplicates (`rules/core/file.js`, `core/duplicates.js`):**
   - `file.trailing_newline_single` (`warn`): checks `!content.endsWith('\n') || content.endsWith('\n\n')`.
   - `duplicates.name` (`warn`): exact match on `frontmatter.name`.
   - `duplicates.description` (`warn`): exact match on lowercase trimmed description (when length >= 20).
6. **External Security Scan (`core/agent-scan.js`):**
   - Does not implement security rules itself; spawns `mcp-scan --skills <path>` (via `local`, `uvx`, or `pipx`) and parses `[A-Z]\d{3}` finding codes from stdout.

### Quality Score Formula (`core/quality-score.js`)
Weights total 100 points: `frontmatter` (30), `description` (30), `body` (20), `links` (10), `file` (10).
For each diagnostic in category `c`, `penalties[c] += (severity === 'error' ? 1.0 : 0.5)`, and
`categoryScore = Math.max(0, weight - penalties[c] * weight)`.
**Flaw:** A single `error` (or two `warn` findings) in a category wipes out 100% of that category's points.

### What It Can Fix and Why Some Fixes Are Harmful (`core/fix.js`, `core/body-split.js`)
- **Safe fixes in `check --fix`:**
  - `frontmatter.name_required`, `frontmatter.name_matches_directory`, `frontmatter.name_slug_format`: sets `frontmatter.name = skill.slug` (directory name).
  - `frontmatter.field_order`: re-serializes frontmatter with `name` first, then `description`, then remaining keys via `YAML.stringify` (note: destroys YAML comments and custom scalar formatting like `>-`).
  - `file.trailing_newline_single`: normalizes EOF to a single `\n` or `\r\n`.
- **Harmful / synthetic fixes in `check --fix` (avoid in production):**
  - `frontmatter.description_required`: injects generic boilerplate (`"Use when <slug> should be selected for a task based on explicit user intent and clear context."`).
  - `description.use_when_phrase`: blindly prepends `"Use when "` and lowercases the first character (`Use when ${first.toLowerCase()}${trimmed.slice(1)}`), producing broken grammar when descriptions start with an acronym, proper noun, or verb.
  - `description.min_recommended_length`: runs a `while (value.length < minLength)` loop that repeatedly appends `" Include clear triggers, constraints, and expected outcomes."` to silence the length warning without adding any real trigger signal.
- **Body splitting (`split-body --write` in `core/body-split.js`):**
  - Splits `skill.body` on every `/^##\s+(.+?)\s*$/` line into `references/<slug>.md` and replaces the body with the intro plus a `## References` bullet list.
  - **Flaw:** `parseSections` does not skip fenced code blocks, so a `#` or `## comment` inside a bash/python code block is treated as a markdown H2 heading and slices the code block in half.

---

## 2. `skill-lint` (`0.2.0`, `LichAmnesia/skill-lint`)

### How It Works (`src/scanner.js`, `src/classify.js`, `src/rules/R01..R10.js`)
A static security linter mapped to OWASP Agentic Security Top 10 (`AST01` to `AST04`).
- **Role-Aware Classification (`classify.js`):** Classifies every file in a skill repo as `skill` (`SKILL.md`), `skill-script` (executable/code inside a skill dir), `skill-doc` (other `.md` inside a skill dir), `readme` (`README.md`, `CHANGELOG.md`, `LICENSE`), `doc` (`docs/**`), or `repo-infra`.
- **Prose vs. Code-Fence Splitting:** For `skill`, `skill-script`, and `skill-doc`, scans the full text (because agents follow prose instructions in `SKILL.md`). For `readme` and `doc`, scans only fenced code blocks (`splitMarkdown(text).fences`) and downgrades severities (`CRITICAL -> MEDIUM`, `HIGH -> LOW`, `MEDIUM/LOW -> skipped`) unless `_allowInReadme: true` is set (used for prompt injections and leaked API keys).
- **Shell Normalization:** Collapses backslash-newline continuations (`collapseLineContinuations`) before matching so multi-line `curl ... \ | bash` commands cannot evade regexes.

### What Failures It Catches (`src/rules/`)
- **`R01` Prompt Injection (`CRITICAL`):** 14 regexes for `"ignore previous instructions"`, `"you are now in developer/god/dan mode"`, `"do not tell the user"`, fake chat-role tags (`<system>`, `[INST]`, `<|im_start|>`), and `"reveal your system prompt"`. Avoids matching TypeScript generics like `Promise<User>` by requiring a non-alphanumeric boundary before `<`.
- **`R02` Obfuscation (`CRITICAL` to `MEDIUM`):**
  - Zero-width/bidi characters (`\u200B-\u200F`, `\u202A-\u202E`, `\u2060-\u206F`, `\uFEFF`) in full text (`HIGH`). Note: flags UTF-8 BOM `\uFEFF`.
  - Cyrillic homoglyphs mixed into ASCII-dominant files (`MEDIUM`).
  - `base64 -d | sh` (or `xxd -r -p`, `openssl -base64 -d`) piped to any interpreter (`CRITICAL`), or `eval "$(... base64 -d)"` command substitution (`CRITICAL`).
- **`R03` Shell Danger (`CRITICAL` to `LOW`):** `curl | sh`, `wget -O - | sh`, `bash <(curl ...)`, download-then-source (`curl -o /tmp/x.sh && sh /tmp/x.sh`), `/dev/tcp` and `nc -e` reverse shells, `python -c "import..."`, `chmod 777`. Downgrades `curl | sh` from `CRITICAL` to `LOW` only when the URL host is in a curated installer allowlist (`astral.sh`, `brew.sh`, `sh.rustup.rs`, `bun.sh`, `deno.land`, etc., intentionally excluding `raw.githubusercontent.com`).
- **`R04` Credential Exfiltration (`CRITICAL` to `HIGH`):**
  - Secret env vars (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `AWS_SECRET_ACCESS_KEY`, `GITHUB_TOKEN`, etc.) interpolated into URLs or `curl -H Authorization` headers.
  - Access to `~/.aws`, `~/.ssh`, `~/.config/gcloud`, `~/.kube`, `~/.netrc`, `~/.gnupg`, `~/.claude/credentials`.
  - **Provider-Host Co-occurrence Check:** If Python/JS code reads a secret env var (e.g. `process.env.ANTHROPIC_API_KEY`) and calls an HTTP client (`fetch`, `requests`, `subprocess`), it checks all URLs in the file against `PROVIDER_HOSTS`. It only flags `CRITICAL` if the file contacts a non-provider host or shells out without a static provider URL.
  - Hardcoded key literals (`sk-ant-...`, `AKIA...`, `ghp_...`, `-----BEGIN PRIVATE KEY-----`).
- **`R05` External Fetch (`CRITICAL` to `MEDIUM`):** Password-protected zip extraction (`unzip -P`, scanner evasion), dynamic `exec(await fetch(...))`, raw IP URLs, and runtime fetches from non-trusted hosts.
- **`R06` Suspicious Binaries (`HIGH` to `LOW`):** Flags compiled binaries (`.exe`, `.dll`, `.so`, `.dylib`, `.pyc`, `.class`) and opaque archives (`.zip`, `.7z`, `.tar.gz`) inside skill directories.
- **`R07` Persistence & Agent-State Tampering (`CRITICAL` to `MEDIUM`):**
  - Detects writes (`>`, `>>`, `tee`, `fs.writeFile`, `open(..., 'w')`) targeting `~/.claude/CLAUDE.md`, `~/.claude/settings.json`, `~/.claude/hooks`, `MEMORY.md`, `SOUL.md`, shell RC files (`.bashrc`, `.zshrc`), `crontab`/`launchctl`/`systemctl`, and `.ssh/authorized_keys`.
  - **3-Hop Variable Taint Analysis:** Tracks shell variable assignments (`A=~; B=$A/.claude/CLAUDE.md; echo x >> $B`) across up to 3 hops to catch variable-indirection writes (`CRITICAL`), plus proximity windowing (`+-240` chars) around target path mentions.
- **`R08` Destructive Ops (`CRITICAL` to `MEDIUM`):** `rm -rf /`, `rm -rf ~`, `rm -rf *` (while allowing targeted subpath cleanup like `rm -rf ~/.claude/skills/foo`), `mkfs`, `dd if=/dev/zero`, fork bombs, `git reset --hard`, `git push --force`, disabling `ufw`/`firewalld`/`selinux`, `DROP DATABASE`.
- **`R09` Metadata Abuse (`HIGH` to `LOW`):** Claims of `anthropic-official` / `verified-by-anthropic` when repo is not Anthropic's, and overbroad triggers (`"whenever the user asks anything"`, `"on every message"`, `"always activate"`).
- **`R10` Over-Privilege (`CRITICAL` to `MEDIUM`):** `--dangerously-skip-permissions`, `"disable permission prompts"`, and unrestricted `allowed-tools: Bash(*)` or `Write(*)`.

---

## 3. `@elonmust1/skillcheck` (`0.1.0`)

### How It Works (`bin/skillcheck.js`)
A single-file (274-line), zero-dependency ESM script focused on **trigger quality** and **cross-skill description overlap**.

### What It Catches and How
- **Trigger Quality Heuristics (its strongest feature):**
  - `trigger/no-when` (`warn`): checks 6 regexes (`TRIGGER_SIGNALS`) for trigger cues (`use when/whenever/if/for/to`, `trigger(s|ed|ing)`, `when the user/someone/you`, `for creating|reading|...`, `invoke/activate/apply when`).
  - `trigger/generic-opener` (`warn`): flags descriptions starting with filler (`/^a skill (for|that|to)\b/i`, `/^use this skill to (help|assist)\b/i`, `/^helps? (you )?(with|to)\b/i`, `/^this skill\b/i`).
  - `trigger/low-specificity` (`warn`): tokenizes description into lowercase words (> 2 chars), removes 43 English/skill stopwords (`use`, `skill`, `help`, `task`, `user`, etc.), and warns if fewer than **4 distinctive tokens** remain.
- **Cross-Skill Semantic Collision via Jaccard Similarity (`detectCollisions`):**
  - Computes Jaccard similarity $|A \cap B| / |A \cup B|$ between the non-stopword token sets of every pair of skills.
  - Emits `collision/overlap` (`warn`) when **Jaccard similarity >= 0.45 (45%)**, warning that the model may struggle to pick between the two skills.
- **Score Formula:** `Math.max(0, 100 - (34 * errors + 12 * warnings + 3 * infos))`.

### Implementation Flaws
- **Hand-rolled regex YAML parser (`parseFrontmatter`):** Splits lines and matches `/^([A-Za-z0-9_-]+)\s*:\s*(.*)$/`. It does **not** strip YAML block scalar indicators (`>` or `|` or `>-`), so `description: >-` sets `data.description` to `">- actual text..."`, which then fails its `/^[A-Z]/` capitalization check (`description/style`) and pollutes the token set with `">-"`!

---

## 4. `@swarmclawai/agent-skills-lint` (`0.1.0`)

### How It Works (`dist/chunk-2XYTO6Z5.js`, `dist/cli.js`)
Uses `gray-matter` and `zod` (`skillFrontmatterSchema`) with an agent `FLAVORS` table (`claude`, `codex`, `opencode`, `aider`, `copilot`, `gemini`, `cursor`, `claw`, `droid`, `trae`, `hermes`, `kiro`, `antigravity`, `generic`), plus `install`, `index`, and `collisions` subcommands and a `help-agents` JSON catalog.

### Serious Implementation Bugs Found in Source
1. **Scans ALL `.md` files, not just `SKILL.md` (`findSkillFiles`):**
   `entry.name.toLowerCase().endsWith(wantExt)` (where `wantExt` is `".md"`) recursively loads **every Markdown file** in a directory (`README.md`, `CHANGELOG.md`, `references/*.md`, `docs/*.md`) as if each were a standalone skill, emitting `load/schema` errors (`name: Required`) on every reference doc.
2. **Broken Name Regex (`SKILL_NAME_PATTERN`):**
   Uses `/^[a-z][a-z0-9-]*[a-z0-9]$/`, which rejects 1-character names (`a`), rejects names starting with a digit (`3d-tools`), and **allows consecutive hyphens** (`a--b`) in violation of `agentskills.io`.
3. **Inaccurate Agent Flavors (`FLAVORS`):**
   - Hardcodes `antigravity` user install root as `~/.agent/skills` (actual global root is `~/.gemini/config/skills/`).
   - Restricts `claude` known frontmatter keys to only `["name", "description"]`, flagging standard fields (`license`, `compatibility`, `metadata`, `allowed-tools`, `hooks`, `model`) as `unknown-frontmatter-key`.

---

## 5. Other Packages Inspected

### `@effectorhq/skill-lint` (`0.2.0`)
- Built specifically for publishing to **ClawHub / OpenClaw**, not general `agentskills.io` repos.
- Warns if `description` is over **200 characters** (whereas `agentskills.io` allows 1024), warns if `metadata.openclaw.emoji` is missing (`missing-emoji`), and warns if the Markdown body lacks literal `## Purpose`, `## When to Use`, and `## Setup` headings (`missing-sections`).

### `@haasstefan/skills-lint` (`0.1.2`)
- Thin Node launcher (`bin/skills-lint`) that invokes a platform-specific native binary (`@haasstefan/skills-lint-darwin-arm64`, etc.).
- Uses real BPE tokenizers (`o200k_base` for `gpt-4o`/`gpt-5`, `cl100k_base` for `gpt-4`) configured via `.skills-lint.config.json`, caches counts in `.skills-lint-cache/tokens.json`, and exits `1` on error threshold or `2` on warning threshold.

### `skills` (`1.8.1`, Vercel Labs `npx skills`)
- The canonical multi-agent package manager (`skills.sh`), supporting 50+ agent targets.
- Distinguishes **universal agents** (whose project dir is `.agents/skills`) from vendor-specific agents.
- Validates `SKILL.md` in `parseSkillMd`: requires `name` and `description` to be non-empty strings after stripping terminal escapes (`stripTerminalEscapes`), skips skills with `metadata.internal: true`, checks symlink containment (`SKILLS_SYMLINK_OUTSIDE_SKILL`, `SKILLS_SYMLINK_CYCLE`), and maintains `skills-lock.json` v2 using deterministic SHA-256 folder hashes (`computeSkillFolderHash`, normalizing `\r\n` to `\n` and excluding `.git`, `__pycache__`, `metadata.json`).

### `skillpm` (`2.0.0`) and `skills-ref` (`agentskills/agentskills`)
- `skillpm` scans `node_modules` (including workspace symlinks) for packages containing `skills/<name>/SKILL.md` and syncs them via `npx skills`.
- For validation (`skillpm publish`), it shells out to the Python reference validator **`skills-ref validate <dir>`** from `agentskills/agentskills`, which strictly checks the 6 spec fields (`name`, `description`, `license`, `compatibility`, `metadata`, `allowed-tools`), Unicode NFKC directory-name matching, and `name` format (`1-64` chars, lowercase alphanumeric + single hyphens).

---

## Summary: Delegate vs. Native in `the skills`

`the` prioritizes minimal, auditable Rust dependencies (`clap`, `anstyle`, `serde`, `yaml-rust2`) and
fast offline execution. See [docs/skills/SKILLS-LINT.md](../../../docs/skills/SKILLS-LINT.md) for the complete
design and roadmap.

### 1. Out of Scope (Reserved, Not Delegated)
1. **Remote Skill Package Management (`the skills add`, `update`, `remove`):**
   - **Why reserved and not delegated to `skills.sh` (`npx skills`):** While `skills.sh` handles
     remote acquisition, `skills-lock.json` v2 hashing, and 50+ agent fan-out installation, `the` is
     focused on authoring, inspecting, testing, and refining self-authored AI abilities rather than
     acting as a consumer wrapper around third-party skill registries. These subcommands are reserved
     in `docs/DEVELOP.md`.

### 2. Capabilities to Delegate (Call the Canonical Tool)
1. **Deep Security, Prompt-Injection, and Toxic-Flow Auditing (`the skills audit` -> `npx skill-lint --json`):**
   - **Why delegate:** Security scanning is a signature and threat-intelligence problem (`R01`-`R10`,
     role-aware fence splitting, 3-hop shell variable taint tracking, provider-host allowlisting)
     that evolves rapidly. `the skills lint` runs a fast in-process smoke test (`skill-smoke`), while
     `the skills audit` delegates deep static analysis to `LichAmnesia/skill-lint@0.2.0` (`--json`)
     and maps its findings into `the`'s `Diagnostic` stream.
2. **Phase 1 Exact Multi-Model BPE Token Counting (`npx @haasstefan/skills-lint` -> Bundled Small Models):**
   - **Why delegate initially, then replace:** Before `the` bundles local small models, exact BPE
     token counting (`o200k_base`, `cl100k_base`, `claude`) is delegated via `the skills lint --tokens`
     to `@haasstefan/skills-lint`. Once `the` ships its bundled small-model runtime and native
     tokenizer, `the` will replace this delegation with native 3-tier progressive-disclosure token
     budgeting (Tier 1 catalog index, Tier 2 `SKILL.md` body, Tier 3 linked `references/*.md`) and
     semantic routing evaluation.

### 3. Capabilities to Implement Natively in `the` (Borrow the Idea, Not the Package)
1. **Per-Agent Discovery, Precedence, Shadowing, and Conflict Resolution (Already in `the`):**
   - None of the npm packages model real agent parent walks, shadowing vs. OpenCode concurrent
     conflicts, or lenient vs. strict per-agent loaders (`crates/the/src/skills/`).
2. **Full `agentskills.io` Frontmatter Spec Lint (Extend `crates/the/src/skills/parse.rs`):**
   - Adding optional-field checks (`compatibility` <= 500 chars, `metadata` string map,
     `allowed-tools`, unknown top-level keys) to `parse.rs` takes ~30 lines of Rust, removing any
     need to shell out to Python `skills-ref validate` or `skill-check`.
3. **Fence-Aware Local Link Resolution (Borrow from `skill-check` `core/links.js`):**
   - Only ~90 lines of logic (`std::fs` + fence state machine, zero crate dependencies). Directly
     catches broken `./scripts/...` and `./references/...` links in `SKILL.md` and skill root traversal.
4. **Trigger Specificity and Jaccard Overlap (Borrow from `@elonmust1/skillcheck`):**
   - Only ~60 lines of logic (stopword stripping, distinctive token count >= 4, generic opener check,
     and pairwise Jaccard similarity >= 0.45). Running this on top of `the`'s real `yaml-rust2`
     parser avoids `@elonmust1/skillcheck`'s broken regex YAML parser.
5. **Zero-Latency Security Smoke Test (`skill-smoke` in `the skills lint`):**
   - Catches high-confidence hazards (symlink escapes, zero-width / BiDi trojan characters, raw keys
     in frontmatter/commands, and `curl | sh` pipes) in < 1 ms during normal linting and directs
     users to `the skills audit` for full static analysis.

