#!/usr/bin/env python3
"""eval-trigger: lightweight local trigger evaluation for a SKILL.md description.

Replaces the heavy multi-session `scripts.run_loop` / `scripts.run_eval` workflow
(which spawns 20 to 300 full agent sessions per run) with a quota-friendly runner
that evaluates all queries in `trigger-eval.json` in a SINGLE batched call by
default (~1.5k tokens total instead of ~600k+ tokens), and switches between
Claude (`haiku`, `sonnet`) and Gemini (`gemini-2.5-flash-lite`, `gemini-2.5-flash`)
based on `--model` or `--agent`.

Modes:
  (default)       1-call batched trigger classification across all queries in
                  the eval set, using the selected agent and model.
  --per-query     run 1 single-turn CLI call per query (`--max-turns 1`) instead
                  of batching. Use `--limit N` to cap the number of queries.
  --max-iterations N
                  when N > 1 and any query fails, ask the model to propose a
                  tighter description and re-evaluate (up to N iterations).
                  Writes the winning description only if `--apply` is passed.
  --self-test     validate SKILL.md frontmatter parsing, trigger-eval.json
                  loading, and the scoring pipeline with zero API calls.

Defaults:
  --skill-path    .claude/skills/agent-skills
  --eval-set      .claude/skills/agent-skills-workspace/trigger-eval.json
  --agent         auto (infers from --model, else claude or gemini on PATH)
  --model         haiku (for claude) or gemini-2.5-flash-lite (for gemini)

Usage:
  python3 eval-trigger.py [--model haiku|sonnet|gemini-2.5-flash-lite|gemini-2.5-flash]
                          [--agent auto|claude|gemini] [--skill-path DIR]
                          [--eval-set FILE] [--max-iterations N] [--apply]
                          [--per-query] [--limit N] [--self-test] [--json]
Exit: 0 when all evaluated queries match `should_trigger` (or skipped), 1 on
any false positive/negative, 2 on usage error.
"""

import argparse
import json
import os
import re
import subprocess
import sys
import urllib.error
import urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent
DEFAULT_SKILL_DIR = HERE.parent
DEFAULT_EVAL_SET = DEFAULT_SKILL_DIR.parent / "agent-skills-workspace" / "trigger-eval.json"

DEFAULT_MODELS = {
    "claude": "haiku",
    "gemini": "gemini-2.5-flash-lite",
}

# Realistic sibling skills included in the routing prompt so the model has to
# discriminate between adjacent domains rather than rubber-stamping the only skill.
DECOY_SKILLS = [
    (
        "rust-cli-dev",
        "Building, linting, formatting and testing Rust CLI subcommands with clap, cargo, clippy, and terminal table formatting.",
    ),
    (
        "mcp-server-config",
        "Configuring and wiring Model Context Protocol (MCP) servers and tools across coding agents.",
    ),
    (
        "ci-github-actions",
        "Authoring and debugging GitHub Actions workflows, release jobs, tag triggers, and Nix flake CI checks.",
    ),
]


def parse_skill_frontmatter(skill_md: Path):
    """Extract (name, description, raw_text) from SKILL.md."""
    text = skill_md.read_text(encoding="utf-8")
    clean = text.lstrip("\ufeff")
    if not clean.startswith("---"):
        raise ValueError(f"{skill_md}: missing opening --- frontmatter")
    parts = clean.split("\n---", 1)
    if len(parts) < 2:
        raise ValueError(f"{skill_md}: unterminated --- frontmatter")
    fm_lines = parts[0].splitlines()[1:]
    name = skill_md.parent.name
    desc_lines = []
    in_desc = False
    desc_indent = 0

    for line in fm_lines:
        stripped = line.lstrip()
        indent = len(line) - len(stripped)
        if in_desc:
            if stripped and indent <= desc_indent and re.match(r"^[A-Za-z0-9_-]+\s*:", stripped):
                in_desc = False
            else:
                if stripped:
                    desc_lines.append(stripped)
                continue
        if stripped.lower().startswith("name:"):
            val = stripped.split(":", 1)[1].strip().strip("\"'")
            if val:
                name = val
        elif stripped.lower().startswith("description:"):
            in_desc = True
            desc_indent = indent
            first = stripped.split(":", 1)[1].strip()
            if first not in ("", ">", ">-", "|", "|-"):
                desc_lines.append(first.strip("\"'"))

    description = " ".join(desc_lines).strip()
    if not description:
        raise ValueError(f"{skill_md}: missing description in frontmatter")
    return name, description, text


