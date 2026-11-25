//! Where each assistant keeps agent definitions, and reading them into [`AgentDef`]s.
//!
//! A definition is a single `*.md` file in an assistant's agents directory. Discovery folds over the
//! [assistant registry](crate::assistants): for each assistant it scans the project
//! (`<project_subdir>` from the cwd up to the home directory) and the user (`$HOME/<user_subdir>`,
//! unless `no_user`). A `--dir PATH` is classified ([`classify`]) and scanned by what it is: a project
//! root (has `.claude` / an instruction file) scans its `<project_subdir>`s only, a bare agents directory
//! scans its `*.md`, and a workspace scans each project-root child. So a project root's loose docs
//! (README.md, GOVERNANCE.md) are never read as agent definitions.

use std::path::{Path, PathBuf};

use the_skills::locate::{DirRole, Env, classify_dir};

use crate::{AgentDef, Assistant, Scope, assistants, lint};

/// One directory that may hold agent definitions.
#[derive(Debug, Clone)]
pub struct Root {
    pub path: PathBuf,
    pub scope: Scope,
    pub assistant: &'static Assistant,
    /// The directory exists (only existing roots are scanned; a missing `--dir` is reported).
    pub exists: bool,
}

/// A path that could not be read while scanning (surfaced as a notice, never silently dropped).
#[derive(Debug, Clone)]
pub struct ReadError {
    pub path: PathBuf,
    pub error: String,
}

fn root(path: PathBuf, scope: Scope, assistant: &'static Assistant) -> Root {
    let exists = path.is_dir();
    Root {
        path,
        scope,
        assistant,
        exists,
    }
}

/// Resolve the directories to scan across every registered assistant: project (`<project_subdir>` from
/// the cwd up to, but not including, the home directory), user (`$HOME/<user_subdir>`, unless
/// `no_user`), and the `--dir` roots. The home directory ends the project walk so a home-level dir is
/// the single User root, never double-counted.
pub fn roots(env: &Env, dirs: &[PathBuf], no_user: bool) -> Vec<Root> {
    let mut roots = Vec::new();

    for assistant in assistants::ASSISTANTS {
        // Project: the cwd and each ancestor, stopping at the home directory (exclusive).
        for dir in env.cwd.ancestors() {
            if env.home.as_deref() == Some(dir) {
                break;
            }
            roots.push(root(
                dir.join(assistant.project_subdir),
                Scope::Project,
                assistant,
            ));
        }
        // User.
        if !no_user && let Some(home) = &env.home {
            roots.push(root(
                home.join(assistant.user_subdir),
                Scope::User,
                assistant,
            ));
        }
    }

    // Each `--dir`: classify what it points at and scan accordingly, so a project root's loose docs
    // (README.md, GOVERNANCE.md) are never read as agent definitions.
    for d in dirs {
        expand_dir_root(d, &mut roots);
    }

    roots
}

/// Expand one `--dir` target into the roots to scan, per its [`classify_dir`] role (the shared
/// directory classifier in `the-skills`, used by every dimension). A non-existent `--dir` is still added
/// as a (non-existent) root so a missing path is reported, matching the prior behavior. Agent-specific
/// part: a `Collection` is scanned as a bare agents directory (its `*.md` are agent files); a
/// `ProjectRoot` is scanned by its `<project_subdir>`s; a `Workspace` by its project-root children.
fn expand_dir_root(d: &Path, roots: &mut Vec<Root>) {
    if !d.is_dir() {
        roots.push(root(d.to_path_buf(), Scope::Dir, assistants::claude()));
        return;
    }
    match classify_dir(d) {
        DirRole::Collection => roots.push(root(d.to_path_buf(), Scope::Dir, assistants::claude())),
        DirRole::ProjectRoot => push_project_subdirs(d, roots),
        DirRole::Workspace => {
            let mut children: Vec<PathBuf> = std::fs::read_dir(d)
                .into_iter()
                .flatten()
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .collect();
            children.sort();
            // One level only: a project-root child is scanned as a project; a bare agents-dir child
            // (named `agents`) by its files. A workspace of workspaces is not recursed.
            for c in children {
                match classify_dir(&c) {
                    DirRole::ProjectRoot => push_project_subdirs(&c, roots),
                    DirRole::Collection if is_agents_dirname(&c) => {
                        roots.push(root(c, Scope::Dir, assistants::claude()));
                    }
                    _ => {}
                }
            }
        }
    }
}

/// Scan each assistant's `<project_subdir>` under a project root (e.g. `.claude/agents`), not its root.
fn push_project_subdirs(d: &Path, roots: &mut Vec<Root>) {
    for assistant in assistants::ASSISTANTS {
        roots.push(root(
            d.join(assistant.project_subdir),
            Scope::Dir,
            assistant,
        ));
    }
}

