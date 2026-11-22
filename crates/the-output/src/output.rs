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
    write_result(&mut io::stdout().lock(), text)
}

/// Write `text` to `out`, mapping a broken pipe to `Ok` and any other failure to a diagnostic. Split
/// from [`print`] so the error-mapping is testable against a fake writer.
fn write_result(out: &mut impl Write, text: &str) -> Result<(), Diagnostic> {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A writer whose `write_all` fails with a chosen error kind.
    struct FailingWriter(io::ErrorKind);
    impl Write for FailingWriter {
        fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
            Err(io::Error::new(self.0, "boom"))
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn writes_to_a_good_sink() {
        let mut buf = Vec::new();
        assert!(write_result(&mut buf, "hello").is_ok());
        assert_eq!(buf, b"hello");
    }

    #[test]
    fn broken_pipe_is_not_an_error() {
        let mut w = FailingWriter(io::ErrorKind::BrokenPipe);
        assert!(write_result(&mut w, "x").is_ok());
    }

    #[test]
    fn other_write_failure_becomes_a_diagnostic() {
        let mut w = FailingWriter(io::ErrorKind::PermissionDenied);
        let d = write_result(&mut w, "x").expect_err("should surface a diagnostic");
        assert_eq!(d.code, "stdout-write");
        assert_eq!(d.exit, Exit::Generic);
    }
}
