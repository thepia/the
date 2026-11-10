//! Structured diagnostics, rendered rustc-style for humans or as JSON lines for machines.
//!
//! Every diagnostic says WHAT went wrong (`title`), WHY (`cause`), and HOW to fix it (`help`), under a
//! stable kebab-case `code`. Errors carry a semantic exit code so callers branch on the number, not
//! on the prose. Diagnostics always go to stderr.

use std::io::Write;

use serde_json::json;

use crate::output::Format;
use crate::style::{self, Term};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Error,
    Warning,
    Note,
}

impl Level {
    fn as_str(self) -> &'static str {
        match self {
            Level::Error => "error",
            Level::Warning => "warning",
            Level::Note => "note",
        }
    }
}

/// Semantic process exit codes (shared with thepia/cli): 0 ok, 1 generic, 2 usage, 3 not found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Exit {
    Generic = 1,
    Usage = 2,
    NotFound = 3,
}

#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub level: Level,
    pub code: &'static str,
    pub title: String,
    pub cause: Option<String>,
    pub help: Option<String>,
    /// The offending values: paths, names.
    pub context: Vec<String>,
    pub exit: Exit,
}

impl Diagnostic {
    fn new(level: Level, code: &'static str, title: impl Into<String>) -> Self {
        Self {
            level,
            code,
            title: title.into(),
            cause: None,
            help: None,
            context: Vec::new(),
            exit: Exit::Generic,
        }
    }

    pub fn error(code: &'static str, title: impl Into<String>) -> Self {
        Self::new(Level::Error, code, title)
    }

    pub fn warning(code: &'static str, title: impl Into<String>) -> Self {
        Self::new(Level::Warning, code, title)
    }

    pub fn note(code: &'static str, title: impl Into<String>) -> Self {
        Self::new(Level::Note, code, title)
    }

    pub fn cause(mut self, cause: impl Into<String>) -> Self {
        self.cause = Some(cause.into());
        self
    }

    pub fn help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    pub fn context(mut self, item: impl Into<String>) -> Self {
        self.context.push(item.into());
        self
    }

    pub fn exit(mut self, exit: Exit) -> Self {
        self.exit = exit;
        self
    }

    /// Human output is rustc-shaped; `term` decides color (JSON is never colored).
    pub fn render(&self, format: Format, term: Term) -> String {
        match format {
            Format::Human => {
                let tone = match self.level {
                    Level::Error => style::FAIL,
                    Level::Warning => style::WARN,
                    Level::Note => style::INFO,
                };
                let label = format!("{}[{}]", self.level.as_str(), self.code);
                let mut s = format!(
                    "{}{} {}\n",
                    term.paint(tone.bold(), &label),
                    term.paint(style::HEAD, ":"),
                    term.paint(style::HEAD, &self.title)
                );
                for c in &self.context {
                    s.push_str(&format!("  {} {c}\n", term.paint(style::DIM, "-->")));
                }
                if let Some(cause) = &self.cause {
                    s.push_str(&format!(
                        "  {} {cause}\n",
                        term.paint(style::DIM, "= cause:")
                    ));
                }
                if let Some(help) = &self.help {
                    s.push_str(&format!("  {} {help}\n", term.paint(style::DIM, "= help:")));
                }
                s
            }
            Format::Json => {
                let mut v = json!({
                    "level": self.level.as_str(),
                    "code": self.code,
                    "title": self.title,
                });
                if let Some(cause) = &self.cause {
                    v["cause"] = json!(cause);
                }
                if let Some(help) = &self.help {
                    v["help"] = json!(help);
                }
                if !self.context.is_empty() {
                    v["context"] = json!(self.context);
                }
                if self.level == Level::Error {
                    v["exit"] = json!(self.exit as u8);
                }
                format!("{v}\n")
            }
        }
    }

    /// Write to stderr. Failure to write a diagnostic has nowhere left to be reported, so it is ignored.
    pub fn emit(&self, format: Format) {
        let text = self.render(format, Term::stderr());
        let _ = std::io::stderr().lock().write_all(text.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Diagnostic {
        Diagnostic::error("skill-not-found", "no skill named `x`")
            .context("/home/u/.claude/skills")
            .cause("searched 1 directory")
            .help("run `the skills list`")
            .exit(Exit::NotFound)
    }

    const COLOR: Term = Term {
        color: true,
        tty: true,
    };

    #[test]
    fn human_render_is_rustc_shaped() {
        let plain = "error[skill-not-found]: no skill named `x`\n  --> /home/u/.claude/skills\n  = cause: searched 1 directory\n  = help: run `the skills list`\n";
        assert_eq!(sample().render(Format::Human, Term::PLAIN), plain);
        let colored = sample().render(Format::Human, COLOR);
        assert!(colored.starts_with(&style::paint(
            true,
            style::FAIL.bold(),
            "error[skill-not-found]"
        )));
        assert!(colored.contains(&style::paint(true, style::DIM, "= help:")));
    }

    #[test]
    fn json_render_is_one_line_uncolored_with_exit_for_errors() {
        let line = sample().render(Format::Json, COLOR);
        assert!(!line.contains('\u{1b}'));
        assert_eq!(line.matches('\n').count(), 1);
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["code"], "skill-not-found");
        assert_eq!(v["exit"], 3);
        let w: serde_json::Value =
            serde_json::from_str(&Diagnostic::warning("w", "t").render(Format::Json, Term::PLAIN))
                .unwrap();
        assert!(w.get("exit").is_none());
    }
}
