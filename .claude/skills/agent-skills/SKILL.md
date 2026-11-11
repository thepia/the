---
name: agent-skills
description: >
  How Agent Skills are resolved, found, parsed, lint-checked, verified and managed on disk across
  coding agents (Claude Code, OpenCode, Goose), and the discovery model `the skills` imitates. Use
  when working on `the` CLI's skill search, resolution, load or format-lint logic; reasoning about
  which directories an agent searches, how deep it reads, whether it walks parent directories up to
  the repository root, or what counts as a valid skill; debugging why one agent loads a skill while
  another shadows, conflicts on, or skips it; empirically verifying how an installed agent resolves
  skills on disk; authoring a portable SKILL.md frontmatter that all three agents accept; or
  questions on skill scope, precedence, shadowing, discovery paths, repo-root walk, symlinked
  skills, or format lint. Do not use for general Rust terminal/UI formatting (such as terminal width
  truncation) or for copywriting a skill's prose to improve LLM trigger rates.
---

# Agent Skills across agents

An Agent Skill is a directory holding a `SKILL.md` (optional YAML frontmatter plus Markdown
instructions) that an agent discovers and may load into context to teach itself a task. Claude Code,
OpenCode and Goose each search for skills, decide which to load, and resolve same-named collisions by
**different rules**. `the skills` imitates all three so it can say, for any skill on the machine,
which agents load it and why the others do not.

This skill is the orientation. The authoritative, versioned, source-cited record of the imitation
rules is [docs/skills-search.md](../../../docs/skills-search.md), and the code lives under
[crates/the/src/skills/](../../../crates/the/src). When a rule here and the doc disagree, the doc
wins and this file is stale: fix it.

## The five questions this domain answers

- **Resolved**: which directories each agent searches, in what precedence, and (on a collision)
  which copy wins.
- **Found**: how deep each agent reads under a root and what counts as a skill there.
- **Described**: the `SKILL.md` shape, the frontmatter fields, and the published format a skill is
  measured against.
- **Quality-checked**: format deviations (notes) versus the things that actually make an agent drop
  a skill (warnings).
- **Managed across agents**: one skill reached from many paths, same name in many places, and
  sharing through symlinks.

## Vocabulary (the verdicts `the` reports)

Keep these precise; they are the output contract, not loose synonyms.

- **scope**: where a skill sits, one of `managed` (org policy), `user` (per-machine home), `project` (in
  the repo tree). An agent's precedence is expressed over these.
- **loaded**: the agent reads and uses this copy.
- **shadowed**: a same-named skill in a higher-precedence location won; this copy is live on disk
  but the agent ignores it. Claude Code and Goose both pick a single winner, so losers are shadowed.
- **conflict**: the agent has no deterministic winner. OpenCode parses roots concurrently and keeps
  "the last one," which is not predictable, so `the` reports every same-named OpenCode skill as a
  conflict and warns rather than guessing.
- **skipped**: the agent rejects the skill outright (bad frontmatter, bad name). Claude Code is
  lenient and still loads; OpenCode and Goose skip.
- **linked**: the skill, or its whole skills directory, is reached through a symlink (often to share
  one skill across agents). One entry, keyed by resolved path, with the link reported.

## Resolved, found, described: the shape, not the table

Do not memorize or paraphrase the per-agent location tables; read them from
[docs/skills-search.md](../../../docs/skills-search.md), which carries the source and version each
rule came from. The shape to hold in your head:

- **Where.** All three read a `user` home skills dir (`~/.claude/skills`, also `~/.agents/skills` for
  OpenCode and Goose). For `project` skills they differ on the walk: Claude Code walks parents up to
  the repository root (version-sensitive and directional; see below), OpenCode walks to the git
  worktree (or `/` outside git), Goose reads the current directory only. Managed/policy skills are
  Claude Code only.
- **How deep.** Claude Code reads exactly `<root>/<name>/SKILL.md`. OpenCode globs `**/SKILL.md`
  below each root. Goose walks recursively with a symlink cycle guard, skipping `.git`/`.hg`/`.svn`.
- **Described.** Frontmatter is `name` (optional; defaults to the directory name) and `description`
  (the triggering text). The published format (agentskills.io) is stricter than any agent enforces:
  `name` is 1 to 64 chars of lowercase letters, digits and single hyphens, equal to the directory
  name; `description` is 1 to 1024 chars. `the` checks this format regardless and reports deviations
  as lint notes.

### Claude Code precedence (verified against the docs, 2026-11)

Highest to lowest: **Enterprise/managed → Personal (`~/.claude/skills`) → Project
(`.claude/skills` at the root) → Nested (`.claude/skills` in subdirs) → `--add-dir` → Plugin →
claude.ai synced.** Highest scope wins on a name collision, so a Personal skill shadows a
same-named Project skill. "Repository root" bounds the Project walk upward; outside any git repo the
docs are silent and `the` searches the current directory only (recorded as an assumption in the doc).

