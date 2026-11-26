# Thepia Agent Config (VS Code)

Live diagnostics for **`SKILL.md`** and **agent-definition** files, inline in VS Code.

The extension is a thin LSP client: it spawns **`thepiad --lsp`** over stdio and the daemon's language
server ([`crates/the-daemon/src/lsp.rs`](../../crates/the-daemon/src/lsp.rs)) surfaces the existing
`the-skills` / `the-agents` lint engines as editor diagnostics. Editing a skill or agent config shows
the same findings `the skills lint` reports, as you type.

## Requirements

The `thepiad` binary (built from this repo). Either put it on `PATH`, or point the extension at it:

```jsonc
// settings.json
"thepia.lsp.serverPath": "/absolute/path/to/the/target/release/thepiad"
```

Build it with `just build` (or `cargo build --release -p the-daemon`); the binary is `target/<profile>/thepiad`.

## What it lints

| File | Engine | Example findings |
|------|--------|------------------|
| `**/SKILL.md` | `the-skills` | missing / malformed frontmatter, name-vs-directory mismatch |
| `**/.claude/agents/**/*.md` | `the-agents` (Claude rules) | agent-definition rule violations |

Arbitrary markdown is deliberately not linted (it is not an agent config).

## Develop / run locally

```bash
cd editors/vscode
npm install
npm run compile          # or: npm run watch
```

Then press **F5** in VS Code (an Extension Development Host opens). Open a repo with a `SKILL.md`; the
findings appear in the Problems panel and inline.

## Remote-SSH

Works unchanged. The VS Code extension host runs on the remote node via vscode-server, so `thepiad` is
launched there, next to the files it lints. Install the extension on the remote and ensure `thepiad` is
reachable there (`PATH` or `thepia.lsp.serverPath`).

## Settings

| Setting | Default | Meaning |
|---------|---------|---------|
| `thepia.lsp.serverPath` | `thepiad` | Path to the server binary. |
| `thepia.lsp.serverArgs` | `["--lsp"]` | Arguments; the server speaks stdio. |
| `thepia.lsp.trace.server` | `off` | Trace JSON-RPC traffic (`off` / `messages` / `verbose`). |

## Scope

Increment 1: diagnostics only (full document sync). Hover, completion, incremental sync, and code
actions are deferred, and a daemon-hosted socket transport (mesh-reachable) is a later increment; the
server core in `the-daemon` is shared across transports.
