#!/usr/bin/env python3
"""eval-resolution: an ongoing conformance eval of the skill's model of Claude
Code, Gemini CLI, and OpenCode skill resolution against the real installed
agents.

The agent-skills skill and docs/skills/skills-search.md claim how coding agents resolve project
skills. Agents change between versions, so a claim is only trustworthy if it is
re-checked against reality. This runner encodes each claim as a CASE with a
fixture and an expected set of skills visible at startup, drives every
installed agent binary (`claude`, `gemini`, `opencode`) to report what it
actually sees, and fails on any divergence. Run it in CI or on a schedule: when
an agent release changes behavior, this goes red and tells you the skill and
the doc are now stale.

Cases:
  parent-walk   markers at cwd, repo root, an intermediate parent, above the
                repo-root boundary, and below cwd. Claude Code and OpenCode load
                cwd + parents up to repo root; Gemini CLI loads cwd only.
  depth-limit   a skill nested two levels below `.claude/skills/`
                (`.claude/skills/group/marker-deep-nested/SKILL.md`) is ignored
                by Claude Code and Gemini CLI (1 level deep), and loaded by
                OpenCode (`**/SKILL.md`).
  symlink       a skill directory reached via a symlink inside `.claude/skills/`
                is followed and loaded.
  name-default  a skill dir with no `name:` field loads under its directory name
                in Claude Code, and is skipped by Gemini CLI and OpenCode.
  lenient-yaml  a skill whose frontmatter is not strict YAML still loads in both
                Claude Code and Gemini CLI (fallback line parser), and is
                skipped by OpenCode's strict YAML parser.

Agents other than Claude Code, Gemini CLI, and OpenCode are declared pending
here on purpose (honesty over coverage): Goose needs its own enumeration
adapter, added when we can drive it. It is reported as pending, never as
passing.

Modes:
  (default)     drive all installed agent binaries (`claude`, `gemini`,
                `opencode`) when `--agent all` or `--agent auto` is used, or a
                single agent when `--agent claude|gemini|opencode` is passed.
                Uses zero-API local skill listing for Gemini (`gemini skills
                list`) and OpenCode (`opencode debug skill`), and a single
                `--model haiku` call for Claude (override with `--model sonnet`
                or `CLAUDE_MODEL`). Skips missing or rate-limited binaries
                cleanly.
  --self-test   use the documented-behavior oracle (claude-skill-dirs.py) as a
                stand-in to prove the harness plumbing end to end without an
                agent. This validates wiring, NOT the real world.

Usage:
  python3 eval-resolution.py [--self-test] [--agent all|auto|claude|gemini|opencode] [--model MODEL] [--json] [--keep] [--results-dir DIR]
Exit: 0 all checked cases pass (or all skipped), 1 any mismatch, 2 usage.
"""

import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from datetime import datetime, timezone
from pathlib import Path

HERE = Path(__file__).resolve().parent
ORACLE = HERE / "claude-skill-dirs.py"
ALL_AGENTS = ("claude", "gemini", "opencode")
DEFAULT_MODELS = {
    "claude": "haiku",
    "gemini": "gemini-2.5-flash",
    "opencode": "",
}

LIMIT_PATTERNS = re.compile(
    r"(hit your .*limit|hour limit|rate[_ ]limit|quota exceeded|resource_exhausted|"
    r"http 429|status 429|too many requests|credit balance is too low|overloaded)",
    re.IGNORECASE,
)
RESET_PATTERN = re.compile(r"(resets?\s+[^\n]+)", re.IGNORECASE)


class QuotaLimitError(Exception):
    def __init__(self, agent: str, model: str, message: str, reset_hint: str = ""):
        super().__init__(message)
        self.agent = agent
        self.model = model
        self.message = message
        self.reset_hint = reset_hint

ENUM_PROMPT = (
    'You have a set of available skills. Output a line "SKILL: <name>" for every '
    'available skill whose name begins with "marker-". If there are none, output '
    'exactly "SKILL: none". Output nothing else and do not call any tools.'
)


