#!/usr/bin/env python3
"""claude-skill-dirs: predict where Claude Code looks for skills from a directory.

The deterministic oracle for Claude Code skill resolution: given a starting
directory, print the `.claude/skills` roots Claude Code's documented algorithm
would search, in precedence order, and the skills each holds. This is the
"what SHOULD load" reference the live probe (probe-claude-resolution.sh) checks
the real binary against.

It encodes the documented rules only, and marks what is version-sensitive or
assumed rather than pretending certainty:

  - managed (policy) skills, OS-specific path, unless CLAUDE_CODE_DISABLE_POLICY_SKILLS
  - personal  ~/.claude/skills (+ /synced)
  - project   .claude/skills from the start dir up to the repository root
  - nested    .claude/skills below the start dir load LAZILY (not at startup)

The project parent-walk is a recent, version-sensitive feature; this tool
predicts the documented current behavior and flags it, it does not assert your
installed build does it. Run the live probe to find out.

No network, stdlib only. Usage:
    python3 claude-skill-dirs.py [START_DIR] [--agent claude|gemini] [--json]
START_DIR defaults to the current directory.
"""

import json
import os
import platform
import subprocess
import sys
from pathlib import Path


def repo_root(start: Path):
    """git toplevel containing start, or None outside a repo."""
    try:
        out = subprocess.run(
            ["git", "-C", str(start), "rev-parse", "--show-toplevel"],
            capture_output=True, text=True, check=True,
        ).stdout.strip()
        return Path(out) if out else None
    except (subprocess.CalledProcessError, FileNotFoundError):
        return None


def managed_root():
    """OS-specific managed-policy skills root, or None if disabled."""
    if os.environ.get("CLAUDE_CODE_DISABLE_POLICY_SKILLS"):
        return None
    if platform.system() == "Darwin":
        return Path("/Library/Application Support/ClaudeCode/.claude/skills")
    return Path("/etc/claude-code/.claude/skills")


def personal_roots():
    """~/.claude/skills and its synced subdir. CLAUDE_CONFIG_DIR may move this;
    the docs are silent, so we note it but follow the documented home path."""
    home = Path(os.environ.get("CLAUDE_CONFIG_DIR", str(Path.home() / ".claude")))
    base = home / "skills"
    return [base, base / "synced"]


def skills_in(root: Path):
    """Skill names under a root: each child dir holding a SKILL.md."""
    try:
        if not root.is_dir():
            return None  # root absent
        out = []
        for child in sorted(root.iterdir()):
            if child.is_dir() and (child / "SKILL.md").exists():
                out.append(child.name)
        return out
    except OSError:
        return None


def project_walk(start: Path, root):
    """Start dir and each parent up to the repository root (inclusive).
    Outside a repo the walk is just the start dir (the `the` assumption)."""
    dirs = [start]
    if root is not None:
        cur = start
        while cur != root and cur.parent != cur:
            cur = cur.parent
            dirs.append(cur)
    return dirs  # nearest first


def nested_below(start: Path, limit=200):
    """`.claude/skills` dirs strictly below start (lazy-loaded, informational)."""
    found = []
    for i, p in enumerate(start.rglob(".claude/skills")):
        if i >= limit:
            break
        if p.is_dir() and p.resolve() != (start / ".claude" / "skills").resolve():
            found.append(p)
    return sorted(found)


def find_binary(binary: str) -> str:
    import shutil
    found = shutil.which(binary)
    if found:
        return found
    if binary == "opencode":
        for candidate in (
            Path.home() / ".opencode" / "bin" / "opencode",
            Path.home() / ".bun" / "bin" / "opencode",
            Path.home() / ".local" / "bin" / "opencode",
        ):
            try:
                if candidate.is_file():
                    return str(candidate)
            except OSError:
                continue
    return binary


def cli_version(binary: str):
    env = dict(os.environ)
    if binary == "gemini":
        env.setdefault("GEMINI_CLI_HOME", os.environ.get("TMPDIR", "/tmp"))
    exe = find_binary(binary)
    try:
        return subprocess.run(
            [exe, "--version"], capture_output=True, text=True, check=True, env=env
        ).stdout.strip()
    except (subprocess.CalledProcessError, OSError):
        return None