def replace_description_in_skill(skill_md: Path, new_description: str):
    """Replace the frontmatter description block in SKILL.md with a folded block."""
    text = skill_md.read_text(encoding="utf-8")
    parts = text.split("\n---", 1)
    name, _, _ = parse_skill_frontmatter(skill_md)
    wrapped = []
    words = new_description.split()
    line = "  "
    for w in words:
        if len(line) + len(w) + 1 > 100 and line.strip():
            wrapped.append(line)
            line = "  " + w
        else:
            line = (line + " " + w) if line.strip() else ("  " + w)
    if line.strip():
        wrapped.append(line)
    new_fm = "---\nname: " + name + "\ndescription: >\n" + "\n".join(wrapped)
    skill_md.write_text(new_fm + "\n---" + parts[1], encoding="utf-8")


def cli_version(binary: str):
    env = dict(os.environ)
    if binary == "gemini":
        env.setdefault("GEMINI_CLI_HOME", os.environ.get("TMPDIR", "/tmp"))
    try:
        return subprocess.run(
            [binary, "--version"], capture_output=True, text=True, check=True, env=env
        ).stdout.strip()
    except (subprocess.CalledProcessError, OSError):
        return None


def resolve_agent_and_model(agent_arg: str, model_arg):
    agent = agent_arg
    if agent == "auto" and model_arg:
        if model_arg.startswith("gemini") or model_arg == "flash":
            agent = "gemini"
        elif model_arg in ("haiku", "sonnet", "opus") or model_arg.startswith("claude"):
            agent = "claude"

    if agent == "auto":
        if cli_version("claude") is not None:
            agent = "claude"
        elif cli_version("gemini") is not None:
            agent = "gemini"
        else:
            agent = "claude"

    if model_arg == "flash":
        model_arg = "gemini-2.5-flash"
    env_key = "GEMINI_MODEL" if agent == "gemini" else "CLAUDE_MODEL"
    model = model_arg or os.environ.get(env_key, DEFAULT_MODELS[agent])
    return agent, model


class QuotaLimitError(Exception):
    def __init__(self, agent: str, model: str, message: str, reset_hint: str = ""):
        super().__init__(message)
        self.agent = agent
        self.model = model
        self.message = message
        self.reset_hint = reset_hint


class AgentBlockError(Exception):
    def __init__(self, agent: str, model: str, message: str, hint: str = ""):
        super().__init__(message)
        self.agent = agent
        self.model = model
        self.message = message
        self.hint = hint


LIMIT_PATTERNS = re.compile(
    r"(hit your .*limit|hour limit|rate[_ ]limit|quota exceeded|resource_exhausted|"
    r"http 429|status 429|too many requests|credit balance is too low|overloaded)",
    re.IGNORECASE,
)
RESET_PATTERN = re.compile(r"(resets?\s+[^\n]+)", re.IGNORECASE)


def classify_cli_failure(agent: str, model: str, detail: str):
    first_line = detail.splitlines()[0].strip() if detail.strip() else "unknown CLI error"
    if LIMIT_PATTERNS.search(detail):
        m = RESET_PATTERN.search(detail)
        reset_hint = m.group(1).strip() if m else ""
        return QuotaLimitError(agent, model, first_line, reset_hint)
    if "cloudcode-pa.googleapis.com" in detail:
        proj_match = re.search(r"project\s+(\d+)", detail)
        proj = proj_match.group(1) if proj_match else "your GCP project"
        url_match = re.search(r"(https://console\.developers\.google\.com\S+)", detail)
        enable_url = url_match.group(1) if url_match else "https://console.developers.google.com"
        return AgentBlockError(
            agent,
            model,
            f"Google Code Assist API (cloudcode-pa.googleapis.com) is disabled on GCP project {proj}.",
            f"1) Set GEMINI_API_KEY=<your-ai-studio-key> to call the Gemini REST API directly, or\n"
            f"  2) Enable the API at: {enable_url}",
        )
    if "Permission allow rule" in detail:
        return AgentBlockError(
            agent,
            model,
            first_line,
            "Remove the mid-command '*' wildcard from permissions.allow in ~/.claude/settings.json.",
        )
    return AgentBlockError(agent, model, detail[:400])


