#!/usr/bin/env bash
# probe-claude-resolution.sh: observe how the INSTALLED Claude Code resolves
# project skills, to settle the version-sensitive questions the docs leave open:
# does this build walk parents up to the repository root, stop at the repo-root
# boundary, and withhold skills nested below the start directory until a file is
# touched there?
#
# It builds a throwaway fixture tree with a uniquely named marker skill at four
# positions, launches `claude -p` (headless) from a deep subdirectory, asks it to
# list the marker skills it can see, and reports observed-vs-expected per
# position. The fixture is removed on exit.
#
# Expected (per the current docs) when launched from .../repo/pkg/app:
#   marker-repo-root   LOADED   (repo root is a parent of cwd)
#   marker-parent-cwd  LOADED   (pkg/ is a parent of cwd, inside the repo)
#   marker-above-repo  ABSENT   (above the repository-root boundary)
#   marker-below-cwd   ABSENT   (below cwd: loads lazily on first file touch, not at startup)
#
# If the two parents come back ABSENT, this build does not walk parents for
# project skills (the pre-monorepo behavior), even though the docs describe it.
#
# Caveats: `claude` or `gemini` must be on PATH and authenticated. Supports
# `--agent auto|claude|gemini` (or `EVAL_AGENT`) and `--model <model>`.
# Defaults to `--model haiku` for Claude (override with `--model sonnet` or
# `CLAUDE_MODEL`) and `--model gemini-2.5-flash` for Gemini (`GEMINI_MODEL`).
# Headless print mode is assumed to load project skills the same way an
# interactive session does; if a future build diverges, that is itself a
# finding. No network beyond whatever the agent binary does.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
agent="${EVAL_AGENT:-auto}"
model_arg=""

while [ $# -gt 0 ]; do
  case "$1" in
    --agent)
      if [ -z "${2:-}" ]; then
        echo "probe-claude-resolution: --agent needs auto, claude, or gemini" >&2
        exit 2
      fi
      agent="$2"
      shift 2
      ;;
    --model)
      if [ -z "${2:-}" ]; then
        echo "probe-claude-resolution: --model needs a model name (e.g. haiku, sonnet, gemini-2.5-flash)" >&2
        exit 2
      fi
      model_arg="$2"
      shift 2
      ;;
    *)
      echo "probe-claude-resolution: unknown option: $1" >&2
      exit 2
      ;;
  esac
done

if [ "$agent" = "auto" ]; then
  case "$model_arg" in
    gemini*) agent="gemini" ;;
    haiku|sonnet|opus|claude*) agent="claude" ;;
    *)
      if command -v claude >/dev/null 2>&1; then
        agent="claude"
      elif command -v gemini >/dev/null 2>&1; then
        agent="gemini"
      else
        agent="claude"
      fi
      ;;
  esac
fi

if ! command -v "$agent" >/dev/null 2>&1; then
  cat >&2 <<EOF
probe-claude-resolution: the '$agent' binary is not on PATH, so the live probe
cannot run here (e.g. you are inside an IDE extension, which does not export the
CLI). Run this script from a terminal where '$agent --version' works. Meanwhile,
the documented expectation is:
EOF
  python3 "$here/claude-skill-dirs.py" "$(pwd)" --agent "$agent" >&2 || true
  exit 3
fi

if [ "$agent" = "gemini" ]; then
  model="${model_arg:-${GEMINI_MODEL:-gemini-2.5-flash}}"
else
  model="${model_arg:-${CLAUDE_MODEL:-haiku}}"
fi

echo "agent:  $agent ($("$agent" --version 2>/dev/null || echo unknown))"
echo "model:  $model"

work="$(mktemp -d "${TMPDIR:-/tmp}/agent-skill-probe.XXXXXX")"
cleanup() { rm -rf "$work"; }
trap cleanup EXIT

mk_skill() {  # mk_skill <base_dir> <name>
  local base="$1" name="$2"
  mkdir -p "$base/.claude/skills/$name" "$base/.gemini"
  cat >"$base/.claude/skills/$name/SKILL.md" <<EOF
---
name: $name
description: Probe marker skill $name used to observe agent skill resolution. It does nothing.
---

Marker skill $name.
EOF
  ln -sfn "../.claude/skills" "$base/.gemini/skills"
}

