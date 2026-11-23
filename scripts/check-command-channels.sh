#!/usr/bin/env bash
# Channel-discipline gate for command crates.
#
# Every command group is a `the-*` crate that implements `the_output::command::Target` and renders
# ONLY through `Results` (stdout) + diagnostics (stderr), which the pipeline writes. A command crate
# must therefore never write output directly. This check fails if one does.
#
# Scope: `crates/the-*/src` except `the-output` (which owns the output primitives themselves). The
# binary (`crates/the`) legitimately holds no command logic. Keep this wired into `just gate`.
set -euo pipefail

cd "$(dirname "$0")/.."

# Direct-output constructs a command crate must not use (results go through `Results`):
#   println!/print!/eprintln!/eprint!  — raw writes
#   .emit(                              — Diagnostic::emit writes to the real stderr
#   output::print / io::stdout / io::stderr — bypassing the injected streams
pattern='e?print(ln)?!|\.emit\(|output::print|io::std(out|err)'

violations=0
for dir in crates/the-*/; do
  name="$(basename "$dir")"
  [ "$name" = "the-output" ] && continue
  [ -d "$dir/src" ] || continue
  if hits="$(grep -rnE "$pattern" "$dir/src" 2>/dev/null)"; then
    echo "command-channels: $name writes output directly; route it through the_output::command::Results"
    echo "$hits"
    violations=1
  fi
done

if [ "$violations" -ne 0 ]; then
  echo
  echo "A command crate's result must flow through Results::render (stdout) and its diagnostics"
  echo "through Results::render_diagnostics (stderr). See docs/command-authoring.md."
  exit 1
fi
echo "command-channels OK"