def gemini_loads_skill(skill_md: Path) -> bool:
    """Gemini CLI requires frontmatter with both name: and description: lines."""
    try:
        text = skill_md.read_text(encoding="utf-8")
    except OSError:
        return False
    text = text.lstrip("\ufeff")
    if not text.startswith("---"):
        return False
    parts = text.split("\n---", 1)
    if len(parts) < 2:
        return False
    fm = parts[0].splitlines()[1:]
    has_name = any(line.lstrip().lower().startswith("name:") for line in fm)
    has_desc = any(line.lstrip().lower().startswith("description:") for line in fm)
    return has_name and has_desc


def opencode_loads_skill(skill_md: Path) -> str | None:
    """OpenCode requires valid YAML frontmatter with a string `name:` and optional string `description:`."""
    try:
        text = skill_md.read_text(encoding="utf-8")
    except OSError:
        return None
    text = text.lstrip("\ufeff")
    if not text.startswith("---"):
        return None
    parts = text.split("\n---", 1)
    if len(parts) < 2:
        return None
    fm = parts[0].splitlines()[1:]
    name_val = None
    for line in fm:
        s = line.strip()
        if not s or s.startswith("#"):
            continue
        if ":" in s:
            k, v = s.split(":", 1)
            val = v.strip()
            # Unquoted ': ' inside a plain scalar is rejected by strict YAML
            if ": " in val and not (val.startswith('"') or val.startswith("'") or val in (">", ">-", "|", "|-")):
                return None
            if k.strip().lower() == "name" and val:
                name_val = val.strip("\"'")
    return name_val


def skills_in_for(root: Path, agent: str):
    try:
        if not root.is_dir():
            return None
        out = []
        if agent == "opencode":
            # OpenCode globs **/SKILL.md recursively under each root, following symlinks
            visited = set()

            def walk_dir(d: Path):
                try:
                    real = d.resolve()
                except OSError:
                    return
                if real in visited:
                    return
                visited.add(real)
                try:
                    entries = sorted(d.iterdir())
                except OSError:
                    return
                for p in entries:
                    if p.is_dir():
                        walk_dir(p)
                    elif p.name == "SKILL.md" and p.is_file():
                        loaded_name = opencode_loads_skill(p)
                        if loaded_name:
                            out.append(loaded_name)

            walk_dir(root)
            return sorted(set(out))

        for child in sorted(root.iterdir()):
            skill_md = child / "SKILL.md"
            if child.is_dir() and skill_md.exists():
                if agent == "gemini":
                    if not gemini_loads_skill(skill_md):
                        continue
                out.append(child.name)
        return out
    except OSError:
        return None


def build_claude(start: Path):
    start = start.resolve()
    root = repo_root(start)
    tiers = []

    mr = managed_root()
    if mr is not None:
        tiers.append({"scope": "managed", "root": str(mr),
                      "skills": skills_in(mr), "note": "policy; OS path"})

    for pr in personal_roots():
        tiers.append({"scope": "user", "root": str(pr),
                      "skills": skills_in(pr), "note": ""})

    for d in project_walk(start, root):
        sr = d / ".claude" / "skills"
        note = "start dir" if d == start else "parent (version-sensitive walk)"
        tiers.append({"scope": "project", "root": str(sr),
                      "skills": skills_in(sr), "note": note})

    for p in nested_below(start):
        tiers.append({"scope": "project", "root": str(p),
                      "skills": skills_in(p), "note": "below start: LAZY, not at startup"})

    ver = cli_version("claude")
    return {
        "agent": "claude",
        "start": str(start),
        "repo_root": str(root) if root else None,
        "claude_version": ver,
        "agent_version": ver,
        "tiers": tiers,
    }