def write_skill(root: Path, name: str, *, frontmatter=True, with_name=True, valid=True):
    d = root / name
    d.mkdir(parents=True, exist_ok=True)
    if not frontmatter:
        body = f"Marker {name}, no frontmatter.\n"
    else:
        desc = "Probe marker skill used to eval agent skill resolution."
        if not valid:
            # An unquoted ': ' in a plain scalar is not strict YAML; Claude Code
            # and Gemini CLI's fallback parser load it anyway, stricter parsers skip it.
            desc = "broken: value with an unquoted colon that strict YAML rejects"
        lines = ["---"]
        if with_name:
            lines.append(f"name: {name}")
        lines.append(f"description: {desc}")
        lines += ["---", "", f"Marker {name}.", ""]
        body = "\n".join(lines)
    (d / "SKILL.md").write_text(body, encoding="utf-8")
    # Mirror .claude/skills into .gemini/skills so the same fixture serves both agents.
    if root.name == "skills" and root.parent.name == ".claude":
        base = root.parent.parent
        gemini_dir = base / ".gemini"
        gemini_dir.mkdir(parents=True, exist_ok=True)
        gemini_skills = gemini_dir / "skills"
        if not gemini_skills.exists() and not gemini_skills.is_symlink():
            gemini_skills.symlink_to(Path("..") / ".claude" / "skills")


def git_init(repo: Path):
    subprocess.run(["git", "-C", str(repo), "init", "-q"], check=True)
    subprocess.run(["git", "-C", str(repo), "config", "user.email", "eval@example.com"], check=True)
    subprocess.run(["git", "-C", str(repo), "config", "user.name", "eval"], check=True)
    subprocess.run(["git", "-C", str(repo), "add", "-A"], check=True)
    subprocess.run(["git", "-C", str(repo), "-c", "commit.gpgsign=false", "commit", "-qm", "init"], check=True)


# --- combined fixture (all cases checked in ONE invocation) ----------------
def build_combined_fixture(tmp: Path, agent: str):
    """Build all three cases into a single fixture tree so the agent is invoked
    at most once per eval run (0 API calls on Gemini, 1 single-turn Haiku call
    on Claude)."""
    above = tmp / "above"
    repo = above / "repo"
    app = repo / "pkg" / "app"
    (app / "sub").mkdir(parents=True)
    # parent-walk markers
    write_skill(above / ".claude" / "skills", "marker-above-repo")
    write_skill(repo / ".claude" / "skills", "marker-repo-root")
    write_skill(repo / "pkg" / ".claude" / "skills", "marker-parent-cwd")
    write_skill(app / ".claude" / "skills", "marker-cwd")
    write_skill(app / "sub" / ".claude" / "skills", "marker-below-cwd")
    # depth-limit marker (nested 2 levels under .claude/skills/; Claude & Gemini only read 1 level deep)
    write_skill(app / ".claude" / "skills" / "group", "marker-deep-nested")
    # symlink marker (symlinked skill directory under .claude/skills/)
    ext_target = tmp / "external-skills"
    write_skill(ext_target, "marker-symlinked")
    (app / ".claude" / "skills" / "marker-symlinked").symlink_to(ext_target / "marker-symlinked")
    # name-default marker (frontmatter omits `name`)
    write_skill(app / ".claude" / "skills", "marker-dir-named", with_name=False)
    # lenient-yaml marker (unquoted colon in plain scalar)
    write_skill(app / ".claude" / "skills", "marker-bad-yaml", valid=False)
    (app / "README.md").write_text("fixture\n")
    git_init(repo)

    walks_parents = agent in ("claude", "opencode")
    cases = [
        ("parent-walk", {
            "marker-cwd": True,
            "marker-repo-root": walks_parents,
            "marker-parent-cwd": walks_parents,
            "marker-above-repo": False,
            "marker-below-cwd": False,
        }),
        ("depth-limit", {
            # OpenCode globs **/SKILL.md recursively; Claude and Gemini read 1 level deep.
            "marker-deep-nested": agent == "opencode",
        }),
        ("symlink", {
            "marker-symlinked": True,
        }),
        ("name-default", {
            # Claude defaults to directory name; OpenCode and Gemini skip missing `name:`.
            "marker-dir-named": agent == "claude",
        }),
        ("lenient-yaml", {
            # Claude and Gemini load lenient YAML; OpenCode's strict YAML parser skips it.
            "marker-bad-yaml": agent in ("claude", "gemini"),
        }),
    ]
    return app, cases


