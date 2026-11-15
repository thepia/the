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

/// A prioritized, capped view over a batch of [`Diagnostic`]s for an end-of-command stderr summary:
/// the most urgent few (errors before warnings before notes), how many were elided, and the per-level
/// counts. Pure (no rendering) so the selection stays testable and separate from presentation. Build
/// it with [`summarize`] and render it with [`Summary::render`]. Shared by every command that prints a
/// table of results and then a short "most urgent issues + status line" summary (skills lint today;
/// doctor and the other ports next).
pub struct Summary<'a> {
    shown: Vec<&'a Diagnostic>,
    hidden: usize,
    errors: usize,
    warnings: usize,
    notes: usize,
}

fn level_rank(level: Level) -> u8 {
    match level {
        Level::Error => 0,
        Level::Warning => 1,
        Level::Note => 2,
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// Order `diags` by urgency (errors, then warnings, then notes), cap the shown set to `limit` (`None`
/// shows all), and count each level. The sort is stable, so same-level diagnostics keep input order.
pub fn summarize(diags: &[Diagnostic], limit: Option<usize>) -> Summary<'_> {
    let mut order: Vec<&Diagnostic> = diags.iter().collect();
    order.sort_by_key(|d| level_rank(d.level));
    let shown_len = limit.map_or(order.len(), |n| order.len().min(n));
    let count = |lvl: Level| diags.iter().filter(|d| d.level == lvl).count();
    Summary {
        hidden: order.len() - shown_len,
        errors: count(Level::Error),
        warnings: count(Level::Warning),
        notes: count(Level::Note),
        shown: order.into_iter().take(shown_len).collect(),
    }
}

impl Summary<'_> {
    /// How many diagnostics the cap elided.
    pub fn hidden(&self) -> usize {
        self.hidden
    }

    /// Whether any diagnostic is an error or warning (callers map this to a non-zero exit code).
    pub fn has_error_or_warning(&self) -> bool {
        self.errors > 0 || self.warnings > 0
    }

    /// Render for stderr: a leading blank line, the shown diagnostics (each with its context and fix),
    /// an optional "N more … run `<more_hint>`" note when some were hidden, and a colored `= status:`
    /// line counting issues by level. `more_hint` is the command that reveals the hidden ones (e.g.
    /// `"the skills lint --all"`); pass `None` to omit the hint even when some were elided.
    pub fn render(&self, term: Term, more_hint: Option<&str>) -> String {
        // One block per issue, separated by a blank line for breathing room; the status line is the
        // last block. Each block already ends in a newline, so joining with "\n" inserts the blanks.
        let mut blocks: Vec<String> = self
            .shown
            .iter()
            .map(|d| d.render(Format::Human, term))
            .collect();
        if self.hidden > 0
            && let Some(hint) = more_hint
        {
            blocks.push(
                Diagnostic::note(
                    "more-issues",
                    format!(
                        "{} more issue{} not shown; run `{hint}` to see them",
                        self.hidden,
                        plural(self.hidden)
                    ),
                )
                .render(Format::Human, term),
            );
        }
        let mut parts = Vec::new();
        if self.errors > 0 {
            parts.push(term.paint(
                style::FAIL.bold(),
                &format!("{} error{}", self.errors, plural(self.errors)),
            ));
        }
        if self.warnings > 0 {
            parts.push(term.paint(
                style::WARN.bold(),
                &format!("{} warning{}", self.warnings, plural(self.warnings)),
            ));
        }
        if self.notes > 0 {
            parts.push(term.paint(
                style::INFO,
                &format!("{} note{}", self.notes, plural(self.notes)),
            ));
        }
        blocks.push(format!(
            "{} {}\n",
            term.paint(style::DIM, "= status:"),
            parts.join(", ")
        ));
        // Leading blank line separates the summary from the table; blank lines separate the blocks.
        format!("\n{}", blocks.join("\n"))
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

    fn batch() -> Vec<Diagnostic> {
        // Deliberately out of urgency order to prove the sort.
        vec![
            Diagnostic::note("n1", "note one"),
            Diagnostic::error("e1", "error one"),
            Diagnostic::warning("w1", "warn one"),
            Diagnostic::warning("w2", "warn two"),
        ]
    }

    #[test]
    fn summarize_orders_by_urgency_caps_and_counts() {
        let diags = batch();
        let s = summarize(&diags, Some(2));
        // Errors first, then warnings, then notes; capped to 2.
        assert_eq!(
            s.shown.iter().map(|d| d.code).collect::<Vec<_>>(),
            ["e1", "w1"]
        );
        assert_eq!(s.hidden(), 2);
        assert_eq!((s.errors, s.warnings, s.notes), (1, 2, 1));
        assert!(s.has_error_or_warning());
        // No cap shows all and hides none.
        assert_eq!(summarize(&diags, None).hidden(), 0);
    }

    #[test]
    fn render_shows_more_hint_only_when_hidden_and_counts_in_status() {
        let diags = batch();
        let out = summarize(&diags, Some(2)).render(Term::PLAIN, Some("the skills lint --all"));
        assert!(
            out.starts_with('\n'),
            "leading blank line separates from the table"
        );
        assert!(out.contains("error[e1]: error one"));
        assert!(
            out.contains("2 more issues not shown; run `the skills lint --all` to see them"),
            "{out}"
        );
        assert!(
            out.contains("= status: 1 error, 2 warnings, 1 note"),
            "{out}"
        );

        // No cap: no more-issues note even if a hint is supplied; singular pluralization.
        let one = summarize(&diags[..1], None).render(Term::PLAIN, Some("x"));
        assert!(!one.contains("more issue"), "{one}");
        assert!(one.contains("= status: 1 note"), "{one}");

        // Hidden but no hint supplied: the note is omitted.
        let no_hint = summarize(&diags, Some(1)).render(Term::PLAIN, None);
        assert!(!no_hint.contains("more issue"), "{no_hint}");
    }
}
