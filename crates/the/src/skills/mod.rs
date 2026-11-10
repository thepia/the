//! Agent Skills across coding agents.
//!
//! A skill is a directory holding a `SKILL.md`: YAML frontmatter (`name`, `description`, other keys)
//! and Markdown instructions, optionally with bundled files. `the skills` finds every skill that
//! Claude Code, OpenCode or Goose would find from the current directory, imitating each agent's own
//! search and load rules, and reports per skill which agents load it, shadow it, or skip it, and
//! whether it is shared between agents through a symlink.
//!
//! - [`locate`]: where each agent looks (search plan).
//! - [`scan`]: finding SKILL.md files under a root; symlink detection.
//! - [`parse`]: frontmatter, each agent's load rules, the advisory format lint.
//! - [`resolve`]: one catalog entry per distinct skill, with each agent's verdict.
//! - [`cmd`]: the `the skills ...` commands, rendering through [`crate::table`].

pub mod cmd;
pub mod locate;
pub mod parse;
pub mod resolve;
pub mod scan;

use std::fs;
use std::path::Path;

use clap::ValueEnum;
use serde::Serialize;

pub const SKILL_FILE: &str = "SKILL.md";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Agent {
    /// Claude Code.
    Claude,
    /// OpenCode.
    Opencode,
    /// Goose.
    Goose,
}

impl Agent {
    pub const ALL: [Agent; 3] = [Agent::Claude, Agent::Opencode, Agent::Goose];

    pub fn as_str(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Opencode => "opencode",
            Agent::Goose => "goose",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    /// The current directory and its parents.
    Project,
    /// The user's home and config directories.
    User,
    /// Machine-wide managed settings (Claude Code).
    System,
    /// Directories passed with `--dir`.
    Dir,
}

impl Scope {
    pub fn as_str(self) -> &'static str {
        match self {
            Scope::Project => "project",
            Scope::User => "user",
            Scope::System => "system",
            Scope::Dir => "dir",
        }
    }
}

/// Files bundled with a skill (everything but its SKILL.md), relative to the skill directory,
/// sorted. Hidden entries are skipped and symlinked directories are not followed.
pub fn bundled_files(dir: &Path) -> Vec<String> {
    fn walk(base: &Path, d: &Path, out: &mut Vec<String>) {
        let Ok(entries) = fs::read_dir(d) else { return };
        for e in entries.filter_map(Result::ok) {
            let p = e.path();
            if p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with('.'))
            {
                continue;
            }
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                walk(base, &p, out);
            } else if let Ok(rel) = p.strip_prefix(base)
                && rel != Path::new(SKILL_FILE)
            {
                out.push(rel.to_string_lossy().into_owned());
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}
