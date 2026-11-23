//! The root command-line surface (clap).
//!
//! The root parser carries only the program metadata and the global flags ([`RootCli`]). Each command
//! group is its own crate implementing [`the_output::command::Target`]; the registry folds their
//! grammar onto [`root_command`], and `dispatch` reads the globals from the parsed matches by id
//! (`format`, `no-header`).

use clap::{CommandFactory, Parser};

use the_output::output::Format;

/// The root parser: program name/version/about plus the global flags. Command crates contribute the
/// subcommands onto [`root_command`]; this type exists so the globals are declared with clap's derive
/// (the drift test and `dispatch` both rely on their ids).
#[derive(Debug, Parser)]
#[command(
    name = "the",
    version,
    about = "Thepia command line for nurturing local AI agents"
)]
pub struct RootCli {
    /// Output format for results (stdout) and diagnostics (stderr).
    #[arg(id = "format", long, alias = "output", global = true, value_enum, default_value_t = Format::Human)]
    pub format: Format,

    /// Omit table header rows (human output).
    #[arg(id = "no-header", long, global = true)]
    pub no_header: bool,
}

/// Build the base clap command: the root globals, with a required subcommand so a bare `the` prints
/// help (matching the prior behavior). The registry folds each command crate's subcommand onto this.
pub fn root_command() -> clap::Command {
    RootCli::command()
        .subcommand_required(true)
        .arg_required_else_help(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use the_output::command::Target;

    #[test]
    fn root_globals_are_valid() {
        RootCli::command().debug_assert();
    }

    #[test]
    fn skills_grammar_folds_onto_the_root() {
        // The folded tree (root globals + a command crate's subcommand) must be a valid clap command:
        // this is the parser `dispatch` actually runs.
        the_skills::Skills.augment(root_command()).debug_assert();
    }

    #[test]
    fn agents_grammar_folds_onto_the_root() {
        the_agents::Agents.augment(root_command()).debug_assert();
    }

    #[test]
    fn both_command_crates_fold_together_without_collision() {
        // Every registered target's grammar folds onto one root (the real pipeline parser), so two
        // command crates must not collide on subcommand name or global-flag id.
        let root = the_agents::Agents.augment(the_skills::Skills.augment(root_command()));
        root.debug_assert();
    }
}
