//! The `agents` command's argument surface (clap). Owned by this crate so `the-agents` is a
//! self-contained command group: it carries its own grammar, parsing, execution ([`crate::cmd`]), and
//! rendering. The binary only registers [`crate::Agents`].

use std::path::PathBuf;

use clap::{Args, Subcommand};

use crate::Scope;

/// A leading PATH positional stands in for a subcommand (`the agents ..`) while flags are still parsed
/// in any position (unlike an external subcommand, which swallows every trailing token).
#[derive(Debug, Args)]
pub struct AgentsArgs {
    #[command(flatten)]
    pub search: SearchArgs,

    /// A relative or absolute PATH given in place of a subcommand (e.g. `the agents ..`): scan that
    /// directory/project like the bare `agents`, additively. A non-path token here is reported as an
    /// unrecognized subcommand.
    pub path: Option<PathBuf>,

    /// (beta) Run the experimental lint rules still staged behind `--beta` in the rule registry.
    #[arg(long, global = true)]
    pub beta: bool,

    #[command(subcommand)]
    pub command: Option<AgentsCmd>,
}

#[derive(Debug, Subcommand)]
pub enum AgentsCmd {
    /// List agent definitions, their runtime, model, tools, and description.
    List {
        /// Optional directory or project PATH to also list definitions from (equivalent to `--dir`).
        path: Option<PathBuf>,
    },
    /// Show one agent definition: its metadata, findings, and the instruction body.
    Show {
        /// The agent's name (frontmatter `name`, or its file name).
        name: String,
    },
    /// List the lint rules `the agents lint` can emit: code, severity, gate, and what each checks.
    Rules,
    /// Lint agent definitions for loadability, delegation quality, field validity, and security.
    Lint {
        /// An agent name to lint one definition, OR a relative or absolute PATH (one containing `/`,
        /// starting with `~`, or `.`/`..`) to also lint the definitions in that directory or project.
        target: Option<String>,
        /// Show every issue, not just the 5 most urgent.
        #[arg(long)]
        all: bool,
    },
}

/// The scan-scope flags shared by every `agents` subcommand. They are `global`, so they may be placed
/// before or after the subcommand (`the agents --dir x lint` and `the agents lint --dir x` both work)
/// and are defined once rather than duplicated per subcommand.
#[derive(Debug, Clone, Args)]
pub struct SearchArgs {
    /// Also search this directory or project root for agent definitions (the directory itself and its
    /// `.claude/agents` subdirectory). Repeatable.
    #[arg(long = "dir", value_name = "PATH", global = true)]
    pub dirs: Vec<PathBuf>,

    /// Only consider this scope (`project`, `user`, or `dir`).
    #[arg(long, value_enum, global = true)]
    pub scope: Option<Scope>,

    /// Exclude the user's own agent definitions (`~/.claude/agents`) from the scan.
    #[arg(long, global = true)]
    pub no_user: bool,
}