**The Project parent-walk is version-sensitive and directional; verify it, do not assert it.** The
current docs say project skills load from `.claude/skills/` in the start directory and every parent
up to the repository root. But this is a recent monorepo feature (the same page gates related
behavior, `/cd` picking up a directory's skills, to v2.1.246+), and it is directional: skills in a
`.claude/skills/` *below* the start directory do not load at startup, they load lazily the first time
Claude reads or edits a file in that subtree. Rules (CLAUDE.md memory) have walked parents far
longer, so older builds load rules from parents but project skills from the start directory only;
that is why a running Claude can show no parent pickup while the docs describe it. Do not assume
rules and skills walk alike on a given build. Settle it on the installed version with the probe in
[scripts/](scripts/) rather than trusting the prose.

## Managed across agents: collisions and sharing

- **Same name, several places.** Claude Code: managed beats personal beats project; rest shadowed.
  Goose: first found in its search order wins (project before user); rest shadowed. OpenCode:
  nondeterministic, so `the` emits a conflict.
- **One skill, several paths.** A skill reached through a shared root or a symlink is a single entry
  keyed by its resolved path, every location listed, every symlink on the way reported. This is how
  a skill shared between agents is spotted rather than double-counted.

## Quality-checked: two distinct severities

Keep the channels separate, because they answer different questions:

- **`skill-skipped` / `skill-conflict` (warnings)**: the skill will not load, or loads
  unpredictably, for a real agent. This is about behavior.
- **`skill-lint` (notes)**: the skill loads fine but deviates from the published format. This is
  about conformance, not breakage. Never let a lint note masquerade as a load failure.

Output discipline (it is a diagnostics tool): the result table goes to **stdout**, every diagnostic
to **stderr**, so `the skills list > out` stays clean. `--format json` gives typed JSON on stdout and
one JSON object per diagnostic on stderr.

## Working discipline when you change this area

The value of `the skills` is that its answer matches what the agents actually do. Protect that:

- **Cite the source and version.** Every imitation rule traces to a doc page or a pinned source
  commit in [docs/skills-search.md](../../../docs/skills-search.md). A new or changed rule updates
  that table in the same change; code and doc move together or the tool lies.
- **Observe, do not guess, where a source is silent.** Several rules are undocumented (Claude Code
  outside a repo, intra-tier duplicates, lenient-YAML parsing). The doc has an explicit "assumptions"
  section and a "not imitated yet" section. When behavior is uncertain, run the real agent and record
  what you saw; add an assumption with its reasoning rather than inventing a confident rule.
- **Honesty over coverage.** If a tier is not imitated (plugins, OpenCode `skills.paths`/`skills.urls`,
  `--add-dir`), it belongs in "not imitated yet," not silently half-handled. A gap named is worth more
  than a gap papered over.
- **No em-dashes** in anything authored here (docs, comments, this skill); restructure the sentence
  instead.

## Verifying resolution against the real agent

The agent's real behavior is the ground truth and it drifts between versions, so
the claims above ship with scripts that check them. The scripts support both
Claude Code and Gemini CLI via `--agent auto|claude|gemini` and `--model MODEL`
(auto-detecting whichever binary is on PATH); [scripts/](scripts/) is the
extension point for the other agents.

- `scripts/claude-skill-dirs.py [DIR] [--agent claude|gemini]` is the **oracle**:
  it prints the skills roots the selected agent's documented algorithm would
  search from DIR, in precedence order, flagging parents (version-sensitive) and
  lazy-below roots, the git repository-root boundary, and the installed agent
  version. It is the "what should load" reference. Deterministic, stdlib only, no
  network.
- `scripts/probe-claude-resolution.sh [--agent auto|claude|gemini] [--model MODEL]`
  is a **one-shot live probe**: it builds a fixture with marker skills at five
  positions (launch dir, repo root, an intermediate parent, one above the
  repo-root boundary, one below the start dir), launches `claude --model haiku -p`
  (override with `--model sonnet` or `CLAUDE_MODEL`) or
  `gemini --skip-trust -m gemini-2.5-flash -p` (`GEMINI_MODEL`) from a deep
  subdirectory, and reports observed versus expected.
- `scripts/eval-resolution.py [--self-test] [--agent auto|claude|gemini] [--model MODEL]`
  is the **ongoing conformance eval**: it runs the marker cases
  (parent-walk/boundary/lazy, name-defaults-to-directory, lenient-YAML) in a
  single combined fixture against `gemini skills list` (zero API calls) or the
  real `claude` (1 single-turn `--model haiku` call, override with `--model sonnet`)
  and exits non-zero on any divergence, so it can gate CI or run on a schedule
  and go red when a release changes behavior. `--self-test` validates the
  harness itself against the oracle; OpenCode and Goose adapters are declared
  pending, never faked. Both scripts skip cleanly when no usable agent binary is
  on PATH.
- `scripts/eval-trigger.py [--self-test] [--model haiku|sonnet|gemini-2.5-flash-lite|gemini-2.5-flash]`
  is the **lightweight local trigger eval**: it checks all 20 queries in
  `../agent-skills-workspace/trigger-eval.json` against this skill's frontmatter
  description in a single batched prompt (~1k tokens total instead of spawning
  20 to 300 full agent sessions), switching between `claude` and `gemini`
  automatically from `--model` or `--agent`. Pass `--max-iterations N` (with
  optional `--apply`) to propose description improvements when a query fails.

A red eval is the signal to re-derive the rule from the agent and update this
skill and [docs/skills-search.md](../../../docs/skills-search.md) together, rather
than trusting the prose.

## Where to go next

- [docs/skills-search.md](../../../docs/skills-search.md): the authoritative per-agent rules, with
  sources, versions, assumptions, and what is not imitated yet. Read this before editing resolution
  code.
- Upstream sources of truth: Claude Code docs (`code.claude.com/docs/en/skills`, managed-settings,
  env-vars), OpenCode source (`anomalyco/opencode`: `src/skill/index.ts`, `config/paths.ts`), Goose
  source (`thepia/goose`: `crates/goose/src/skills/mod.rs`, `config/paths.rs`).
- The published format: agentskills.io.
