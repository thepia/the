# `the skills`: Implementation

The code lives in [`crates/the/src/skills/`](../../crates/the/src/skills/); each module's doc
comment has the detail.

| Module | Role |
|---|---|
| [`locate.rs`](../../crates/the/src/skills/locate.rs) | Where each assistant looks, in its precedence order ([skills-search.md](skills-search.md)) |
| [`installed.rs`](../../crates/the/src/skills/installed.rs) | Which assistant CLIs are installed |
| [`scan.rs`](../../crates/the/src/skills/scan.rs) | Finding `SKILL.md` files under a root; symlink detection |
| [`parse.rs`](../../crates/the/src/skills/parse.rs) | Frontmatter, each assistant's load rules, the format lint |
| [`resolve.rs`](../../crates/the/src/skills/resolve.rs) | One entry per distinct skill with each assistant's verdict |
| [`links.rs`](../../crates/the/src/skills/links.rs) | Local link checks |
| [`smoke.rs`](../../crates/the/src/skills/smoke.rs) | Security smoke checks |
| [`cmd.rs`](../../crates/the/src/skills/cmd.rs) | The `list`, `show`, `dirs` and `lint` commands |

When behavior changes, keep these in step: [skills-search.md](skills-search.md) for search and load
rules, [lint-checks.md](lint-checks.md) for checks, fixes and confidence,
[SKILLS-LINT.md](SKILLS-LINT.md) for design and plans, and [validations.md](validations.md) for the
validation scripts in [`scripts/skills/`](../../scripts/skills/) and the record of live checks.