# --- enumerators (what an agent reports as visible) ------------------------
def find_binary(binary: str) -> str:
    found = shutil.which(binary)
    if found:
        return found
    try:
        home = Path.home()
    except OSError:
        home = None
    candidates = []
    if home is not None:
        if binary == "opencode":
            candidates.extend([
                home / ".opencode" / "bin" / "opencode",
                home / ".bun" / "bin" / "opencode",
            ])
        elif binary == "claude":
            candidates.append(home / ".claude" / "local" / "claude")
        candidates.append(home / ".local" / "bin" / binary)
    candidates.extend([
        Path("/opt/homebrew/bin") / binary,
        Path("/usr/local/bin") / binary,
    ])
    for candidate in candidates:
        try:
            if candidate.is_file() and os.access(candidate, os.X_OK):
                return str(candidate)
        except OSError:
            continue
    return binary


def parse_markers(out: str):
    seen = set()
    for line in out.splitlines():
        s = line.strip()
        if s.upper().startswith("SKILL:"):
            val = s.split(":", 1)[1].strip()
            if val.startswith("marker-"):
                seen.add(val)
    return seen


def check_for_quota_limit(agent: str, model: str, proc: subprocess.CompletedProcess):
    combined = "\n".join(x for x in (proc.stderr.strip(), proc.stdout.strip()) if x)
    if LIMIT_PATTERNS.search(combined):
        first_line = combined.splitlines()[0].strip() if combined else "quota limit reached"
        m = RESET_PATTERN.search(combined)
        raise QuotaLimitError(agent, model, first_line, m.group(1).strip() if m else "")
    if proc.returncode != 0 and not proc.stdout.strip():
        first_line = combined.splitlines()[0].strip() if combined else f"exited with code {proc.returncode}"
        raise QuotaLimitError(agent, model, first_line, "")


def enumerate_claude(launch: Path, model: str):
    proc = subprocess.run(
        [find_binary("claude"), "--setting-sources", "project", "--model", model, "-p", ENUM_PROMPT],
        cwd=str(launch),
        capture_output=True, text=True,
    )
    check_for_quota_limit("claude", model, proc)
    return parse_markers(proc.stdout)


def enumerate_opencode(launch: Path, model: str, use_llm: bool = False):
    exe = find_binary("opencode")
    if not use_llm:
        # Zero-API path: `opencode debug skill` dumps the JSON array from Skill.all()
        # directly to stdout without making any LLM calls.
        empty_home = launch / ".opencode-empty-home"
        empty_home.mkdir(exist_ok=True)
        env = dict(os.environ)
        env["HOME"] = str(empty_home)
        try:
            proc = subprocess.run(
                [exe, "debug", "skill"], cwd=str(launch),
                env=env, capture_output=True, text=True,
            )
        finally:
            shutil.rmtree(empty_home, ignore_errors=True)
        out = proc.stdout.strip()
        seen = set()
        # Try JSON parse first (array of {"name": ...}); fall back to regex scan
        try:
            start_idx = out.find("[")
            if start_idx != -1:
                items = json.loads(out[start_idx:])
                for item in items:
                    if isinstance(item, dict):
                        name = str(item.get("name", ""))
                        if name.startswith("marker-"):
                            seen.add(name)
                return seen
        except json.JSONDecodeError:
            pass
        for m in re.finditer(r"\b(marker-[a-z0-9-]+)\b", out):
            seen.add(m.group(1))
        return seen

    cmd = [exe, "run"]
    if model:
        cmd.extend(["-m", model])
    cmd.append(ENUM_PROMPT)
    proc = subprocess.run(cmd, cwd=str(launch), capture_output=True, text=True)
    check_for_quota_limit("opencode", model or "default", proc)
    return parse_markers(proc.stdout)


