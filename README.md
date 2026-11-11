# the CLI

**Thepia's command line for Linux and Nix.** It starts small: `the skills` shows Agent Skill situation
for Claude Code, OpenCode and other agents where you stand.

```console
$ the skills list
warning[skill-skipped]: opencode and goose skip skill `notes`
  --> ~/.claude/skills/notes/SKILL.md
  = cause: invalid YAML at SKILL.md line 3 column 18: mapping values are not allowed in this context
  = help: fix the YAML; a plain value containing `: ` must be quoted (`description: "..."`) or use a `>-` block
NAME       SCOPE    AGENTS                   LINKED  DESCRIPTION
deploy     project  opencode, goose          no      Deploy this repository to staging.
notes      user     claude                   no      -
pdf-tools  user     claude, opencode, goose  no      Fill, merge and extract PDF forms.
recipes    user     goose                    no      Author goose recipes.
review     project  claude, opencode, goose  yes     Review a diff for correctness bugs.
```

The table goes to stdout and the diagnostics to stderr, so `the skills list > skills.txt` stays clean.
On a terminal, tables are colored, ruled and truncated to fit. Piped or redirected, they stay aligned
but plain and untruncated. `NO_COLOR`, `CLICOLOR=0` and `CLICOLOR_FORCE`/`FORCE_COLOR` are honored,
and `--no-header` drops the header row.

## Features

- **One view across agents.** Searches every location Claude Code, OpenCode and Goose use, with each
  agent's own depth, walk-up and load rules ([details](docs/skills-search.md)).
- **Per-agent verdicts.** Each skill is *loaded*, *shadowed* by a same-named one, in an OpenCode
  *conflict*, or *skipped*, with the reason and the fix.
- **Symlinks spotted.** A skill or skills directory shared between agents through a symlink is one
  entry, flagged `LINKED`, with where the link leads.
- **Lint, kept separate.** Deviations from the published SKILL.md format are notes. Problems that
  make an agent drop a skill are warnings.
- **Machine-readable.** `--format json` gives typed JSON on stdout and one JSON object per
  diagnostic on stderr. Exit codes: 0 ok, 1 error, 2 usage, 3 not found.

## Usage

```sh
the skills list                     # every skill, which agents load it
the skills list --agent goose       # exactly what Goose loads from here
the skills show review              # metadata, per-agent verdict, locations, instructions
the skills dirs                     # where each agent looks, and where symlinked dirs lead
the skills list --dir ./vendor/skills   # also search another directory
the skills list --format json | jq '.skills[] | select(.linked)'
```

`the skills show` reports every path a skill is reached from:

```console
$ the skills show review
name:        review
description: Review a diff for correctness bugs.
scope:       project
path:        ~/src/app/.claude/skills/review/SKILL.md
resolved:    ~/shared/review/SKILL.md

AGENT     STATUS  DETAIL
claude    loaded  -
opencode  loaded  -
goose     loaded  -

LOCATION                                  AGENTS                   LINKS TO
~/src/app/.claude/skills/review/SKILL.md  claude, opencode, goose  ~/shared/review

Instructions.
```

## Install

With Nix:

```sh
nix run github:thepia/the -- skills list     # try it
nix profile install github:thepia/the        # install
```

With Cargo (Rust 1.88 or newer):

```sh
cargo install --git https://github.com/thepia/the the
```

## Development

`nix develop` (or rustup, which reads `rust-toolchain.toml`) gives the pinned toolchain. Common tasks
are `just` recipes: `just test`, `just clippy`, `just gate` (everything CI runs on Rust), and
`just nix-check`. `just install-hooks` enables the pre-commit gate.

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT), at your option.
