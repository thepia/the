//! Command-line surface (clap).

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use crate::output::Format;
use crate::skills::{Agent, Scope};

#[derive(Debug, Parser)]
#[command(name = "the", version, about = "Thepia command line for Linux and Nix")]
pub struct Cli {
    /// Output format for results (stdout) and diagnostics (stderr).
    #[arg(long, global = true, value_enum, default_value_t = Format::Human)]
    pub format: Format,

    /// Omit table header rows (human output).
    #[arg(long, global = true)]
    pub no_header: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Agent Skills as Claude Code, OpenCode and Goose find them from here.
    #[command(subcommand)]
    Skills(SkillsCmd),
}

#[derive(Debug, Subcommand)]
pub enum SkillsCmd {
    /// List skills, which agents load each one, and whether it is shared through a symlink.
    List(SearchArgs),
    /// Show one skill: metadata, each agent's verdict, where it is reached from, and its instructions.
    Show {
        /// The skill's name (frontmatter `name`, or its directory name).
        name: String,
        #[command(flatten)]
        search: SearchArgs,
    },
    /// List the directories each agent searches for skills, and where symlinked ones lead.
    Dirs(SearchArgs),
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