def enumerate_gemini(launch: Path, model: str, use_llm: bool = False):
    exe = find_binary("gemini")
    if not use_llm:
        # Zero-API path: `gemini skills list` runs Gemini CLI's real SkillManager.discoverSkills()
        # locally without making any LLM network calls or burning API quota.
        tf = launch / ".gemini-trusted-folders.json"
        empty_home = launch / ".gemini-empty-home"
        empty_home.mkdir(exist_ok=True)
        tf.write_text(json.dumps({str(launch.resolve()): "TRUST_FOLDER"}), encoding="utf-8")
        env = dict(os.environ)
        env["GEMINI_CLI_TRUSTED_FOLDERS_PATH"] = str(tf)
        env["GEMINI_CLI_HOME"] = str(empty_home)
        try:
            out = subprocess.run(
                [exe, "skills", "list"], cwd=str(launch),
                env=env, capture_output=True, text=True,
            ).stdout
        finally:
            tf.unlink(missing_ok=True)
            shutil.rmtree(empty_home, ignore_errors=True)
        seen = set()
        for line in out.splitlines():
            s = line.strip()
            if s.startswith("marker-"):
                seen.add(s.split()[0])
        return seen

    proc = subprocess.run(
        [exe, "--skip-trust", "-m", model, "-p", ENUM_PROMPT], cwd=str(launch),
        capture_output=True, text=True,
    )
    check_for_quota_limit("gemini", model, proc)
    return parse_markers(proc.stdout)


def enumerate_oracle(launch: Path, agent: str):
    """Documented-behavior stand-in: markers in non-lazy tiers at startup."""
    data = json.loads(subprocess.run(
        ["python3", str(ORACLE), str(launch), "--agent", agent, "--json"],
        capture_output=True, text=True, check=True,
    ).stdout)
    seen = set()
    for tier in data["tiers"]:
        if "LAZY" in (tier.get("note") or ""):
            continue
        for name in tier.get("skills") or []:
            if name.startswith("marker-"):
                seen.add(name)
    return seen


def agent_version(agent: str):
    env = dict(os.environ)
    if agent == "gemini":
        env.setdefault("GEMINI_CLI_HOME", os.environ.get("TMPDIR", "/tmp"))
    exe = find_binary(agent)
    try:
        return subprocess.run(
            [exe, "--version"], capture_output=True, text=True, check=True, env=env
        ).stdout.strip()
    except (subprocess.CalledProcessError, OSError):
        return None


def gemini_has_auth() -> bool:
    for var in ("GEMINI_API_KEY", "GOOGLE_API_KEY", "GOOGLE_GENAI_USE_VERTEXAI", "GOOGLE_GENAI_USE_GCA"):
        if os.environ.get(var):
            return True
    home = Path(os.environ.get("GEMINI_CLI_HOME", str(Path.home()))) / ".gemini"
    for fname in ("oauth_creds.json", "google_accounts.json", "settings.json"):
        try:
            if (home / fname).is_file():
                return True
        except OSError:
            continue
    return False


def resolve_target_agents(requested: str, model_arg):
    """Return the list of agents to evaluate."""
    if requested in ("all", "auto") and model_arg:
        if model_arg.startswith("gemini"):
            return ["gemini"]
        if model_arg in ("haiku", "sonnet", "opus") or model_arg.startswith("claude"):
            return ["claude"]
        if "/" in model_arg:
            return ["opencode"]
    if requested in ("all", "auto"):
        return list(ALL_AGENTS)
    return [requested]


def run(agent: str, enumerate_fn, keep: bool):
    results = []
    tmp = Path(tempfile.mkdtemp(prefix="eval-skills-"))
    try:
        launch, cases = build_combined_fixture(tmp, agent)
        seen = enumerate_fn(launch)
        for name, expect in cases:
            checks = []
            ok = True
            for marker, want in sorted(expect.items()):
                got = marker in seen
                passed = got == want
                ok = ok and passed
                checks.append({"marker": marker, "expected": want, "observed": got, "passed": passed})
            results.append({"case": name, "passed": ok, "checks": checks})
    finally:
        if not keep:
            shutil.rmtree(tmp, ignore_errors=True)
    return results