/// True when a directory's own name is the conventional agents-dir name (`agents`): used to decide which
/// workspace children to scan as bare agent collections.
fn is_agents_dirname(d: &Path) -> bool {
    d.file_name().and_then(|n| n.to_str()) == Some("agents")
}

/// Read every `*.md` agent definition under the existing `roots`, parsing and linting each. Files are
/// read in sorted order per root so output is deterministic. Each `*.md` in an agents directory is
/// treated as a definition, so a file with no frontmatter is linted (and flagged) rather than ignored.
pub fn discover(roots: &[Root]) -> (Vec<AgentDef>, Vec<ReadError>) {
    let mut defs = Vec::new();
    let mut errors = Vec::new();
    for root in roots.iter().filter(|r| r.exists) {
        let entries = match std::fs::read_dir(&root.path) {
            Ok(e) => e,
            Err(e) => {
                errors.push(ReadError {
                    path: root.path.clone(),
                    error: e.to_string(),
                });
                continue;
            }
        };
        let mut files: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| {
                p.extension().is_some_and(|x| x == "md")
                    && p.is_file()
                    // Instruction files (CLAUDE.md / AGENTS.md) are not agent definitions; the
                    // instruction pass of `the agents lint` handles them.
                    && !crate::instructions::is_instruction_file(p)
            })
            .collect();
        files.sort();
        for path in files {
            match std::fs::read_to_string(&path) {
                Ok(raw) => defs.push(build_def(path, &raw, root.scope, root.assistant)),
                Err(e) => errors.push(ReadError {
                    path,
                    error: e.to_string(),
                }),
            }
        }
    }
    (defs, errors)
}

fn build_def(path: PathBuf, raw: &str, scope: Scope, assistant: &'static Assistant) -> AgentDef {
    let parsed = the_skills::parse::parse(raw);
    let file_stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let name = parsed
        .str_field("name")
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map_or_else(|| file_stem.clone(), ToOwned::to_owned);
    let parent = path.parent().unwrap_or(Path::new("."));
    let is_plugin = in_plugin(&path);
    let findings = lint::lint(&parsed, raw, &file_stem, parent, assistant, is_plugin);
    AgentDef {
        name,
        file_stem,
        path,
        scope,
        assistant,
        parsed,
        findings,
        is_plugin,
    }
}