above="$work/above"
repo="$above/repo"
app="$repo/pkg/app"
mkdir -p "$app/sub"

mk_skill "$above"     marker-above-repo
mk_skill "$repo"      marker-repo-root
mk_skill "$repo/pkg"  marker-parent-cwd
mk_skill "$app"       marker-cwd
mk_skill "$app/sub"   marker-below-cwd
printf 'probe fixture\n' >"$app/README.md"

git -C "$repo" init -q
git -C "$repo" config user.email probe@example.com
git -C "$repo" config user.name probe
git -C "$repo" add -A
git -C "$repo" -c commit.gpgsign=false commit -qm init

echo
echo "Documented expectation from the launch dir ($app):"
python3 "$here/claude-skill-dirs.py" "$app" --agent "$agent" || true

prompt='You have a set of available skills. Output a line "SKILL: <name>" for every available skill whose name begins with "marker-". If there are none, output exactly "SKILL: none". Output nothing else and do not call any tools.'

echo
if [ "$agent" = "gemini" ]; then
  echo "Running zero-API 'gemini skills list' from $app ..."
  tf="$work/trusted.json"
  python3 -c 'import json, sys; from pathlib import Path; Path(sys.argv[1]).write_text(json.dumps({str(Path(sys.argv[2]).resolve()): "TRUST_FOLDER"}))' "$tf" "$app"
  raw="$(cd "$app" && GEMINI_CLI_TRUSTED_FOLDERS_PATH="$tf" GEMINI_CLI_HOME="$work" gemini skills list 2>/dev/null || true)"
  out="$(awk '/^[[:space:]]*marker-[a-z0-9-]+/{print "SKILL: " $1}' <<<"$raw")"
  if [ -z "$out" ]; then out="SKILL: none"; fi
else
  echo "Running claude --setting-sources project --model $model -p from $app ..."
  out="$(cd "$app" && claude --setting-sources project --model "$model" -p "$prompt" 2>&1 || true)"
  if grep -qiE "hit your .*limit|hour limit|rate[_ ]limit|quota exceeded|resource_exhausted|credit balance" <<<"$out"; then
    echo "[RATE LIMIT] claude ($model) quota reached:" >&2
    echo "  $(head -n 1 <<<"$out")" >&2
    echo "  Tip: run '$0 --agent gemini' (zero API calls) while waiting for reset." >&2
    exit 0
  fi
fi
echo "--- $agent output ---"
echo "$out"
echo "---------------------"

seen() { grep -qiE "SKILL:[[:space:]]*$1\b" <<<"$out"; }
verdict() {  # verdict <name> <expect-loaded:0/1>
  local name="$1" expect="$2" obs fail=0
  if seen "$name"; then obs="LOADED"; else obs="ABSENT"; fi
  if { [ "$expect" = 1 ] && [ "$obs" = LOADED ]; } || { [ "$expect" = 0 ] && [ "$obs" = ABSENT ]; }; then
    printf "  %-18s observed=%-7s expected=%-7s  OK\n" "$name" "$obs" "$([ "$expect" = 1 ] && echo LOADED || echo ABSENT)"
  else
    printf "  %-18s observed=%-7s expected=%-7s  MISMATCH\n" "$name" "$obs" "$([ "$expect" = 1 ] && echo LOADED || echo ABSENT)"
    fail=1
  fi
  return $fail
}

echo
echo "Verdict (expected = current docs for $agent):"
rc=0
expect_parents=1
if [ "$agent" = "gemini" ]; then
  expect_parents=0
fi
verdict marker-cwd        1                 || rc=1
verdict marker-repo-root  "$expect_parents" || rc=1
verdict marker-parent-cwd "$expect_parents" || rc=1
verdict marker-above-repo 0                 || rc=1
verdict marker-below-cwd  0                 || rc=1

echo
if [ "$rc" = 0 ]; then
  echo "This build matches the documented resolution (parents walked to repo root; boundary and lazy-child honored)."
else
  echo "This build DIVERGES from the docs. If the two parents are ABSENT, it does not walk parents for project skills."
fi
exit "$rc"