def call_gemini_rest(model: str, prompt: str, api_key: str) -> str:
    url = f"https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent?key={api_key}"
    payload = json.dumps({
        "contents": [{"parts": [{"text": prompt}]}],
        "generationConfig": {"temperature": 0.0},
    }).encode("utf-8")
    req = urllib.request.Request(url, data=payload, headers={"Content-Type": "application/json"}, method="POST")
    try:
        with urllib.request.urlopen(req, timeout=60) as resp:
            data = json.loads(resp.read().decode("utf-8"))
    except urllib.error.HTTPError as e:
        body = e.read().decode("utf-8", errors="replace")[:400]
        raise classify_cli_failure("gemini", model, f"HTTP {e.code}: {body}") from e
    candidates = data.get("candidates") or []
    if not candidates:
        raise AgentBlockError("gemini", model, f"Gemini REST API returned no candidates: {json.dumps(data)[:400]}")
    parts = (candidates[0].get("content") or {}).get("parts") or []
    return "\n".join(p.get("text", "") for p in parts)


def call_llm(agent: str, model: str, prompt: str) -> str:
    env = dict(os.environ)
    if agent == "gemini":
        api_key = env.get("GEMINI_API_KEY") or env.get("GOOGLE_API_KEY")
        if api_key and not env.get("GOOGLE_GENAI_USE_VERTEXAI"):
            return call_gemini_rest(model, prompt, api_key)
        # Unless Vertex AI or explicit override is active, strip GOOGLE_CLOUD_PROJECT
        # from the child env so OAuth login uses the free personal Code Assist tier
        # rather than failing when cloudcode-pa.googleapis.com is disabled on a
        # shell GCP project.
        if not env.get("GOOGLE_GENAI_USE_VERTEXAI") and not env.get("GEMINI_KEEP_CLOUD_PROJECT"):
            env.pop("GOOGLE_CLOUD_PROJECT", None)
            env.pop("GOOGLE_CLOUD_PROJECT_ID", None)
        cmd = ["gemini", "--skip-trust", "-m", model, "-p", prompt]
    else:
        cmd = [
            "claude",
            "--setting-sources", "project",
            "--model", model,
            "-p", prompt,
        ]
    # Run from /tmp so the CLI does not load workspace skills or repo context.
    proc = subprocess.run(cmd, cwd="/tmp", env=env, capture_output=True, text=True)
    combined = "\n".join(x for x in (proc.stderr.strip(), proc.stdout.strip()) if x)
    if proc.returncode != 0 or LIMIT_PATTERNS.search(combined):
        raise classify_cli_failure(agent, model, combined or f"{agent} exited with code {proc.returncode}")
    return proc.stdout


def build_batch_prompt(skill_name: str, description: str, queries: list) -> str:
    skills_block = [f"- {skill_name}: {description}"]
    for d_name, d_desc in DECOY_SKILLS:
        skills_block.append(f"- {d_name}: {d_desc}")

    queries_block = [
        f"Q{idx}: {item['query']}" for idx, item in enumerate(queries, start=1)
    ]
    return (
        "You are evaluating which user queries should trigger the Agent Skill "
        f'named "{skill_name}".\n\n'
        "Available skills (only activate a skill when the query genuinely needs "
        "its specialized domain knowledge rather than general coding or a sibling skill):\n"
        + "\n".join(skills_block)
        + "\n\nUser queries:\n"
        + "\n".join(queries_block)
        + "\n\nFor each query from Q1 to Q"
        + str(len(queries))
        + f', output a line in the exact format "Q<number>: TRIGGER" if you would '
        f'activate "{skill_name}", or "Q<number>: SKIP" if you would not. '
        "Do not call any tools. Output only those lines and nothing else."
    )


def parse_batch_output(out: str, count: int) -> dict:
    decisions = {}
    for line in out.splitlines():
        cleaned = re.sub(r"[*_`#-]+", "", line).strip()
        m = re.match(r"^Q?0*(\d+)\s*[:.\-]\s*(TRIGGER|SKIP)\b", cleaned, re.IGNORECASE)
        if m:
            idx = int(m.group(1))
            if 1 <= idx <= count:
                decisions[idx] = (m.group(2).upper() == "TRIGGER")
    if not decisions and count > 0:
        preview = out.strip()[:500] or "(empty stdout)"
        raise RuntimeError(f"Could not parse any Q<n>: TRIGGER/SKIP lines from model output:\n{preview}")
    return decisions


def evaluate_once(agent: str, model: str, skill_name: str, description: str, queries: list, per_query: bool) -> list:
    if not per_query:
        prompt = build_batch_prompt(skill_name, description, queries)
        raw = call_llm(agent, model, prompt)
        decisions = parse_batch_output(raw, len(queries))
    else:
        decisions = {}
        for idx, item in enumerate(queries, start=1):
            single_prompt = build_batch_prompt(skill_name, description, [item])
            raw = call_llm(agent, model, single_prompt)
            single = parse_batch_output(raw, 1)
            decisions[idx] = single.get(1, False)

    results = []
    for idx, item in enumerate(queries, start=1):
        want = bool(item["should_trigger"])
        got = decisions.get(idx, False)
        results.append({
            "index": idx,
            "query": item["query"],
            "should_trigger": want,
            "triggered": got,
            "passed": got == want,
        })
    return results


