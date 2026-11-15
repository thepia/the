# `the skills`

`the skills` tells you which of your Agent Skills each coding assistant will actually load from
where you stand, why the others won't, and what to fix. It covers Claude Code, Gemini CLI, OpenCode
and Goose, applying each assistant's own search and load rules rather than one generic rule.

## Commands

- **`the skills list`**: every skill found from the current directory, with the assistants that
  load it and whether it is shared through a symlink. Looks in your home and config directories,
  the current project, and the parent folders each assistant walks.
- **`the skills show <name>`**: one skill in full: its metadata, each assistant's verdict
  (`loaded`, `shadowed` by a same-named skill, an OpenCode `conflict`, or `skipped` with the
  reason), every path it is reached from, and its instructions.
- **`the skills dirs`**: every directory each assistant searches, whether it exists, and where a
  symlinked one leads. Use it when a skill is "missing".
- **`the skills lint [<name>]`**: problems that make an assistant drop a skill, deviations from the
  Agent Skills format, broken links to bundled files, and security hazards such as leaked keys. Each
  finding says what is wrong, why, and how to fix it. Exits `1` on any error or warning; format
  notes alone don't fail it.

All of them accept `--agent` (one assistant's exact view), `--scope`, `--dir` (search another
directory), `--format json` and `--no-header`. Results go to stdout and diagnostics to stderr, so
output pipes cleanly; see `the skills --help`.