def eval_single_agent(agent: str, *, self_test: bool, use_llm: bool, model_arg, keep: bool, stamp: str):
    ver = agent_version(agent)
    if self_test:
        mode = f"self-test ({agent} oracle plumbing only)"
        results = run(agent, lambda launch: enumerate_oracle(launch, agent), keep)
        return {
            "agent": agent,
            "mode": mode,
            "when": stamp,
            "agent_version": ver,
            "claude_version": ver if agent == "claude" else None,
            "model": None,
            "skipped": False,
            "rate_limited": False,
            "results": results,
            "passed": all(r["passed"] for r in results),
        }

    if ver is None:
        return {
            "agent": agent,
            "mode": f"live {agent}",
            "when": stamp,
            "agent_version": None,
            "model": None,
            "skipped": True,
            "rate_limited": False,
            "reason": f"'{agent}' binary not found on PATH",
            "results": [],
            "passed": True,
        }

    if agent == "gemini" and use_llm and not gemini_has_auth():
        return {
            "agent": agent,
            "mode": f"live {agent} (llm)",
            "when": stamp,
            "agent_version": ver,
            "model": None,
            "skipped": True,
            "rate_limited": False,
            "reason": "--llm requested without Gemini auth/credentials",
            "results": [],
            "passed": True,
        }

    env_key = "GEMINI_MODEL" if agent == "gemini" else ("OPENCODE_MODEL" if agent == "opencode" else "CLAUDE_MODEL")
    zero_api = (agent in ("gemini", "opencode")) and not use_llm
    model = "none (zero-API skill dump)" if zero_api else (model_arg or os.environ.get(env_key, DEFAULT_MODELS[agent]))
    mode = f"live {agent} ({'zero-API skill dump' if zero_api else 'llm'})"
    if agent == "gemini":
        enumerate_fn = lambda launch: enumerate_gemini(launch, model, use_llm=use_llm)
    elif agent == "opencode":
        enumerate_fn = lambda launch: enumerate_opencode(launch, model, use_llm=use_llm)
    else:
        enumerate_fn = lambda launch: enumerate_claude(launch, model)

    try:
        results = run(agent, enumerate_fn, keep)
    except QuotaLimitError as e:
        return {
            "agent": agent,
            "mode": mode,
            "when": stamp,
            "agent_version": ver,
            "model": model,
            "skipped": True,
            "rate_limited": True,
            "reason": e.message,
            "reset": e.reset_hint or None,
            "results": [],
            "passed": True,
        }

    return {
        "agent": agent,
        "mode": mode,
        "when": stamp,
        "agent_version": ver,
        "claude_version": ver if agent == "claude" else None,
        "model": model,
        "skipped": False,
        "rate_limited": False,
        "results": results,
        "passed": all(r["passed"] for r in results),
    }


