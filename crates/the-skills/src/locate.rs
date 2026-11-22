//! Where each agent looks for skills, imitated from its source or official docs. Each agent's roots
//! are listed in that agent's precedence order (see `resolve`).
//!
//! - Claude Code (docs: code.claude.com/docs/en/skills): managed `/etc/claude-code/.claude/skills`
//!   (Linux) or `/Library/Application Support/ClaudeCode/.claude/skills` (macOS), unless
//!   `CLAUDE_CODE_DISABLE_POLICY_SKILLS`; `~/.claude/skills` and `~/.claude/skills/synced`; then
//!   `.claude/skills` in the cwd and each parent up to the repository root. One level deep:
//!   `<root>/<name>/SKILL.md`. Precedence: managed, then personal, then project. (Docs do not say
//!   where the walk ends outside a repository; we search only the cwd there. Nearest project wins
//!   within the project tier, also undocumented.)
//! - OpenCode v1.19.5 (`packages/opencode/src/skill/index.ts`, `config/paths.ts`): `~/.claude` and
//!   `~/.agents` `skills/**`, then the same two in the cwd and each parent up to the git worktree
//!   (filesystem root outside git), unless `OPENCODE_DISABLE_EXTERNAL_SKILLS` (and `.claude` unless
//!   `OPENCODE_DISABLE_CLAUDE_CODE[_SKILLS]`); then each config dir's `{skill,skills}/**` without
//!   dot-directories: `$XDG_CONFIG_HOME/opencode`, `.opencode` up to the worktree (unless
//!   `OPENCODE_DISABLE_PROJECT_CONFIG`), `~/.opencode`, `$OPENCODE_CONFIG_DIR`. Recursive, follows
//!   symlinks. Config `skills.paths`/`skills.urls` are not imitated yet.
//! - Goose (`crates/goose/src/skills/mod.rs` at 846cbeaf): `<cwd>/.agents/skills`,
//!   `<cwd>/.goose/skills`, `<cwd>/.claude/skills` (no parent walk), then `~/.agents/skills`,
//!   `<goose config>/skills`, `~/.claude/skills`, `~/.config/agents/skills`. Recursive, follows
//!   symlinks, skips `.git`/`.hg`/`.svn`. `<goose config>` is `$GOOSE_PATH_ROOT/config` when that is
//!   absolute, else `$XDG_CONFIG_HOME/goose` (when absolute) or `~/.config/goose`. Plugin skill dirs
//!   are not imitated yet.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::{Agent, Scope};

/// The environment discovery runs in, captured once so it can be tested without touching the process.
#[derive(Debug, Clone)]
pub struct Env {
    pub cwd: PathBuf,
    pub home: Option<PathBuf>,
    pub os: &'static str,
    pub vars: HashMap<String, String>,
}

impl Env {
    pub fn from_process() -> std::io::Result<Self> {
        let vars: HashMap<String, String> = std::env::vars().collect();
        let home = vars
            .get("HOME")
            .filter(|h| !h.is_empty())
            .map(PathBuf::from);
        Ok(Self {
            cwd: std::env::current_dir()?,
            home,
            os: std::env::consts::OS,
            vars,
        })
    }

    fn var(&self, k: &str) -> Option<&str> {
        self.vars
            .get(k)
            .map(String::as_str)
            .filter(|v| !v.is_empty())
    }

    /// OpenCode `Flag.truthy` and Claude Code's `=1` convention.
    fn truthy(&self, k: &str) -> bool {
        matches!(self.var(k), Some("true" | "1"))
    }

