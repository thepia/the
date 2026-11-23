//! Where each runtime keeps agent definitions, and reading them into [`AgentDef`]s.
//!
//! A definition is a single `*.md` file in an agents directory. Claude Code uses `.claude/agents`, in
//! the project (the current directory and its ancestors up to the home directory) and in the user's
//! home. A `--dir PATH` adds `PATH` itself and `PATH/.claude/agents` as extra roots, so a project or a
//! bare agents directory can be scanned explicitly (and hermetically, with `--scope dir`).

use std::path::{Path, PathBuf};

use the_skills::locate::Env;

use crate::{AgentDef, Runtime, Scope, lint};

/// One directory that may hold agent definitions.
#[derive(Debug, Clone)]
pub struct Root {
    pub path: PathBuf,
    pub scope: Scope,
    pub runtime: Runtime,
    /// The directory exists (only existing roots are scanned; a missing `--dir` is reported).
    pub exists: bool,
}

/// A path that could not be read while scanning (surfaced as a notice, never silently dropped).
#[derive(Debug, Clone)]
pub struct ReadError {
    pub path: PathBuf,
    pub error: String,
}

/// Resolve the directories to scan: project (`.claude/agents` from the cwd up to, but not including,
/// the home directory), user (`~/.claude/agents`, unless `no_user`), and one or two roots per `--dir`.
/// The home directory ends the project walk so its agents are the single User root, never double-counted.
pub fn roots(env: &Env, dirs: &[PathBuf], no_user: bool) -> Vec<Root> {
    let mut roots = Vec::new();

    // Project: the cwd and each ancestor, stopping at the home directory (exclusive).
    for dir in env.cwd.ancestors() {
        if env.home.as_deref() == Some(dir) {
            break;
        }
        let path = dir.join(Runtime::Claude.subdir());
        let exists = path.is_dir();
        roots.push(Root {
            path,
            scope: Scope::Project,
            runtime: Runtime::Claude,
            exists,
        });
    }

    // User.
    if !no_user && let Some(home) = &env.home {
        let path = home.join(Runtime::Claude.subdir());
        let exists = path.is_dir();
        roots.push(Root {
            path,
            scope: Scope::User,
            runtime: Runtime::Claude,
            exists,
        });
    }

    // Each `--dir`: the directory itself, and its `.claude/agents` subdirectory.
    for d in dirs {
        roots.push(Root {
            path: d.clone(),
            scope: Scope::Dir,
            runtime: Runtime::Claude,
            exists: d.is_dir(),
        });
        let sub = d.join(Runtime::Claude.subdir());
        if &sub != d {
            let exists = sub.is_dir();
            roots.push(Root {
                path: sub,
                scope: Scope::Dir,
                runtime: Runtime::Claude,
                exists,
            });
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
            .filter(|p| p.extension().is_some_and(|x| x == "md") && p.is_file())
            .collect();
        files.sort();
        for path in files {
            match std::fs::read_to_string(&path) {
                Ok(raw) => defs.push(build_def(path, &raw, root.scope, root.runtime)),
                Err(e) => errors.push(ReadError {
                    path,
                    error: e.to_string(),
                }),
            }
        }
    }
    (defs, errors)
}

fn build_def(path: PathBuf, raw: &str, scope: Scope, runtime: Runtime) -> AgentDef {
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
    let findings = lint::lint(&parsed, raw, &file_stem, parent);
    AgentDef {
        name,
        file_stem,
        path,
        scope,
        runtime,
        parsed,
        findings,
    }
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

        let roots = vec![Root {
            path: dir.clone(),
            scope: Scope::Dir,
            runtime: Runtime::Claude,
            exists: true,
        }];
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
        let sub = d.join(Runtime::Claude.subdir());
        vec![
            Root {
                path: d.clone(),
                scope: Scope::Dir,
                runtime: Runtime::Claude,
                exists: d.is_dir(),
            },
            Root {
                path: sub.clone(),
                scope: Scope::Dir,
                runtime: Runtime::Claude,
                exists: sub.is_dir(),
            },
        ]
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
        let roots = vec![Root {
            path: dir.clone(),
            scope: Scope::Dir,
            runtime: Runtime::Claude,
            exists: true,
        }];
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
