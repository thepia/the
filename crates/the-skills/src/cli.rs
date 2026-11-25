//! The `skills` command's argument surface (clap). Owned by this crate so `the-skills` is a
//! self-contained command group: it carries its own grammar, parsing, execution ([`crate::cmd`]),
//! and rendering. The binary only registers [`crate::Skills`].

use std::path::PathBuf;

use clap::{Args, Subcommand, ValueEnum};

use crate::{Agent, Scope};

// A leading PATH positional stands in for a subcommand (`the skills ..`) while flags are still parsed
// in any position (unlike an external subcommand, which swallows every trailing token).
#[derive(Debug, Args)]
pub struct SkillsArgs {
    #[command(flatten)]
    pub search: SearchArgs,

    /// A relative or absolute PATH given in place of a subcommand (e.g. `the skills ..`): scan that
    /// directory/project like the bare `skills`, additively. A non-path token here is reported as an
    /// unrecognized subcommand.
    pub path: Option<PathBuf>,

    /// Group prose into multiple paragraphs with a heading for each scope/level where skills live.
    #[arg(long)]
    pub by_scope: bool,

    /// (beta) Expand beta-gated detail across the skills subcommands: run the experimental lint rules
    /// still staged behind `--beta` in the rule registry. For corpus-quality evaluation.
    #[arg(long, global = true)]
    pub beta: bool,

    #[command(subcommand)]
    pub command: Option<SkillsCmd>,
}

#[derive(Debug, Subcommand)]
pub enum SkillsCmd {
    /// List skills, which agents load each one, and whether it is shared through a symlink.
    List(ListArgs),
    /// Show one skill: metadata, each agent's verdict, where it is reached from, and its instructions.
    Show {
        /// The skill's name (frontmatter `name`, or its directory name).
        name: String,
    },
    /// List the directories each agent searches for skills, and where symlinked ones lead.
    Dirs {
        /// Optional directory or project PATH to also scan (equivalent to `--dir <PATH>`; the user's
        /// own skills are still included unless `--no-user`).
        path: Option<PathBuf>,
    },
    /// List the lint rules `the skills lint` can emit: code, severity, gate, and what each checks.
    Rules,
    /// Lint skills for agent loadability, spec conformance, broken local links, and security hazards.
    Lint {
        /// A skill name (frontmatter `name`, or its directory name) to lint one skill, OR a relative or
        /// absolute PATH (one containing `/`, starting with `~`, or `.`/`..`) to also lint the skills in
        /// that directory or project (equivalent to `--dir <PATH>`; the user's own skills are still
        /// included unless `--no-user`, and `--scope dir` restricts to just the given path).
        target: Option<String>,
        /// Show every issue, not just the 5 most urgent.
        #[arg(long)]
        all: bool,
        /// Recommend a fix approach per issue, classed auto / assisted / manual (writes nothing).
        #[arg(long)]
        advice: bool,
    },
}

/// Human presentation form for `the skills list`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum)]
pub enum ListForm {
    /// Aligned columns (`NAME`, `SCOPE`, `AGENTS`, `LINKED`, `DESCRIPTION`).
    #[default]
    Tabular,
    /// A single prose paragraph with all skill names.
    Prose,
    /// Multiple prose paragraphs with a heading for where/level each group of skills lives.
    ByScope,
}

#[derive(Debug, Args)]
pub struct ListArgs {
    /// Optional directory or project PATH to also list skills from (equivalent to `--dir <PATH>`; the
    /// user's own skills are still included unless `--no-user`).
    pub path: Option<PathBuf>,

    /// Human presentation form (`tabular`, `prose`, or `by-scope`).
    #[arg(long, value_enum, default_value_t = ListForm::Tabular)]
    pub form: ListForm,

    /// Render as a prose paragraph (shorthand for `--form prose`).
    #[arg(long)]
    pub prose: bool,

    /// Render prose grouped by scope/level with a heading per level (shorthand for `--form by-scope`).
    #[arg(long)]
    pub by_scope: bool,
}

impl ListArgs {
    pub fn presentation(&self) -> ListForm {
        if self.by_scope {
            ListForm::ByScope
        } else if self.prose {
            ListForm::Prose
        } else {
            self.form
        }
    }
}

/// The scan-scope flags shared by every `skills` subcommand. They are `global`, so they may be placed
/// before or after the subcommand (`the skills --dir x lint` and `the skills lint --dir x` both work)
/// and are defined once rather than duplicated per subcommand.
#[derive(Debug, Clone, Args)]
pub struct SearchArgs {
    /// Also search this skills directory or project root (each child holding a SKILL.md is a skill, and
    /// a project's `.claude/skills`, `.agents/skills`, ... are discovered under it). Repeatable.
    #[arg(long = "dir", value_name = "PATH", global = true)]
    pub dirs: Vec<PathBuf>,

    /// Only consider this scope.
    #[arg(long, value_enum, global = true)]
    pub scope: Option<Scope>,

    /// Exclude the user's own skills (`~/.claude/skills` and the other per-agent user roots) from the scan.
    #[arg(long, global = true)]
    pub no_user: bool,

    /// Only what this agent loads (for `dirs`: only where it searches).
    #[arg(long, value_enum, global = true)]
    pub agent: Option<Agent>,
}