    /// Effect's `Config.boolean`, used by OpenCode's runtime flags.
    fn effect_bool(&self, k: &str) -> bool {
        matches!(self.var(k), Some("true" | "yes" | "on" | "1" | "y"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Depth {
    /// `<root>/<name>/SKILL.md` only.
    OneLevel,
    /// `SKILL.md` at any depth below the root.
    Recursive,
}

/// Which hidden (dot) entries a recursive scan descends into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hidden {
    Include,
    /// Everything except `.git`, `.hg`, `.svn` (Goose).
    SkipVcs,
    /// No dot entries (OpenCode's glob without `dot`).
    Exclude,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchRoot {
    /// `None` for a `--dir` root, which no agent searches.
    pub agent: Option<Agent>,
    pub scope: Scope,
    pub path: PathBuf,
    pub depth: Depth,
    pub hidden: Hidden,
}

fn root(
    agent: Option<Agent>,
    scope: Scope,
    path: PathBuf,
    depth: Depth,
    hidden: Hidden,
) -> SearchRoot {
    SearchRoot {
        agent,
        scope,
        path,
        depth,
        hidden,
    }
}

/// Every root to search for the given `agents` (each in its precedence order), then `--dir` roots.
pub fn plan_for(env: &Env, agents: &[Agent], extra: &[PathBuf]) -> Vec<SearchRoot> {
    let mut roots = Vec::new();
    for agent in agents {
        match agent {
            Agent::Claude => roots.extend(claude(env)),
            Agent::Gemini => roots.extend(gemini(env)),
            Agent::Opencode => roots.extend(opencode(env)),
            Agent::Goose => roots.extend(goose(env)),
        }
    }
    for d in extra {
        let path = if d.is_absolute() {
            d.clone()
        } else {
            env.cwd.join(d)
        };
        roots.push(root(
            None,
            Scope::Dir,
            path,
            Depth::OneLevel,
            Hidden::Include,
        ));
    }
    roots
}

/// Every root to search across all known agents, then `--dir` roots.
pub fn plan(env: &Env, extra: &[PathBuf]) -> Vec<SearchRoot> {
    plan_for(env, &Agent::ALL, extra)
}

fn git_root(cwd: &Path) -> Option<&Path> {
    cwd.ancestors().find(|d| d.join(".git").exists())
}

/// The cwd and each parent up to and including `stop`.
fn up_to<'a>(cwd: &'a Path, stop: Option<&Path>) -> Vec<&'a Path> {
    let mut out = Vec::new();
    for d in cwd.ancestors() {
        out.push(d);
        if Some(d) == stop {
            break;
        }
    }
    out
}

fn claude(env: &Env) -> Vec<SearchRoot> {
    let a = Some(Agent::Claude);
    let one = |scope, path| root(a, scope, path, Depth::OneLevel, Hidden::Include);
    let mut out = Vec::new();
    if !env.truthy("CLAUDE_CODE_DISABLE_POLICY_SKILLS") {
        let managed = match env.os {
            "linux" => Some(PathBuf::from("/etc/claude-code")),
            "macos" => Some(PathBuf::from("/Library/Application Support/ClaudeCode")),
            _ => None,
        };
        if let Some(m) = managed {
            out.push(one(Scope::System, m.join(".claude/skills")));
        }
    }
    if let Some(home) = &env.home {
        out.push(one(Scope::User, home.join(".claude/skills")));
        out.push(one(Scope::User, home.join(".claude/skills/synced")));
    }
    let project: Vec<&Path> = match git_root(&env.cwd) {
        Some(top) => up_to(&env.cwd, Some(top)),
        None => vec![env.cwd.as_path()],
    };
    for d in project {
        out.push(one(Scope::Project, d.join(".claude/skills")));
    }
    out
}

fn gemini(env: &Env) -> Vec<SearchRoot> {
    let a = Some(Agent::Gemini);
    let one = |scope, path| root(a, scope, path, Depth::OneLevel, Hidden::Include);
    let mut out = vec![
        one(Scope::Project, env.cwd.join(".agents/skills")),
        one(Scope::Project, env.cwd.join(".gemini/skills")),
    ];
    let home = env
        .var("GEMINI_CLI_HOME")
        .map(PathBuf::from)
        .or_else(|| env.home.clone());
    if let Some(h) = home {
        out.push(one(Scope::User, h.join(".agents/skills")));
        out.push(one(Scope::User, h.join(".gemini/skills")));
    }
    out
}

fn opencode(env: &Env) -> Vec<SearchRoot> {
    let a = Some(Agent::Opencode);
    let ext = |scope, path| root(a, scope, path, Depth::Recursive, Hidden::Include);
    let cfg = |scope, dir: &Path| {
        ["skill", "skills"].map(|s| root(a, scope, dir.join(s), Depth::Recursive, Hidden::Exclude))
    };
    // The worktree; outside git OpenCode uses "/", i.e. walks to the filesystem root.
    let walk = up_to(&env.cwd, git_root(&env.cwd));
    let mut out = Vec::new();

    if !env.effect_bool("OPENCODE_DISABLE_EXTERNAL_SKILLS") {
        let no_claude = env.effect_bool("OPENCODE_DISABLE_CLAUDE_CODE")
            || env.effect_bool("OPENCODE_DISABLE_CLAUDE_CODE_SKILLS");
        let targets: &[&str] = if no_claude {
            &[".agents"]
        } else {
            &[".claude", ".agents"]
        };
        if let Some(home) = &env.home {
            for t in targets {
                out.push(ext(Scope::User, home.join(t).join("skills")));
            }
        }
        for d in &walk {
            for t in targets {
                out.push(ext(Scope::Project, d.join(t).join("skills")));
            }
        }
    }

    let xdg_config = env
        .var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| env.home.as_ref().map(|h| h.join(".config")));
    if let Some(x) = xdg_config {
        out.extend(cfg(Scope::User, &x.join("opencode")));
    }
    if !env.truthy("OPENCODE_DISABLE_PROJECT_CONFIG") {
        for d in &walk {
            out.extend(cfg(Scope::Project, &d.join(".opencode")));
        }
    }
    if let Some(home) = &env.home {
        out.extend(cfg(Scope::User, &home.join(".opencode")));
    }
    if let Some(dir) = env.var("OPENCODE_CONFIG_DIR") {
        out.extend(cfg(Scope::User, Path::new(dir)));
    }
    out
}

fn goose(env: &Env) -> Vec<SearchRoot> {
    let a = Some(Agent::Goose);
    let rec = |scope, path| root(a, scope, path, Depth::Recursive, Hidden::SkipVcs);
    let mut out: Vec<SearchRoot> = [".agents", ".goose", ".claude"]
        .iter()
        .map(|d| rec(Scope::Project, env.cwd.join(d).join("skills")))
        .collect();

    let absolute = |v: Option<&str>| v.map(PathBuf::from).filter(|p| p.is_absolute());
    let config = match absolute(env.var("GOOSE_PATH_ROOT")) {
        Some(r) => Some(r.join("config")),
        None => absolute(env.var("XDG_CONFIG_HOME"))
            .or_else(|| env.home.as_ref().map(|h| h.join(".config")))
            .map(|c| c.join("goose")),
    };
    if let Some(home) = &env.home {
        out.push(rec(Scope::User, home.join(".agents/skills")));
    }
    if let Some(c) = config {
        out.push(rec(Scope::User, c.join("skills")));
    }
    if let Some(home) = &env.home {
        out.push(rec(Scope::User, home.join(".claude/skills")));
        out.push(rec(Scope::User, home.join(".config/agents/skills")));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(cwd: &str, vars: &[(&str, &str)]) -> Env {
        Env {
            cwd: PathBuf::from(cwd),
            home: Some(PathBuf::from("/h")),
            os: "linux",
            vars: vars
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    fn paths(roots: &[SearchRoot], agent: Agent) -> Vec<String> {
        roots
            .iter()
            .filter(|r| r.agent == Some(agent))
            .map(|r| r.path.display().to_string())
            .collect()
    }

    #[test]
    fn claude_outside_git_searches_managed_user_then_cwd() {
        let r = plan(&env("/nonexistent/w", &[]), &[]);
        assert_eq!(
            paths(&r, Agent::Claude),
            [
                "/etc/claude-code/.claude/skills",
                "/h/.claude/skills",
                "/h/.claude/skills/synced",
                "/nonexistent/w/.claude/skills"
            ]
        );
        let r = plan(
            &env(
                "/nonexistent/w",
                &[("CLAUDE_CODE_DISABLE_POLICY_SKILLS", "1")],
            ),
            &[],
        );
        assert_eq!(paths(&r, Agent::Claude)[0], "/h/.claude/skills");
    }

    #[test]
    fn opencode_outside_git_walks_to_filesystem_root_and_honors_flags() {
        let r = plan(&env("/nonexistent/w", &[("XDG_CONFIG_HOME", "/x")]), &[]);
        assert_eq!(
            paths(&r, Agent::Opencode),
            [
                "/h/.claude/skills",
                "/h/.agents/skills",
                "/nonexistent/w/.claude/skills",
                "/nonexistent/w/.agents/skills",
                "/nonexistent/.claude/skills",
                "/nonexistent/.agents/skills",
                "/.claude/skills",
                "/.agents/skills",
                "/x/opencode/skill",
                "/x/opencode/skills",
                "/nonexistent/w/.opencode/skill",
                "/nonexistent/w/.opencode/skills",
                "/nonexistent/.opencode/skill",
                "/nonexistent/.opencode/skills",
                "/.opencode/skill",
                "/.opencode/skills",
                "/h/.opencode/skill",
                "/h/.opencode/skills",
            ]
        );
        let r = plan(
            &env(
                "/nonexistent/w",
                &[
                    ("OPENCODE_DISABLE_CLAUDE_CODE_SKILLS", "yes"),
                    ("OPENCODE_DISABLE_PROJECT_CONFIG", "1"),
                    ("OPENCODE_CONFIG_DIR", "/c"),
                ],
            ),
            &[],
        );
        let p = paths(&r, Agent::Opencode);
        assert!(
            !p.iter()
                .any(|p| p.contains(".claude") || p.contains("/w/.opencode")),
            "{p:?}"
        );
        assert_eq!(p.last().unwrap(), "/c/skills");
        let r = plan(
            &env(
                "/nonexistent/w",
                &[("OPENCODE_DISABLE_EXTERNAL_SKILLS", "true")],
            ),
            &[],
        );
        assert!(
            !paths(&r, Agent::Opencode)
                .iter()
                .any(|p| p.contains(".agents"))
        );
    }

    #[test]
    fn goose_searches_cwd_only_and_its_config_dir() {
        let r = plan(
            &env("/nonexistent/w", &[("XDG_CONFIG_HOME", "relative")]),
            &[],
        );
        assert_eq!(
            paths(&r, Agent::Goose),
            [
                "/nonexistent/w/.agents/skills",
                "/nonexistent/w/.goose/skills",
                "/nonexistent/w/.claude/skills",
                "/h/.agents/skills",
                "/h/.config/goose/skills",
                "/h/.claude/skills",
                "/h/.config/agents/skills",
            ]
        );
        let r = plan(&env("/nonexistent/w", &[("GOOSE_PATH_ROOT", "/g")]), &[]);
        assert_eq!(paths(&r, Agent::Goose)[4], "/g/config/skills");
    }

    #[test]
    fn project_walks_stop_at_the_git_root() {
        let base = std::env::temp_dir().join(format!("the-locate-{}", std::process::id()));
        let sub = base.join("repo/a");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::create_dir_all(base.join("repo/.git")).unwrap();
        let e = Env {
            cwd: sub.clone(),
            home: None,
            os: "other",
            vars: HashMap::new(),
        };
        let r = plan(&e, &[]);
        assert_eq!(
            paths(&r, Agent::Claude),
            [
                sub.join(".claude/skills").display().to_string(),
                base.join("repo/.claude/skills").display().to_string()
            ]
        );
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn gemini_reads_cwd_then_home_with_no_parent_walk() {
        // Gemini CLI (ST-02): `.agents/skills` before `.gemini/skills` in the
        // cwd ONLY (no parent walk), then the same two under the home dir.
        let r = plan(&env("/nonexistent/w/deep", &[]), &[]);
        assert_eq!(
            paths(&r, Agent::Gemini),
            [
                "/nonexistent/w/deep/.agents/skills",
                "/nonexistent/w/deep/.gemini/skills",
                "/h/.agents/skills",
                "/h/.gemini/skills",
            ]
        );
        // GEMINI_CLI_HOME overrides the home dir for the user-scope roots.
        let r = plan(&env("/w", &[("GEMINI_CLI_HOME", "/gh")]), &[]);
        assert_eq!(
            paths(&r, Agent::Gemini),
            [
                "/w/.agents/skills",
                "/w/.gemini/skills",
                "/gh/.agents/skills",
                "/gh/.gemini/skills",
            ]
        );
    }

    #[test]
    fn opencode_project_config_disable_needs_truthy_not_yes() {
        // OPENCODE_DISABLE_PROJECT_CONFIG goes through `truthy` (true|1), unlike
        // the external-skills flags that use Effect's boolean (also yes|on|y).
        // So `yes` does NOT disable project config; `1` does. Guards the split.
        let yes = plan(
            &env("/w", &[("OPENCODE_DISABLE_PROJECT_CONFIG", "yes")]),
            &[],
        );
        assert!(
            paths(&yes, Agent::Opencode)
                .iter()
                .any(|p| p == "/w/.opencode/skill"),
            "`yes` must not disable project config (truthy accepts only true|1)"
        );
        let one = plan(&env("/w", &[("OPENCODE_DISABLE_PROJECT_CONFIG", "1")]), &[]);
        assert!(
            !paths(&one, Agent::Opencode)
                .iter()
                .any(|p| p.contains("/w/.opencode")),
            "`1` disables project config"
        );
    }

    #[test]
    fn claude_managed_path_is_os_specific() {
        // The macOS managed/policy path was untested (the env() helper is linux).
        let macos = Env {
            os: "macos",
            ..env("/nonexistent/w", &[])
        };
        assert_eq!(
            paths(&plan(&macos, &[]), Agent::Claude)[0],
            "/Library/Application Support/ClaudeCode/.claude/skills"
        );
        // An unknown OS has no managed/policy path; the first root is the user one.
        let other = Env {
            os: "other",
            ..env("/nonexistent/w", &[])
        };
        assert_eq!(
            paths(&plan(&other, &[]), Agent::Claude)[0],
            "/h/.claude/skills"
        );
    }

    #[test]
    fn git_root_recognizes_a_dotgit_file_not_only_a_dir() {
        // Worktrees and submodules use a `.git` FILE, not a directory; git_root
        // uses `.exists()`, so the parent walk must stop at it too.
        let base = std::env::temp_dir().join(format!("the-locate-gitfile-{}", std::process::id()));
        let sub = base.join("repo/a");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(base.join("repo/.git"), "gitdir: /elsewhere\n").unwrap();
        let e = Env {
            cwd: sub.clone(),
            home: None,
            os: "other",
            vars: HashMap::new(),
        };
        let r = plan(&e, &[]);
        assert_eq!(
            paths(&r, Agent::Claude),
            [
                sub.join(".claude/skills").display().to_string(),
                base.join("repo/.claude/skills").display().to_string()
            ]
        );
        std::fs::remove_dir_all(&base).unwrap();
    }
}
