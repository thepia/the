//! The `the use` clap grammar. A single optional positional `SUBJECT` carries the subject with an
//! optional `+`/`-` prefix (e.g. `+language-server`); `allow_hyphen_values` lets the `-` form through
//! clap instead of being parsed as a flag. A bare `the use` lists the subjects.

use clap::Args;

#[derive(Args, Debug)]
pub struct UseArgs {
    /// A use subject, optionally prefixed `+` to enable or `-` to disable (e.g. `+language-server`).
    /// Omit to list the subjects and their state. A bare name (no prefix) enables.
    #[arg(value_name = "SUBJECT", allow_hyphen_values = true)]
    pub subject: Option<String>,
}

/// What the parsed `SUBJECT` token asks for.
#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    /// `the use` — list all subjects.
    List,
    /// `the use +X` / `the use X` — install and enable X.
    Enable(String),
    /// `the use -X` — disable X.
    Disable(String),
}

impl Action {
    /// Interpret the optional positional: `None` lists; a `+`/`-` prefix enables/disables; a bare
    /// name enables. An empty or sign-only token is a usage error (`Err` carries the bad token).
    pub fn parse(subject: Option<&str>) -> Result<Action, String> {
        let Some(raw) = subject else {
            return Ok(Action::List);
        };
        let raw = raw.trim();
        if let Some(rest) = raw.strip_prefix('+') {
            non_empty(rest).map(Action::Enable)
        } else if let Some(rest) = raw.strip_prefix('-') {
            non_empty(rest).map(Action::Disable)
        } else {
            non_empty(raw).map(Action::Enable)
        }
    }
}

fn non_empty(s: &str) -> Result<String, String> {
    if s.is_empty() {
        Err("<empty>".to_string())
    } else {
        Ok(s.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_action_grammar() {
        assert_eq!(Action::parse(None).unwrap(), Action::List);
        assert_eq!(
            Action::parse(Some("+language-server")).unwrap(),
            Action::Enable("language-server".to_string())
        );
        assert_eq!(
            Action::parse(Some("-language-server")).unwrap(),
            Action::Disable("language-server".to_string())
        );
        assert_eq!(
            Action::parse(Some("language-server")).unwrap(),
            Action::Enable("language-server".to_string())
        );
        assert!(Action::parse(Some("+")).is_err());
        assert!(Action::parse(Some("")).is_err());
    }
}
