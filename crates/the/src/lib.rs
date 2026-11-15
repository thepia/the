//! `thepia` (`the`): the Thepia command line for nurturing local AI agents.
//!
//! Output contract (same as thepia/cli): stdout carries only the command's result; diagnostics,
//! warnings and notes go to stderr. `--format human|json` is the single switch for both.

mod cli;
mod skills;

// The CLI output primitives (diagnostics, tables, the format switch, styling) are the shared
// `the-output` crate (ADR-0001 / WORK-0019). Re-export them at the crate root so the command modules
// keep reaching them as `crate::{diag,table,output,style}`.
use the_output::{diag, output, style, table};

use std::process::ExitCode;

use clap::Parser;

pub fn run() -> ExitCode {
    ensure_process_name();
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
        Ok(code) => code,
        Err(d) => {
            d.emit(format);
            ExitCode::from(d.exit as u8)
        }
    }
}

/// Ensure the running process appears as `thepia` in OS process lists (`ps`, `top`, Activity Monitor)
/// even when invoked via the `the` or `th` command names.
fn ensure_process_name() {
    #[cfg(target_os = "linux")]
    let _ = std::fs::write("/proc/self/comm", "thepia\n");

    #[cfg(unix)]
    {
        let Some(arg0) = std::env::args_os().next() else {
            return;
        };
        if std::path::Path::new(&arg0)
            .file_name()
            .and_then(|s| s.to_str())
            == Some("thepia")
        {
            return;
        }
        let Ok(exe) = std::env::current_exe() else {
            return;
        };
        let sibling = exe
            .parent()
            .map(|p| p.join("thepia"))
            .filter(|p| p.is_file())
            .or_else(|| {
                exe.canonicalize()
                    .ok()?
                    .parent()
                    .map(|p| p.join("thepia"))
                    .filter(|p| p.is_file())
            });
        if let Some(target) = sibling {
            use std::os::unix::process::CommandExt;
            let _ = std::process::Command::new(&target)
                .args(std::env::args_os().skip(1))
                .exec();
        }
    }
}
