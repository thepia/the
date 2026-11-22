//! Orphan-file detection: a file in the skill directory that nothing in the skill references.
//!
//! A self-contained skill should not carry dead weight. This flags a regular file whose name is
//! mentioned by no other file in the skill, so a human can either wire up the forgotten reference or
//! delete the leftover. It is deliberately **conservative** (shipped Beta): a file is considered
//! referenced if its basename or skill-relative path appears *anywhere* in the text of any file in
//! the skill (a Markdown link, a prose mention, or a `bash scripts/x.sh` command line alike). A
//! coincidental substring match therefore suppresses the flag, so the check errs toward silence and
//! never raises a false alarm on a file that is actually used (INV-1 in spirit: never nag about a
//! working skill).

use std::fs;
use std::path::{Path, PathBuf};

use crate::parse::Problem;

/// Files conventionally present without being linked from SKILL.md, so never treated as orphans.
fn is_always_allowed(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "skill.md"
            | "readme.md"
            | "readme"
            | "readme.txt"
            | "license"
            | "license.md"
            | "license.txt"
            | "licence"
            | "licence.md"
            | "changelog.md"
            | "changelog"
            | "notice"
            | "notice.md"
    )
}

/// Flag every regular file under `skill_dir` referenced by nothing else in the skill.
pub fn check_orphans(skill_dir: &Path) -> Vec<Problem> {
    let mut files = Vec::new();
    collect_files(skill_dir, &mut files);

    // The reference corpus: the UTF-8 text of every readable file in the skill, concatenated. A
    // file's name appearing here (in SKILL.md, a doc, or a script) means something points at it.
    let corpus: String = files
        .iter()
        .filter_map(|p| fs::read_to_string(p).ok())
        .collect::<Vec<_>>()
        .join("\n");

    let mut out = Vec::new();
    for path in &files {
        let Ok(rel) = path.strip_prefix(skill_dir) else {
            continue;
        };
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if is_always_allowed(name) {
            continue;
        }
        if corpus.contains(name) || corpus.contains(rel_str.as_str()) {
            continue;
        }
        out.push(Problem::new(
            "orphan-unreferenced-file",
            format!("`{rel_str}` is in the skill directory but referenced by nothing"),
        ));
    }
    out.sort_by(|a, b| a.message.cmp(&b.message));
    out
}

/// Recursively collect regular files under `dir`, skipping hidden entries (dotfiles and dot-dirs
/// such as `.git`) and never following symlinks (those are the symlink checks' concern).
fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_symlink() {
            continue;
        }
        let path = entry.path();
        if ft.is_dir() {
            collect_files(&path, out);
        } else if ft.is_file() {
            out.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct TempSkill {
        dir: PathBuf,
    }

    impl TempSkill {
        fn new() -> Self {
            static N: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "the-orphans-ut-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::SeqCst)
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self {
                dir: fs::canonicalize(dir).unwrap(),
            }
        }
        fn write(&self, rel: &str, content: &str) {
            let p = self.dir.join(rel);
            if let Some(parent) = p.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(p, content).unwrap();
        }
    }
    impl Drop for TempSkill {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn flags_only_files_referenced_by_nothing() {
        let sk = TempSkill::new();
        sk.write(
            "SKILL.md",
            "---\nname: a\ndescription: d\n---\n\nSee [guide](references/guide.md).\nRun `bash scripts/run.sh`.\n",
        );
        sk.write("references/guide.md", "# Guide\n");
        sk.write("scripts/run.sh", "#!/bin/sh\necho hi\n"); // mentioned via a bash command line
        sk.write("scripts/helper.sh", "#!/bin/sh\n"); // referenced by nothing -> orphan
        sk.write("assets/unused.png", "binaryish\n"); // referenced by nothing -> orphan
        sk.write("README.md", "project readme\n"); // always allowed

        let orphans: Vec<String> = check_orphans(&sk.dir)
            .into_iter()
            .map(|p| p.message)
            .collect();
        assert_eq!(orphans.len(), 2, "{orphans:?}");
        assert!(orphans[0].contains("assets/unused.png"), "{orphans:?}");
        assert!(orphans[1].contains("scripts/helper.sh"), "{orphans:?}");
    }

    #[test]
    fn a_file_named_in_prose_only_is_not_orphaned() {
        let sk = TempSkill::new();
        sk.write(
            "SKILL.md",
            "---\nname: a\ndescription: d\n---\n\nThe template lives at template.json; copy it.\n",
        );
        sk.write("template.json", "{}\n");
        assert!(
            check_orphans(&sk.dir).is_empty(),
            "a plain-text mention of the filename counts as a reference"
        );
    }
}
