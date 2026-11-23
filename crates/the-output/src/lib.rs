//! Shared CLI output primitives for the Thepia command lines (`the`, `thepia`/`cli`).
//!
//! The single source of truth for the public-CLI output contract (SPEC-0001 OUT/ERR/FMT): stdout
//! carries only a command's result, diagnostics go to stderr, and `--format human|json` is the one
//! switch for both. Four modules:
//!
//! - [`diag`]: structured, rustc-shaped [`Diagnostic`](diag::Diagnostic)s (what / why / fix, a stable
//!   code, a semantic [`Exit`](diag::Exit)); human or JSON-lines on stderr.
//! - [`table`]: render-independent result data ([`Report`](table::Report) of
//!   [`Table`](table::Table)s, [`Record`](table::Record)s and text); aligned+colored for a terminal,
//!   plain when piped, fully typed in JSON.
//! - [`output`]: the [`Format`](output::Format) switch and a broken-pipe-tolerant stdout writer.
//! - [`style`]: terminal styling over `anstyle` with `NO_COLOR`/`CLICOLOR` gating and the shared
//!   glyph/colour vocabulary (cli-style-guide §8), so the user's terminal theme wins.
//!
//! Rendering only: this crate never owns a transport/wire contract. A daemon's transportable
//! diagnostic subset lives elsewhere and is rendered through [`diag`] at the CLI boundary.

pub mod diag;
pub mod output;
pub mod style;
pub mod table;

// The command pipeline contract (routable invocation, uniform `Target`/`Results` interface, a live
// `Registry`, and a stream-injectable `dispatch`). Needs clap, so it rides the `cli` feature. A
// command group is a crate implementing `command::Target`; the binary registers each one.
#[cfg(feature = "cli")]
pub mod command;
