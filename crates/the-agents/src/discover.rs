//! Where each assistant keeps agent definitions, and reading them into [`AgentDef`]s.
//!
//! A definition is a single `*.md` file in an assistant's agents directory. Discovery folds over the
//! [assistant registry](crate::assistants): for each assistant it scans the project
//! (`<project_subdir>` from the cwd up to the home directory) and the user (`$HOME/<user_subdir>`,
//! unless `no_user`). A `--dir PATH` adds `PATH` itself (as Claude, the default for a bare directory of
//! agent files) plus `PATH/<project_subdir>` per assistant, so a project or a bare agents directory can
//! be scanned explicitly (and hermetically, with `--scope dir`).

use std::path::{Path, PathBuf};

use the_skills::locate::Env;

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

    // Each `--dir`: the directory itself (Claude rules, the default for a bare agents dir) plus each
    // assistant's project subdirectory under it.
    for d in dirs {
        roots.push(root(d.clone(), Scope::Dir, assistants::claude()));
        for assistant in assistants::ASSISTANTS {
            let sub = d.join(assistant.project_subdir);
            if &sub != d {
                roots.push(root(sub, Scope::Dir, assistant));
            }
        }
    }

    roots
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
