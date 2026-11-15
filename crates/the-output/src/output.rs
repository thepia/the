//! Result output: the global format switch and a stdout writer that tolerates a closed pipe.

use std::io::{self, Write};

use crate::diag::{Diagnostic, Exit};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "cli", derive(clap::ValueEnum))]
pub enum Format {
    /// Human-readable text.
    Human,
    /// JSON: one document on stdout; one JSON object per line for diagnostics on stderr.
    Json,
}

/// Write the command's result to stdout. A closed pipe (`the skills list | head -1`) is not an error.
pub fn print(text: &str) -> Result<(), Diagnostic> {
    let mut out = io::stdout().lock();
    match out.write_all(text.as_bytes()).and_then(|()| out.flush()) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        Err(e) => Err(
            Diagnostic::error("stdout-write", "could not write the result to stdout")
                .cause(e.to_string())
                .exit(Exit::Generic),
        ),
    }
}
