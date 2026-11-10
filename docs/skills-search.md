# How `the skills` finds skills

`the skills` imitates the skill search and load rules of three coding agents, so it can say, for any
skill on the machine, which agents load it and why the others don't. This page is the record of
those rules: where each came from, what we assumed where the source is silent, and what is not
imitated yet. The code lives in [`crates/the/src/skills/`](../crates/the/src/skills/).

## Sources

| Agent | Source | Version |
|---|---|---|
| Claude Code | Official docs: [skills](https://code.claude.com/docs/en/skills), [managed settings](https://code.claude.com/docs/en/managed-settings), [env vars](https://code.claude.com/docs/en/env-vars) | fetched 2026-11-10 |
| OpenCode | Source: `packages/opencode/src/skill/index.ts`, `config/paths.ts`, `effect/runtime-flags.ts` in `anomalyco/opencode` | v1.19.5 (`810c0eba`) |
| Goose | Source: `crates/goose/src/skills/mod.rs`, `config/paths.rs` | `thepia/goose` fork at `846cbeaf` |

## Where each agent looks

Listed in each agent's precedence order. "Walk" means the current directory and each parent.

| Location | Claude Code | OpenCode | Goose |
|---|---|---|---|
| Managed `/etc/claude-code/.claude/skills` (Linux), `/Library/Application Support/ClaudeCode/.claude/skills` (macOS) | yes, unless `CLAUDE_CODE_DISABLE_POLICY_SKILLS` | | |
| `~/.claude/skills` | yes, plus `~/.claude/skills/synced` | yes | yes |
| `~/.agents/skills` | | yes | yes |
| `.claude/skills` in the project | walk to the repository root | walk to the git worktree, or to `/` outside git | cwd only |
| `.agents/skills` in the project | | walk, as above | cwd only |
| `.goose/skills` in the project | | | cwd only |
| `{skill,skills}` in `$XDG_CONFIG_HOME/opencode`, `.opencode` (walk), `~/.opencode`, `$OPENCODE_CONFIG_DIR` | | yes | |
| `<goose config>/skills` | | | yes |
| `~/.config/agents/skills` | | | yes |

- OpenCode skips `.claude` and `.agents` entirely with `OPENCODE_DISABLE_EXTERNAL_SKILLS`, and
  `.claude` alone with `OPENCODE_DISABLE_CLAUDE_CODE` or `OPENCODE_DISABLE_CLAUDE_CODE_SKILLS`.
  `OPENCODE_DISABLE_PROJECT_CONFIG` skips the `.opencode` walk. `$XDG_CONFIG_HOME` falls back to
  `~/.config`.
- `<goose config>` is `$GOOSE_PATH_ROOT/config` when that is an absolute path, otherwise
  `$XDG_CONFIG_HOME/goose` (only if absolute) or `~/.config/goose`, on Linux and macOS alike.

`the skills dirs` prints this plan as resolved on the current machine.

## How deep, and what counts as a skill

- **Claude Code** reads exactly `<root>/<name>/SKILL.md`; `<name>` may be a symlink.
- **OpenCode** globs `**/SKILL.md` below each root, following symlinks. Under `.claude`/`.agents` it
  includes dot-directories; under its config dirs it does not.
- **Goose** walks recursively, following symlinks with a cycle guard, and skips `.git`, `.hg` and
  `.svn`.

A skill reached through several paths (a symlink, or a root several agents share) is one entry in
`the`, keyed by its resolved path, with every location listed and any symlink on the way reported.
That is how a skill or a whole skills directory shared between agents is spotted.

## Which skills an agent loads

| | Claude Code | OpenCode | Goose |
|---|---|---|---|
| Missing or invalid frontmatter | loads, with no fields | skips | skips |
| No `name` | loads under the directory name | skips | skips |
| Other | | skips a non-string `description` | skips a `name` containing `/` |

## Same name in several places

- **Claude Code:** managed beats personal beats project. The rest are *shadowed*.
- **Goose:** the first found in its search order wins (project before user). The rest are *shadowed*.
- **OpenCode:** the last one parsed wins, but it parses concurrently, so which one it keeps is not
  predictable. `the` reports all of them as a *conflict* and warns.

## Format lint

None of the three agents enforce the published Agent Skills format (agentskills.io): `name` of 1-64
lowercase letters, digits and single hyphens, equal to the directory name, and a `description` of
1-1024 characters. `the` checks it anyway and reports deviations as notes (`skill-lint`), separate
from the warnings for what actually makes an agent skip a skill (`skill-skipped`, `skill-conflict`).

## Assumptions where the source is silent

- **Claude Code outside a repository:** the docs say the project walk ends at the repository root
  but not where it ends without one. `the` searches only the current directory then.
- **Claude Code duplicates inside the project tier:** undocumented. `the` lets the nearest win.
- **Claude Code and invalid YAML:** the docs say such a skill loads with no fields set. A Claude Code
  session was observed listing the full description of a skill whose frontmatter strict YAML
  rejects (an unquoted `: ` in a plain value), so its parser is more lenient than documented. `the`
  follows the docs and shows no description; the gap is open.
- **OpenCode's upward walk** is taken level by level, nearest first. It affects only which duplicate
  OpenCode would intend to keep, which `the` reports as a conflict anyway.

## Not imitated yet

- Plugin skills (Claude Code, Goose).
- OpenCode `skills.paths` and `skills.urls` from its config files.
- Claude Code `--add-dir` directories and nested `.claude/skills` loaded on demand.
- Whether `CLAUDE_CONFIG_DIR` moves Claude Code's personal skills directory (implied, not documented).
- OpenCode's unreleased rewrite (`dev` branch), which changes these rules again.

`--dir <PATH>` searches any other directory, under no agent's rules.