/// A definition is **plugin-packaged** when a `.claude-plugin/plugin.json` manifest sits at or above its
/// directory (ST-08). Plugin subagents do not support `hooks`/`mcpServers`/`permissionMode`, so the
/// field rules scope those to plugin agents only (agnix `CC-PL-014` detects plugins the same way).
fn in_plugin(path: &Path) -> bool {
    path.ancestors()
        .any(|dir| dir.join(".claude-plugin").join("plugin.json").is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, body: &str) {
        std::fs::write(dir.join(name), body).unwrap();
    }

    // No `name:` field, so the discovered name falls back to the file stem (reviewer / child).
    const AGENT: &str = "---\ndescription: Review changed files for correctness and surface risky diffs\n---\n# b\n";

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("the-agents-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn assistant_config_dirs_cover_every_registered_assistant() {
        // The shared project-root detector (the_skills::locate::ASSISTANT_CONFIG_DIRS) must list every
        // registered assistant's config dir, or a project root for that assistant would be misclassified.
        for a in assistants::ASSISTANTS {
            let first = Path::new(a.project_subdir)
                .components()
                .next()
                .unwrap()
                .as_os_str()
                .to_str()
                .unwrap();
            assert!(
                the_skills::locate::ASSISTANT_CONFIG_DIRS.contains(&first),
                "assistant `{}` config dir `{first}` missing from ASSISTANT_CONFIG_DIRS",
                a.id
            );
        }
    }

    #[test]
    fn a_project_root_scans_claude_agents_not_loose_docs() {
        // A `--dir` pointing at a project root (has `.claude`) scans `.claude/agents`, and its loose
        // root docs (README.md, GOVERNANCE.md) are NOT read as agent definitions.
        let dir = tmp("projroot");
        std::fs::create_dir_all(dir.join(".claude/agents")).unwrap();
        write(&dir.join(".claude/agents"), "reviewer.md", AGENT);
        write(&dir, "README.md", "# readme, not an agent\n");
        write(&dir, "GOVERNANCE.md", "# governance doc\n");

        let mut roots = Vec::new();
        expand_dir_root(&dir, &mut roots);
        let (defs, _) = discover(&roots);
        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        assert!(
            names.contains(&"reviewer"),
            "the real agent is found: {names:?}"
        );
        assert!(
            !names.iter().any(|n| *n == "README" || *n == "GOVERNANCE"),
            "loose project docs are not agent definitions: {names:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_bare_agents_dir_still_scans_its_md_files() {
        // A plain folder of agent files (no assistant config) is still scanned directly.
        let dir = tmp("baredir");
        write(&dir, "reviewer.md", AGENT);
        let mut roots = Vec::new();
        expand_dir_root(&dir, &mut roots);
        let (defs, _) = discover(&roots);
        assert!(defs.iter().any(|d| d.name == "reviewer"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_workspace_scans_project_children_not_its_own_docs() {
        // A parent of project directories (itself not a project root) scans each child project's agents.
        let ws = tmp("workspace");
        std::fs::create_dir_all(ws.join("proj/.claude/agents")).unwrap();
        write(&ws.join("proj/.claude/agents"), "child.md", AGENT);
        write(&ws, "README.md", "# workspace readme\n");

        let mut roots = Vec::new();
        expand_dir_root(&ws, &mut roots);
        let (defs, _) = discover(&roots);
        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        assert!(
            names.contains(&"child"),
            "the child project's agent is found: {names:?}"
        );
        assert!(
            !names.contains(&"README"),
            "the workspace's own loose doc is not an agent"
        );
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn a_dir_root_discovers_and_lints_its_md_files() {
        let dir = std::env::temp_dir().join(format!("the-agents-discover-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        write(
            &dir,
            "reviewer.md",
            "---\nname: reviewer\ndescription: Review changed files for correctness and surface risky diffs\n---\n# body\n",
        );
        write(
            &dir,
            "terse.md",
            "---\nname: terse\ndescription: PDF tools\n---\n# body\n",
        );
        // A non-.md file is ignored.
        write(&dir, "notes.txt", "ignore me\n");

        let roots = vec![root(dir.clone(), Scope::Dir, assistants::claude())];
        let (defs, errors) = discover(&roots);
        assert!(errors.is_empty());
        assert_eq!(defs.len(), 2, "two .md definitions discovered");
        // Sorted read order: reviewer before terse.
        assert_eq!(defs[0].name, "reviewer");
        assert!(defs[0].findings.is_empty(), "{:?}", defs[0].findings);
        assert_eq!(defs[1].name, "terse");
        assert!(
            defs[1]
                .findings
                .iter()
                .any(|f| f.code == "agent-description-terse"),
            "the terse definition is flagged"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_dir_is_a_non_existent_root_not_an_error() {
        let roots = roots_for_dir("/no/such/dir/anywhere");
        assert!(roots.iter().any(|r| r.scope == Scope::Dir && !r.exists));
        let (defs, errors) = discover(&roots);
        assert!(defs.is_empty());
        assert!(
            errors.is_empty(),
            "a non-existent dir is simply not scanned"
        );
    }

    fn roots_for_dir(p: &str) -> Vec<Root> {
        // Build only the Dir roots for `p` (bypass env-dependent project/user roots).
        let d = PathBuf::from(p);
        let sub = d.join(assistants::claude().project_subdir);
        vec![
            root(d.clone(), Scope::Dir, assistants::claude()),
            root(sub, Scope::Dir, assistants::claude()),
        ]
    }

    #[test]
    fn non_claude_definitions_get_universal_checks_only() {
        // An OpenCode-style definition (no `name` field; `mode: subagent`, an unknown key for Claude,
        // and a terse description) must NOT trip the Claude-specific name/description/key rules, because
        // its schema is different. It is still discovered, tagged with its assistant, and security-smoked.
        let dir = std::env::temp_dir().join(format!("the-agents-opencode-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        write(
            &dir,
            "review.md",
            "---\ndescription: Reviews code\nmode: subagent\n---\n# body\n",
        );
        let roots = vec![root(
            dir.clone(),
            Scope::Dir,
            assistants::by_id("opencode").unwrap(),
        )];
        let (defs, _) = discover(&roots);
        assert_eq!(defs[0].assistant.id, "opencode");
        assert_eq!(defs[0].name, "review", "name falls back to the file stem");
        assert!(
            defs[0].findings.is_empty(),
            "no Claude-specific findings on a non-Claude definition: {:?}",
            defs[0].findings
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn name_falls_back_to_the_file_stem() {
        let dir = std::env::temp_dir().join(format!("the-agents-stem-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // No `name:` in the frontmatter: the stem is used as the name, and name-missing is flagged.
        write(
            &dir,
            "fallback.md",
            "---\ndescription: a real sentence about what it does and when\n---\n# body\n",
        );
        let roots = vec![root(dir.clone(), Scope::Dir, assistants::claude())];
        let (defs, _) = discover(&roots);
        assert_eq!(defs[0].name, "fallback");
        assert!(
            defs[0]
                .findings
                .iter()
                .any(|f| f.code == "agent-name-missing")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
