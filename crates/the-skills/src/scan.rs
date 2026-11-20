//! Filesystem scanning for `SKILL.md`, plus symlink detection so a skill (or a whole skills
//! directory) shared between agents by a symlink is spotted.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::SKILL_FILE;
use super::locate::{Depth, Hidden, SearchRoot};

/// A symlink on the way to something: the link itself and the directory it resolves to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Link {
    pub link: PathBuf,
    pub target: PathBuf,
}

/// One `SKILL.md` found under a root.
#[derive(Debug, Clone)]
pub struct Hit {
    /// The SKILL.md path as reached from the root (symlinks not resolved).
    pub path: PathBuf,
    /// The first symlink between the root's parent and the skill directory, if any.
    pub link: Option<Link>,
}

/// The first symlink among `path` and its ancestors, checking at most `depth` components from the
/// end. Used for a root (its `skills` dir and the dot-dir holding it) and for a skill directory.
pub fn link_in(path: &Path, depth: usize) -> Option<Link> {
    let mut chain: Vec<&Path> = path.ancestors().take(depth).collect();
    chain.reverse(); // outermost first: report the link nearest the user's home/project.
    chain.into_iter().find_map(|p| {
        let is_link = fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink());
        is_link.then(|| Link {
            link: p.to_path_buf(),
            target: fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()),
        })
    })
}

/// All SKILL.md files under `root`, sorted by path. A missing root yields none; an unreadable one is
/// an error.
pub fn scan(root: &SearchRoot) -> io::Result<Vec<Hit>> {
    if !root.path.is_dir() {
        return Ok(Vec::new());
    }
    let root_link = link_in(&root.path, 2);
    let mut files = Vec::new();
    match root.depth {
        Depth::OneLevel => {
            for dir in sorted_entries(&root.path)? {
                let f = dir.join(SKILL_FILE);
                if dir.is_dir() && f.is_file() {
                    files.push(f);
                }
            }
        }
        Depth::Recursive => {
            let mut visited = HashSet::new();
            walk(&root.path, root.hidden, &mut visited, &mut files);
        }
    }
    Ok(files
        .into_iter()
        .map(|path| {
            let skill_dir = path.parent().unwrap_or(&root.path);
            let below = skill_dir
                .strip_prefix(&root.path)
                .map_or(0, |r| r.components().count());
            let link = root_link.clone().or_else(|| link_in(skill_dir, below));
            Hit { path, link }
        })
        .collect())
}

fn sorted_entries(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut v: Vec<PathBuf> = fs::read_dir(dir)?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .collect();
    v.sort();
    Ok(v)
}

/// Recursive scan following symlinks, guarded against cycles by canonical path.
fn walk(dir: &Path, hidden: Hidden, visited: &mut HashSet<PathBuf>, out: &mut Vec<PathBuf>) {
    let Ok(canonical) = fs::canonicalize(dir) else {
        return;
    };
    if !visited.insert(canonical) {
        return;
    }
    let Ok(entries) = sorted_entries(dir) else {
        return;
    };
    for p in entries {
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let skip = match hidden {
            Hidden::Include => false,
            Hidden::SkipVcs => matches!(name.as_str(), ".git" | ".hg" | ".svn"),
            Hidden::Exclude => name.starts_with('.'),
        };
        if skip {
            continue;
        }
        if p.is_dir() {
            walk(&p, hidden, visited, out);
        } else if name == SKILL_FILE && p.is_file() {
            out.push(p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::Scope;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("the-scan-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        fs::canonicalize(d).unwrap()
    }

    fn skill(dir: &Path) {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join(SKILL_FILE), "---\nname: x\n---\n").unwrap();
    }

    fn r(path: &Path, depth: Depth, hidden: Hidden) -> SearchRoot {
        SearchRoot {
            agent: None,
            scope: Scope::Dir,
            path: path.to_path_buf(),
            depth,
            hidden,
        }
    }

    fn rel(hits: &[Hit], base: &Path) -> Vec<String> {
        hits.iter()
            .map(|h| h.path.strip_prefix(base).unwrap().display().to_string())
            .collect()
    }

    #[test]
    fn depth_and_hidden_rules() {
        let b = tmp("depth");
        skill(&b.join("a"));
        skill(&b.join("group/b"));
        skill(&b.join(".hidden/c"));
        skill(&b.join(".git/d"));
        assert_eq!(
            rel(&scan(&r(&b, Depth::OneLevel, Hidden::Include)).unwrap(), &b),
            ["a/SKILL.md"]
        );
        assert_eq!(
            rel(
                &scan(&r(&b, Depth::Recursive, Hidden::Include)).unwrap(),
                &b
            ),
            [
                ".git/d/SKILL.md",
                ".hidden/c/SKILL.md",
                "a/SKILL.md",
                "group/b/SKILL.md"
            ]
        );
        assert_eq!(
            rel(
                &scan(&r(&b, Depth::Recursive, Hidden::SkipVcs)).unwrap(),
                &b
            ),
            [".hidden/c/SKILL.md", "a/SKILL.md", "group/b/SKILL.md"]
        );
        assert_eq!(
            rel(
                &scan(&r(&b, Depth::Recursive, Hidden::Exclude)).unwrap(),
                &b
            ),
            ["a/SKILL.md", "group/b/SKILL.md"]
        );
        assert!(
            scan(&r(&b.join("missing"), Depth::OneLevel, Hidden::Include))
                .unwrap()
                .is_empty()
        );
        fs::remove_dir_all(&b).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_skills_and_roots_are_spotted_and_cycles_terminate() {
        use std::os::unix::fs::symlink;
        let b = tmp("links");
        let shared = b.join("shared/pdf");
        skill(&shared);
        let claude = b.join("home/.claude/skills");
        fs::create_dir_all(&claude).unwrap();
        symlink(&shared, claude.join("pdf")).unwrap();
        skill(&claude.join("plain"));
        // A whole skills dir linked to another agent's: ~/.agents/skills -> ~/.claude/skills.
        fs::create_dir_all(b.join("home/.agents")).unwrap();
        symlink(&claude, b.join("home/.agents/skills")).unwrap();
        // A cycle must not hang a recursive scan.
        symlink(&claude, claude.join("plain/loop")).unwrap();

        let hits = scan(&r(&claude, Depth::Recursive, Hidden::Include)).unwrap();
        let pdf = hits
            .iter()
            .find(|h| h.path.ends_with("pdf/SKILL.md"))
            .unwrap();
        assert_eq!(
            pdf.link,
            Some(Link {
                link: claude.join("pdf"),
                target: shared.clone()
            })
        );
        let plain = hits
            .iter()
            .find(|h| h.path.ends_with("plain/SKILL.md"))
            .unwrap();
        assert_eq!(plain.link, None);

        let agents = b.join("home/.agents/skills");
        let hits = scan(&r(&agents, Depth::OneLevel, Hidden::Include)).unwrap();
        assert_eq!(hits.len(), 2);
        assert!(hits.iter().all(|h| h.link
            == Some(Link {
                link: agents.clone(),
                target: claude.clone()
            })));
        fs::remove_dir_all(&b).unwrap();
    }
}