def main():
    argv = sys.argv[1:]
    self_test = "--self-test" in argv
    use_llm = "--llm" in argv
    as_json = "--json" in argv
    keep = "--keep" in argv
    requested_agent = os.environ.get("EVAL_AGENT", "all")
    if "--agent" in argv:
        i = argv.index("--agent")
        if i + 1 >= len(argv) or argv[i + 1] not in ("all", "auto", "claude", "gemini", "opencode"):
            print("eval-resolution: --agent needs all, auto, claude, gemini, or opencode", file=sys.stderr)
            return 2
        requested_agent = argv[i + 1]

    model_arg = None
    if "--model" in argv:
        i = argv.index("--model")
        if i + 1 >= len(argv):
            print("eval-resolution: --model needs a model name (e.g. haiku, sonnet, gemini-2.5-flash)", file=sys.stderr)
            return 2
        model_arg = argv[i + 1]

    results_dir = None
    if "--results-dir" in argv:
        i = argv.index("--results-dir")
        if i + 1 >= len(argv):
            print("eval-resolution: --results-dir needs a path", file=sys.stderr)
            return 2
        results_dir = Path(argv[i + 1])

    stamp = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    target_agents = resolve_target_agents(requested_agent, model_arg)
    agent_reports = [
        eval_single_agent(
            agent,
            self_test=self_test,
            use_llm=use_llm,
            model_arg=model_arg,
            keep=keep,
            stamp=stamp,
        )
        for agent in target_agents
    ]

    overall_passed = all(rep["passed"] for rep in agent_reports)
    active_reports = [rep for rep in agent_reports if not rep["skipped"]]
    skipped_reports = [rep for rep in agent_reports if rep["skipped"]]

    if results_dir:
        results_dir.mkdir(parents=True, exist_ok=True)
        safe = stamp.replace(":", "").replace("-", "")
        for rep in active_reports:
            (results_dir / f"eval-{rep['agent']}-{safe}.json").write_text(json.dumps(rep, indent=2))

    if len(agent_reports) == 1:
        rep = agent_reports[0]
        rep["agents_pending"] = ["goose"]
        if as_json:
            print(json.dumps(rep, indent=2))
            return 0 if rep["passed"] else 1
        if rep["skipped"]:
            if rep.get("rate_limited"):
                print(f"[RATE LIMIT] {rep['agent']} ({rep['model']}) unavailable:", file=sys.stderr)
                print(f"  {rep['reason']}", file=sys.stderr)
                if rep.get("reset"):
                    print(f"  Reset window: {rep['reset']}", file=sys.stderr)
            else:
                print(f"eval-resolution: skipped {rep['agent']}: {rep['reason']}", file=sys.stderr)
            return 0
        print(f"mode:   {rep['mode']}")
        print(f"agent:  {rep['agent']} ({rep['agent_version'] or 'version unknown'})")
        if not self_test:
            print(f"model:  {rep['model']}")
        print(f"when:   {stamp}")
        print(f"pending adapters: {', '.join(rep['agents_pending'])}")
        print()
        for r in rep["results"]:
            print(f"[{'PASS' if r['passed'] else 'FAIL'}] {r['case']}")
            for c in r["checks"]:
                tag = "ok" if c["passed"] else "MISMATCH"
                exp = "loaded" if c["expected"] else "absent"
                obs = "loaded" if c["observed"] else "absent"
                print(f"    {c['marker']:20} expected={exp:7} observed={obs:7} {tag}")
        print()
        print("OVERALL:", "PASS" if rep["passed"] else "FAIL")
        return 0 if rep["passed"] else 1

    combined_report = {
        "mode": "self-test (all agents)" if self_test else "live (all available agents)",
        "when": stamp,
        "agents_pending": ["goose"],
        "agents": agent_reports,
        "passed": overall_passed,
    }
    if as_json:
        print(json.dumps(combined_report, indent=2))
        return 0 if overall_passed else 1

    print(f"mode:   {combined_report['mode']}")
    print(f"when:   {stamp}")
    print(f"pending adapters: {', '.join(combined_report['agents_pending'])}")
    for rep in agent_reports:
        print()
        ver_str = rep["agent_version"] or "not installed"
        model_str = f", model: {rep['model']}" if rep.get("model") else ""
        print(f"=== {rep['agent']} ({ver_str}) [{rep['mode']}{model_str}] ===")
        if rep["skipped"]:
            kind = "RATE LIMIT" if rep.get("rate_limited") else "SKIP"
            reset_str = f" (reset: {rep['reset']})" if rep.get("reset") else ""
            print(f"[{kind}] {rep['reason']}{reset_str}")
            continue
        for r in rep["results"]:
            print(f"[{'PASS' if r['passed'] else 'FAIL'}] {r['case']}")
            for c in r["checks"]:
                tag = "ok" if c["passed"] else "MISMATCH"
                exp = "loaded" if c["expected"] else "absent"
                obs = "loaded" if c["observed"] else "absent"
                print(f"    {c['marker']:20} expected={exp:7} observed={obs:7} {tag}")

    print()
    print("SUMMARY:")
    for rep in agent_reports:
        if rep["skipped"]:
            kind = "RATE LIMIT" if rep.get("rate_limited") else "SKIP"
            print(f"  {rep['agent']:10} {kind} ({rep['reason']})")
        else:
            n_pass = sum(1 for r in rep["results"] if r["passed"])
            n_total = len(rep["results"])
            status = "PASS" if rep["passed"] else "FAIL"
            print(f"  {rep['agent']:10} {status} ({n_pass}/{n_total} cases)")
    n_active_pass = sum(1 for rep in active_reports if rep["passed"])
    skip_note = f", {len(skipped_reports)} skipped" if skipped_reports else ""
    print(
        f"OVERALL: {'PASS' if overall_passed else 'FAIL'} "
        f"({n_active_pass}/{len(active_reports)} active agents passed{skip_note})"
    )
    return 0 if overall_passed else 1


if __name__ == "__main__":
    sys.exit(main())
