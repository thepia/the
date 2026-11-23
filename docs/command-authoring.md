# Authoring a command

A command group in `the` is **its own crate** implementing one shared contract, so adding a command is
a crate + a `register`, never a new dispatch arm. This is the standard every command follows, and the
gate enforces the parts a machine can check.

## The model

```
argv ──> dispatch ──> Registry.get(id) ──> Target::execute ──> Results ──> render ──> exit
         (the_output::command, the one shared API)
```

- The contract lives in **`the_output::command`** (`crates/the-output/src/command.rs`): `Target`,
  `Results`, `Ctx`, `Invocation`, `Registry`, `dispatch`, `Form`, `StreamState`, `CommandDescriptor`.
  Every command crate already depends on `the-output` to render, so there is no separate infra crate.
- Each command group is a `the-*` crate implementing `Target` (the pilot is `the-skills`).
- The binary (`crates/the`) declares only the root globals (`--format`, `--no-header`), registers each
  command crate, and calls `dispatch`. It holds no command logic.

## What a command crate provides

1. **Arguments** — a clap `Args`/`Subcommand` grammar (see `the-skills`'s `cli` module). Keep it a
   plain derive so `augment` can fold it and `execute` can recover it with `FromArgMatches`.
2. **`impl Target`** (4 methods):
   - `id()` — the routing key and top-level subcommand name (e.g. `"skills"`).
   - `descriptors()` — a `&'static [CommandDescriptor]`, one row per leaf command (help / governance +
     the drift check). Summaries are lowercase, imperative, no trailing period, `<= 60` chars.
   - `augment(root)` — fold this crate's grammar onto the clap root (`YourArgs::augment_args(...)`).
   - `execute(inv, ctx)` — recover the typed args from `inv.matches`, run the logic, return
     `Box<dyn Results>`. **Execution produces a result; it does not print.**
3. **`impl Results`** — the view, built once and rendered on demand:
   - `render(out, form)` — the result to **stdout** in `Form::{Plain, Ansi, Json}`.
   - `render_diagnostics(err, format)` — notes/warnings/errors to **stderr** (default streams each; a
     command with a capped summary overrides it).
   - `exit_code()` — `0` = success.

   Build the `the_output::table::Report` once with the `Human`/`Term` matching `ctx.form()`, then let
   `Report::render` emit plain, ansi, or json. Reuse `the-output` as a toolkit; do not reinvent rendering.

## Channel discipline (gated)

stdout carries only the result; everything else is stderr; `--format human|json` is the one switch.
A command crate therefore **never writes output directly** — no `println!`/`eprintln!`, no
`Diagnostic::emit`, no `io::stdout`/`io::stderr`, no `output::print`. The pipeline writes the injected
streams. `scripts/check-command-channels.sh` (wired into `just gate`) fails if a `the-*` command crate
writes output directly.

## Registering it

In the binary (`crates/the/src/lib.rs`):

```rust
fn registry() -> Registry {
    Registry::new()
        .register(Box::new(the_skills::Skills))
    //  .register(Box::new(the_agents::Agents))   // a new command: one line + a crate dep
}
```

## Testing standard — wide coverage from small inputs

The injected streams and the trait boundary make the whole pipeline unit-testable against buffers. A
command crate carries three layers of tests; together they hold the per-crate **90% line-coverage** floor.

1. **Drift test** — `descriptors()` must equal the clap subcommand tree (a command added to one but not
   the other is a registration bug). See `descriptors_match_the_clap_tree` in `the-skills`.
2. **Conformance matrix** — the standard way to prove flexibility. List your subcommands × flags once as
   an argv matrix and hand it to `the_output::command::conformance::assert_matrix`. For every line it
   runs **plain, ansi, and json** and asserts the cross-form invariants:
   - plain stdout carries no ANSI escapes (color is opt-in, never leaked),
   - ansi stripped of color equals plain (color is the only difference),
   - json stdout parses as JSON (the machine form is always valid),
   - the exit code never changes with the form.

   Add a matrix line whenever a subcommand or flag lands. See `skills_matrix_is_form_stable`.
3. **In-process behavior tests** — call `Target::execute` against a hermetic temp fixture and assert the
   specific result/diagnostics/exit (goldens). See `execute_lints_a_dir_in_process`.

Keep the slower, real-binary CLI integration tests (`crates/the/tests/`) as an end-to-end smoke/regression
guard; the bulk of coverage is the fast in-process tests above. `cargo llvm-cov --workspace` attributes
per crate; no crate regresses below 90%. (The binary's residual is the unix argv0 re-exec shim, whose
`exec()` discards its own counters — covered via the integration subprocess, not countable in-process.)

## Checklist for a new command

- [ ] New `the-<name>` crate depending on `the-output` (and its logic crates).
- [ ] `cli` grammar (derive), `impl Target` (id / descriptors / augment / execute), `impl Results`.
- [ ] No direct output — results through `Results` only (`just command-channels`).
- [ ] `register` it in the binary.
- [ ] Drift test + a conformance matrix (`assert_matrix`) + in-process behavior tests.
- [ ] `just gate` green; `cargo llvm-cov --workspace` keeps the crate at/above 90%.