def summarize(results: list) -> dict:
    tp = sum(1 for r in results if r["should_trigger"] and r["triggered"])
    fn = sum(1 for r in results if r["should_trigger"] and not r["triggered"])
    tn = sum(1 for r in results if not r["should_trigger"] and not r["triggered"])
    fp = sum(1 for r in results if not r["should_trigger"] and r["triggered"])
    total = len(results)
    passed = tp + tn
    return {
        "total": total,
        "passed": passed,
        "failed": total - passed,
        "tp": tp,
        "tn": tn,
        "fp": fp,
        "fn": fn,
        "accuracy": round(passed / total, 4) if total else 1.0,
    }


def propose_improved_description(agent: str, model: str, skill_name: str, current_desc: str, failures: list) -> str:
    fn_list = [f"- {r['query']}" for r in failures if r["should_trigger"]]
    fp_list = [f"- {r['query']}" for r in failures if not r["should_trigger"]]
    prompt = (
        f'Improve the trigger description for the skill "{skill_name}" (max 1024 chars, '
        "no em-dashes).\n\n"
        f"Current description:\n{current_desc}\n\n"
        + ("False negatives (should have triggered but skipped):\n" + "\n".join(fn_list) + "\n\n" if fn_list else "")
        + ("False positives (triggered but should have skipped):\n" + "\n".join(fp_list) + "\n\n" if fp_list else "")
        + "Output ONLY the new single-paragraph description text (under 1024 characters), with no quotes or preamble."
    )
    out = call_llm(agent, model, prompt).strip().strip("\"'")
    return " ".join(out.splitlines()).strip()[:1024]


