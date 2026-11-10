//! `the`: the Thepia command line for Linux and Nix.
//!
//! Output contract (same as thepia/cli): stdout carries only the command's result; diagnostics,
//! warnings and notes go to stderr. `--format human|json` is the single switch for both.

mod cli;
mod diag;
mod output;
mod skills;
mod style;
mod table;

use std::process::ExitCode;

use clap::Parser;

fn main() -> ExitCode {
    let cli = cli::Cli::parse();
    let format = cli.format;
    let human = table::Human {
        term: style::Term::stdout(),
        header: !cli.no_header,
    };
    let result = match cli.command {
        cli::Command::Skills(cmd) => skills::cmd::run(cmd, format, human),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(d) => {
            d.emit(format);
            ExitCode::from(d.exit as u8)
        }
    }
}
