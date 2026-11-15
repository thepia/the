//! Terminal styling, ported from `thepia/cli` (`crates/thepia/src/style.rs`, `docs/cli-style-guide.md`
//! §8). A thin layer over `anstyle` (already in the tree via clap) plus a `use_color` gate honoring the
//! ecosystem conventions. Color only reinforces glyphs/words that already carry the meaning, so every
//! helper degrades to plain text when color is off. JSON is never colored.

#![allow(dead_code)]

use std::io::IsTerminal;

use anstyle::{AnsiColor, Color, RgbColor, Style};

/// What a renderer may do on one output stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Term {
    /// Emit ANSI color.
    pub color: bool,
    /// The stream is a terminal: decorate for a reader (table rule, truncation). When piped or
    /// redirected, output stays aligned but undecorated and untruncated.
    pub tty: bool,
}

impl Term {
    /// No color, no decoration: what a pipe gets.
    #[cfg(test)]
    pub const PLAIN: Term = Term {
        color: false,
        tty: false,
    };

    pub fn stdout() -> Self {
        Self::for_stream(std::io::stdout().is_terminal())
    }

    pub fn stderr() -> Self {
        Self::for_stream(std::io::stderr().is_terminal())
    }

    fn for_stream(tty: bool) -> Self {
        Term {
            color: use_color(tty),
            tty,
        }
    }

    /// Wrap `s` in `style` iff color is on.
    pub fn paint(self, style: Style, s: &str) -> String {
        paint(self.color, style, s)
    }
}

/// Convenience: should stdout be colored right now?
pub fn stdout_color() -> bool {
    use_color(std::io::stdout().is_terminal())
}

/// Convenience: should stderr be colored right now?
pub fn stderr_color() -> bool {
    use_color(std::io::stderr().is_terminal())
}

/// Whether to emit color on a stream, given whether it is a terminal. Precedence (clig.dev,
/// no-color.org, the CLICOLOR lineage, same as thepia/cli): `CLICOLOR_FORCE`/`FORCE_COLOR` force on;
/// else `NO_COLOR` (non-empty) forces off; else `CLICOLOR=0` forces off; else on iff a terminal.
pub fn use_color(stream_is_tty: bool) -> bool {
    use_color_from(
        stream_is_tty,
        env_truthy("CLICOLOR_FORCE") || env_truthy("FORCE_COLOR"),
        std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty()),
        std::env::var_os("CLICOLOR").is_some_and(|v| v == "0"),
    )
}

/// Set to a non-empty value other than "0".
fn env_truthy(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|v| !v.is_empty() && v != "0")
}

fn use_color_from(is_tty: bool, force: bool, no_color: bool, clicolor_off: bool) -> bool {
    if force {
        return true;
    }
    if no_color || clicolor_off {
        return false;
    }
    is_tty
}

const fn ansi(c: AnsiColor) -> Option<Color> {
    Some(Color::Ansi(c))
}

// Semantic palette (cli-style-guide §8.2/§8.3): the 16 ANSI colors, so the user's terminal theme wins.
// Yellow and red are reserved for warnings and errors.
/// Success (green).
pub const OK: Style = Style::new().fg_color(ansi(AnsiColor::Green));
/// Warning, actionable gap (yellow).
pub const WARN: Style = Style::new().fg_color(ansi(AnsiColor::Yellow));
/// Error, failure (red).
pub const FAIL: Style = Style::new().fg_color(ansi(AnsiColor::Red));
/// Benign information (blue).
pub const INFO: Style = Style::new().fg_color(ansi(AnsiColor::Blue));
/// Secondary text: labels, punctuation, placeholders.
pub const DIM: Style = Style::new().dimmed();
/// Primary text: headers, titles.
pub const HEAD: Style = Style::new().bold();

/// Status glyph vocabulary (cli-style-guide §8.5): `[✓]` pass, `[i]` info, `[!]` warn, `[✗]` fail.
pub const GLYPH_PASS: &str = "[✓]";
pub const GLYPH_INFO: &str = "[i]";
pub const GLYPH_WARN: &str = "[!]";
pub const GLYPH_FAIL: &str = "[✗]";
/// Solid triangle indicator (`▲`) for inline issue highlighting and issue summary counts.
pub const GLYPH_ISSUE: &str = "▲";

/// Does the terminal advertise 24-bit truecolor? Only then do we use the exact brand lavender;
/// otherwise the brand accent degrades to ANSI magenta (the nearest base), per the style guide.
pub fn truecolor() -> bool {
    std::env::var("COLORTERM")
        .map(|v| v == "truecolor" || v == "24bit")
        .unwrap_or(false)
}

/// The Thepia brand accent: lavender `#988ACA` (bold) where the terminal supports truecolor, else
/// bold magenta (`cli-style-guide.md` §8.4).
pub fn brand() -> Style {
    let fg = if truecolor() {
        Color::Rgb(RgbColor(0x98, 0x8A, 0xCA))
    } else {
        Color::Ansi(AnsiColor::Magenta)
    };
    Style::new().bold().fg_color(Some(fg))
}

/// Wrap `s` in the style's enable/reset sequences iff `on`.
pub fn paint(on: bool, style: Style, s: &str) -> String {
    if on && !s.is_empty() {
        format!("{style}{s}{style:#}")
    } else {
        s.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn use_color_precedence() {
        assert!(
            use_color_from(false, true, true, true),
            "force beats everything"
        );
        assert!(
            !use_color_from(true, false, true, false),
            "NO_COLOR turns a tty off"
        );
        assert!(
            !use_color_from(true, false, false, true),
            "CLICOLOR=0 turns a tty off"
        );
        assert!(use_color_from(true, false, false, false));
        assert!(!use_color_from(false, false, false, false));
    }

    #[test]
    fn paint_is_plain_when_off_and_wrapped_when_on() {
        assert_eq!(paint(false, FAIL, "boom"), "boom");
        let on = paint(true, OK, "ok");
        assert!(on.starts_with('\u{1b}') && on.contains("ok") && on.ends_with("\u{1b}[0m"));
        assert_eq!(paint(true, OK, ""), "", "nothing to color, no escapes");
    }
}