def main():
    p = argparse.ArgumentParser(description="Lightweight local skill trigger evaluator")
    p.add_argument("--skill-path", type=Path, default=DEFAULT_SKILL_DIR)
    p.add_argument("--eval-set", type=Path, default=DEFAULT_EVAL_SET)
    p.add_argument("--agent", choices=["auto", "claude", "gemini"], default=os.environ.get("EVAL_AGENT", "auto"))
    p.add_argument("--model", default=None, help="e.g. haiku, sonnet, gemini-2.5-flash-lite, gemini-2.5-flash")
    p.add_argument("--max-iterations", type=int, default=1, help="1 = eval only; >1 = propose description fixes on failure")
    p.add_argument("--apply", action="store_true", help="Write improved description back to SKILL.md")
    p.add_argument("--per-query", action="store_true", help="1 call per query instead of 1 batched call")
    p.add_argument("--limit", type=int, default=None, help="Evaluate only the first N queries")
    p.add_argument("--self-test", action="store_true", help="Validate parsing and scoring with zero API calls")
    p.add_argument("--json", action="store_true", help="Emit JSON report on stdout")
    args = p.parse_args()

    skill_md = args.skill_path / "SKILL.md" if args.skill_path.is_dir() else args.skill_path
    if not skill_md.is_file():
        print(f"eval-trigger: {skill_md}: not found", file=sys.stderr)
        return 2
    if not args.eval_set.is_file():
        print(f"eval-trigger: {args.eval_set}: not found", file=sys.stderr)
        return 2

    skill_name, description, _ = parse_skill_frontmatter(skill_md)
    queries = json.loads(args.eval_set.read_text(encoding="utf-8"))
    if args.limit:
        queries = queries[: args.limit]

    agent, model = resolve_agent_and_model(args.agent, args.model)

    if args.self_test:
        prompt = build_batch_prompt(skill_name, description, queries)
        synthetic_lines = [
            f"Q{idx}: {'TRIGGER' if q['should_trigger'] else 'SKIP'}"
            for idx, q in enumerate(queries, start=1)
        ]
        parsed = parse_batch_output("\n".join(synthetic_lines), len(queries))
        results = [
            {
                "index": idx,
                "query": q["query"],
                "should_trigger": bool(q["should_trigger"]),
                "triggered": parsed.get(idx, False),
                "passed": parsed.get(idx, False) == bool(q["should_trigger"]),
            }
            for idx, q in enumerate(queries, start=1)
        ]
        # Verify quota/limit classifier in self-test
        sample_limit = classify_cli_failure(
            "claude", "haiku", "You've hit your 5-hour limit · resets 6:20pm (Europe/Zurich)"
        )
        assert isinstance(sample_limit, QuotaLimitError)
        assert "6:20pm" in sample_limit.reset_hint

        summary = summarize(results)
        report = {
            "mode": "self-test (zero API calls)",
            "skill": skill_name,
            "description_chars": len(description),
            "agent": agent,
            "model": model,
            "prompt_chars": len(prompt),
            "summary": summary,
        }
        if args.json:
            print(json.dumps(report, indent=2))
        else:
            print(f"mode:        {report['mode']}")
            print(f"skill:       {skill_name} ({len(description)} chars)")
            print(f"target:      {agent} / {model}")
            print(f"queries:     {summary['total']} (batched prompt = {len(prompt)} chars, ~{len(prompt)//4} tokens)")
            print(f"self-test:   {summary['passed']}/{summary['total']} PASS")
        return 0 if summary["failed"] == 0 else 1

    has_rest_key = agent == "gemini" and bool(os.environ.get("GEMINI_API_KEY") or os.environ.get("GOOGLE_API_KEY"))
    if not has_rest_key and cli_version(agent) is None:
        msg = f"eval-trigger: '{agent}' binary is not on PATH. Use --self-test or run from a terminal with '{agent}'."
        if args.json:
            print(json.dumps({"skipped": True, "agent": agent, "reason": msg}))
        else:
            print(msg, file=sys.stderr)
        return 0

    best_desc = description
    best_results = []
    best_summary = {"passed": -1}
    history = []

    try:
        for iteration in range(1, max(1, args.max_iterations) + 1):
            results = evaluate_once(agent, model, skill_name, description, queries, args.per_query)
            summary = summarize(results)
            history.append({"iteration": iteration, "description": description, "summary": summary, "results": results})
            if summary["passed"] > best_summary["passed"]:
                best_desc = description
                best_results = results
                best_summary = summary
            if summary["failed"] == 0 or iteration >= args.max_iterations:
                break
            failures = [r for r in results if not r["passed"]]
            description = propose_improved_description(agent, model, skill_name, description, failures)
    except QuotaLimitError as e:
        if args.json:
            print(json.dumps({
                "skipped": True,
                "rate_limited": True,
                "agent": e.agent,
                "model": e.model,
                "message": e.message,
                "reset": e.reset_hint or None,
            }, indent=2))
        else:
            print(f"[RATE LIMIT] {e.agent} ({e.model}) quota reached:", file=sys.stderr)
            print(f"  {e.message}", file=sys.stderr)
            if e.reset_hint:
                print(f"  Reset window: {e.reset_hint}", file=sys.stderr)
            other = "GEMINI_API_KEY=... just eval-trigger gemini-2.5-flash-lite" if e.agent == "claude" else "just eval-trigger haiku"
            print(f"  Tip: wait for reset, run `{other}`, or use `--self-test`.", file=sys.stderr)
        return 0
    except AgentBlockError as e:
        if args.json:
            print(json.dumps({
                "skipped": True,
                "blocked": True,
                "agent": e.agent,
                "model": e.model,
                "message": e.message,
                "hint": e.hint or None,
            }, indent=2))
        else:
            print(f"[AGENT UNAVAILABLE] {e.agent} ({e.model}):", file=sys.stderr)
            print(f"  {e.message}", file=sys.stderr)
            if e.hint:
                print(f"  How to fix:\n  {e.hint}", file=sys.stderr)
        return 1

    if args.apply and best_desc != parse_skill_frontmatter(skill_md)[1]:
        replace_description_in_skill(skill_md, best_desc)

    report = {
        "mode": "per-query" if args.per_query else "batched (1 call per iteration)",
        "agent": agent,
        "model": model,
        "skill": skill_name,
        "iterations": len(history),
        "summary": best_summary,
        "results": best_results,
    }
    if args.json:
        print(json.dumps(report, indent=2))
    else:
        print(f"mode:    {report['mode']}")
        print(f"agent:   {agent} / {model}")
        print(f"skill:   {skill_name}")
        print(f"score:   {best_summary['passed']}/{best_summary['total']} (tp={best_summary['tp']} tn={best_summary['tn']} fp={best_summary['fp']} fn={best_summary['fn']})")
        print()
        for r in best_results:
            tag = "PASS" if r["passed"] else "FAIL"
            want = "trigger" if r["should_trigger"] else "skip"
            got = "trigger" if r["triggered"] else "skip"
            print(f"  [{tag}] Q{r['index']:02d} expected={want:7} observed={got:7}  {r['query'][:72]}")

    return 0 if best_summary["failed"] == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