def build_gemini(start: Path):
    start = start.resolve()
    root = repo_root(start)
    home = Path(os.environ.get("GEMINI_CLI_HOME", str(Path.home())))
    tiers = [
        {
            "scope": "project",
            "root": str(start / ".agents" / "skills"),
            "skills": skills_in_for(start / ".agents" / "skills", "gemini"),
            "note": "start dir (.agents/skills alias; highest workspace precedence)",
        },
        {
            "scope": "project",
            "root": str(start / ".gemini" / "skills"),
            "skills": skills_in_for(start / ".gemini" / "skills", "gemini"),
            "note": "start dir (cwd only; no parent walk)",
        },
        {
            "scope": "user",
            "root": str(home / ".agents" / "skills"),
            "skills": skills_in_for(home / ".agents" / "skills", "gemini"),
            "note": "user .agents/skills alias",
        },
        {
            "scope": "user",
            "root": str(home / ".gemini" / "skills"),
            "skills": skills_in_for(home / ".gemini" / "skills", "gemini"),
            "note": "user .gemini/skills",
        },
    ]
    ver = cli_version("gemini")
    return {
        "agent": "gemini",
        "start": str(start),
        "repo_root": str(root) if root else None,
        "claude_version": None,
        "agent_version": ver,
        "tiers": tiers,
    }


def build_opencode(start: Path):
    start = start.resolve()
    root = repo_root(start)
    home = Path.home()
    tiers = [
        {
            "scope": "user",
            "root": str(home / ".claude" / "skills"),
            "skills": skills_in_for(home / ".claude" / "skills", "opencode"),
            "note": "user ~/.claude/skills (recursive **/SKILL.md)",
        },
        {
            "scope": "user",
            "root": str(home / ".agents" / "skills"),
            "skills": skills_in_for(home / ".agents" / "skills", "opencode"),
            "note": "user ~/.agents/skills (recursive **/SKILL.md)",
        },
    ]
    for d in project_walk(start, root):
        for dot in (".claude", ".agents"):
            sr = d / dot / "skills"
            note = "start dir" if d == start else "parent (walk to git worktree; recursive **/SKILL.md)"
            tiers.append({
                "scope": "project",
                "root": str(sr),
                "skills": skills_in_for(sr, "opencode"),
                "note": note,
            })
        for sub in ("skill", "skills"):
            oc = d / ".opencode" / sub
            if oc.is_dir():
                tiers.append({
                    "scope": "project",
                    "root": str(oc),
                    "skills": skills_in_for(oc, "opencode"),
                    "note": ".opencode walk",
                })
    ver = cli_version("opencode")
    return {
        "agent": "opencode",
        "start": str(start),
        "repo_root": str(root) if root else None,
        "claude_version": None,
        "agent_version": ver,
        "tiers": tiers,
    }


def resolve_agent(requested: str) -> str:
    if requested in ("claude", "gemini", "opencode"):
        return requested
    for cand in ("claude", "gemini", "opencode"):
        if cli_version(cand) is not None:
            return cand
    return "claude"


def main():
    argv = sys.argv[1:]
    as_json = "--json" in argv
    agent = os.environ.get("EVAL_AGENT", "claude")
    cleaned = []
    i = 0
    while i < len(argv):
        if argv[i] == "--json":
            i += 1
        elif argv[i] == "--agent":
            if i + 1 >= len(argv):
                print("claude-skill-dirs: --agent needs claude, gemini, opencode, or auto", file=sys.stderr)
                return 2
            agent = argv[i + 1]
            i += 2
        else:
            cleaned.append(argv[i])
            i += 1

    agent = resolve_agent(agent)
    start = Path(cleaned[0]) if cleaned else Path.cwd()
    if not start.exists():
        print(f"claude-skill-dirs: {start}: no such directory", file=sys.stderr)
        return 2

    if agent == "gemini":
        data = build_gemini(start)
    elif agent == "opencode":
        data = build_opencode(start)
    else:
        data = build_claude(start)
    if as_json:
        print(json.dumps(data, indent=2))
        return 0

    print(f"agent:     {data['agent']}")
    print(f"start:     {data['start']}")
    print(f"repo root: {data['repo_root'] or '(none; project walk = start dir only)'}")
    print(f"version:   {data['agent_version'] or '(not on PATH; version unknown)'}")
    print()
    print(f"{'SCOPE':8} {'SKILLS':30} ROOT")
    for t in data["tiers"]:
        if t["skills"] is None:
            skills = "(absent)"
        elif not t["skills"]:
            skills = "(empty)"
        else:
            skills = ", ".join(t["skills"])
        if len(skills) > 29:
            skills = skills[:26] + "..."
        print(f"{t['scope']:8} {skills:30} {t['root']}")
        if t["note"]:
            print(f"{'':8} {'':30}   = {t['note']}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
