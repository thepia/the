//! Command-line surface (clap).

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::output::Format;
use crate::skills::{Agent, Scope};

#[derive(Debug, Parser)]
#[command(name = "the", version, about = "Thepia command line for Linux and Nix")]
pub struct Cli {
    /// Output format for results (stdout) and diagnostics (stderr).
    #[arg(long, alias = "output", global = true, value_enum, default_value_t = Format::Human)]
    pub format: Format,

    /// Omit table header rows (human output).
    #[arg(long, global = true)]
    pub no_header: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Agent Skills as Claude Code, Gemini CLI, OpenCode and Goose find them from here.
    Skills(SkillsArgs),
}

#[derive(Debug, Args)]
pub struct SkillsArgs {
    #[command(flatten)]
    pub search: SearchArgs,

    /// Group prose into multiple paragraphs with a heading for each scope/level where skills live.
    #[arg(long)]
    pub by_scope: bool,

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
        #[command(flatten)]
        search: SearchArgs,
    },
    /// List the directories each agent searches for skills, and where symlinked ones lead.
    Dirs(SearchArgs),
    /// Lint skills for agent loadability, spec conformance, broken local links, and security hazards.
    Lint {
        /// Optional skill name (frontmatter `name`, or its directory name) to lint a single skill.
        name: Option<String>,
        #[command(flatten)]
        search: SearchArgs,
        /// Show every issue, not just the 5 most urgent.
        #[arg(long)]
        all: bool,
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
    #[command(flatten)]
    pub search: SearchArgs,

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

#[derive(Debug, Args)]
pub struct SearchArgs {
    /// Also search this skills directory (each child directory holding a SKILL.md is a skill).
    /// Repeatable.
    #[arg(long = "dir", value_name = "PATH")]
    pub dirs: Vec<PathBuf>,

    /// Only consider this scope.
    #[arg(long, value_enum)]
    pub scope: Option<Scope>,

    /// Only what this agent loads (for `dirs`: only where it searches).
    #[arg(long, value_enum)]
    pub agent: Option<Agent>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }
}
