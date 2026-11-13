# Skills Validation and Guardrails (`scripts/skills/`)

Agents change their search and load behavior between versions, so every rule in [skills-search.md](skills-search.md) is only
as good as its last check against the real assistant. The scripts that check them live in
[`scripts/skills/`](../../scripts/skills/) and are driven by the
[`agent-skills`](../../.claude/skills/agent-skills/SKILL.md) skill. Deterministic, stdlib-only, no
network unless an assistant is invoked.

| Script | Role | Run it with |
|---|---|---|
| [`claude-skill-dirs.py`](../../scripts/skills/claude-skill-dirs.py) | **Oracle:** the skills roots each assistant's documented algorithm searches from a directory, in precedence order. The reference for "what should load". | `python3 scripts/skills/claude-skill-dirs.py [DIR] --agent claude\|gemini\|opencode` |
| [`probe-claude-resolution.sh`](../../scripts/skills/probe-claude-resolution.sh) | **One-shot live probe:** builds marker skills at five positions and asks the installed assistant which it loads. | `scripts/skills/probe-claude-resolution.sh --agent auto` |
| [`eval-resolution.py`](../../scripts/skills/eval-resolution.py) | **Conformance eval:** runs the marker cases (parent walk and boundary, depth, symlink, name default, lenient YAML) against every installed assistant and fails on any divergence from the oracle. `--self-test` checks the harness against the oracle without any assistant. | `just eval-skills` (or `agent=claude` etc.); CI job "Agent-skills resolution eval (non-blocking)" |
| [`eval-trigger.py`](../../scripts/skills/eval-trigger.py) | **Trigger eval:** checks queries against the `agent-skills` skill's description in one batched model call. Needs a local, gitignored eval set at `.claude/skills/agent-skills-workspace/trigger-eval.json`, which is not in the repository. | `just eval-trigger` |

**Guardrail rule.** A change to a search or load rule changes, in the same commit: the code, the
oracle, the tables in [skills-search.md](skills-search.md), and the confidence of the affected checks in
[lint-checks.md](lint-checks.md#checks-fixes-and-confidence). A live run that confirms or contradicts a rule
adds a row to the record below.

## Validation record

| Date | Assistant (version) | What was checked | How | Result |
|---|---|---|---|---|
| 2026-11-13 | Claude Code 2.1.258 | A skill's identity is its directory name; a frontmatter `name` shared by two skills gives the `/` command to only one | Manual `claude -p` sessions on a fixture ([Verified behavior](skills-search.md#verified-behavior)) | Confirmed; implemented as `claude-alias-collision` |
| 2026-11-13 | Claude Code 2.1.258 | Project skills stop at the repository root, while rules and `CLAUDE.md` load from every parent | Manual `claude -p` sessions started in `Thepia/cli`, `Thepia/the` and `Thepia` | Confirmed |
| 2026-11-13 | Oracle (claude, gemini, opencode) | The eval harness agrees with the oracle | `eval-resolution.py --self-test --agent all` | PASS, 5/5 cases each |
| 2026-11-13 | Gemini CLI, OpenCode, Goose | Live behavior | Not run: not installed on the validating machine; the Goose adapter is pending | Open |
