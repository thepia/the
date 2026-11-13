# `the skills lint` Checks

## Why `the skills lint`?

Most skill linters check a single `SKILL.md` file in isolation against basic regexes. `the skills lint` validates skills against how real coding agents discover, route, and execute them (see [SKILLS-LINT.md](SKILLS-LINT.md) for the full design):

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

## Checks, fixes and confidence

Every check `the skills lint` runs today. The fix is the `help` line the diagnostic prints.

**Confidence** says how sure `the` is that the check matches the assistants' real behavior:

| Level | Basis |
|---|---|
| **verified** | Confirmed by running the real assistant; recorded in the [validation record](validations.md#validation-record). |
| **documented** | Stated in the assistant's official documentation or the Agent Skills specification. |
| **source** | Read in the assistant's source code (version in [skills-search.md](skills-search.md#sources)). |
| **modeled** | Taken from a model of the behavior (the repo's resolution oracle, or a third-party linter spec), not yet checked against the assistant's own docs, code or behavior. |
| **assumed** | Our inference where the sources are silent. |
| **n/a** | A file or pattern check that does not depend on any assistant's behavior. |

### Assistant drops the skill (`warning[skill-skipped]`, cause code in the message)

| Cause | Who skips | Fix | Confidence |
|---|---|---|---|
| `no-frontmatter` | Gemini CLI, OpenCode, Goose | start SKILL.md with a `---` block holding `name:` and `description:`, closed by `---` | OpenCode, Goose: source. Gemini: modeled. Claude Code loading it anyway: documented. |
| `frontmatter-unterminated` | Gemini CLI, OpenCode, Goose | close the frontmatter with a `---` line | as above |
| `frontmatter-invalid` | Gemini CLI, OpenCode, Goose | fix the YAML; a plain value containing `: ` must be quoted (`description: "..."`) or use a `>-` block | as above; Claude Code was observed to be even more lenient than its docs |
| `name-missing` | Gemini CLI, OpenCode, Goose | set `name:` to lowercase letters, digits and single hyphens | OpenCode, Goose: source. Gemini: modeled. |
| `name-has-slash` | Goose | remove the `/` from `name:` | source |
| `description-missing` | Gemini CLI | add a `description:` saying what the skill does and when to use it | modeled |
| `description-not-string` | OpenCode | make `description:` a plain string | source |

### Name collisions

| Code | Severity | What it means | Fix | Confidence |
|---|---|---|---|---|
| `skill-conflict` | warning | Several skills share a name; OpenCode keeps one of them unpredictably | rename all but one, or remove the duplicates | source |
| `claude-alias-collision` | warning | Several skills claim the same frontmatter `name`; Claude Code gives that `/` command to only one (each stays invocable by its directory name) | give each skill a unique `name:` (ideally its directory name), or remove `name:` | verified |

### Agent Skills format (`note[skill-lint]`, advisory)

None of the assistants enforce these, so they never fail `lint` on their own; they keep skills
portable.

| Cause | Fix | Confidence |
|---|---|---|
| `name-format` | set `name:` to lowercase letters, digits and single hyphens | documented (Agent Skills spec; Claude Code itself sets no length limit) |
| `name-mismatch` | make `name:` equal the directory name, or rename the directory | documented; Claude Code naming verified |
| `description-missing` | add a `description:` saying what the skill does and when to use it | documented |
| `description-too-long` | shorten `description` to 1024 characters or fewer | documented |
| `compatibility-invalid` | set `compatibility:` to a non-empty string of 500 characters or fewer | documented |
| `license-not-string` | make `license:` a plain string (for example `MIT` or `Apache-2.0`) | assumed (the docs name the field but not its type) |
| `metadata-not-map` | make `metadata:` a flat YAML mapping of `key: value` scalar pairs | documented (Claude Code drops a value that isn't a map) |
| `allowed-tools-invalid` | set `allowed-tools:` to a space-delimited string or a list of tool strings | documented |
| `unknown-frontmatter-key` | remove the unrecognized key or move custom fields under `metadata:` | documented for Claude Code's keys (checked 2026-11-13); assumed for other assistants' keys |

### Local links (warnings)

| Code | Fix | Confidence |
|---|---|---|
| `broken-link` | create the missing file or update the relative link target in SKILL.md | n/a; expanding `${CLAUDE_SKILL_DIR}` is modeled |
| `link-escapes-skill` | keep linked files inside the skill directory so the skill stays self-contained | n/a |
| `broken-symlink` | remove the dangling symlink or point it to an existing file inside the skill directory | n/a |

Links inside code blocks and inline code are ignored, and so are templates (`{{name}}.md`,
`<template>`), unknown variables, and `${CLAUDE_PROJECT_DIR}` targets, which depend on the session.

### Security smoke test

| Code | Severity | Fix | Confidence |
|---|---|---|---|
| `exposed-secret` | error | remove the credential literal from SKILL.md and reference an environment variable instead | n/a |
| `trojan-source` | error | remove hidden bidirectional or zero-width Unicode characters from SKILL.md | n/a |
| `symlink-escape` | error | replace the symlink with a file inside the skill directory | n/a |
| `unrestricted-bash` | warning | scope `allowed-tools:` to specific commands (for example `Bash(git:*)`) instead of `Bash(*)` | n/a for the patterns; assumed that a bare `Bash` grant pre-approves every command |

These three errors are the only findings that fail `lint` for a skill an assistant would still
load: a leaked secret, a hidden-character attack or an escaping symlink is a safety problem, not a
style one.
