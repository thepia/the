//! Fix advice (WORK-0012 Phase 1, FIX4): classify each detected skill issue by how safely it can be
//! repaired and recommend an approach. **Write-nothing** — this plans and explains fixes, it never
//! applies them (applying is Phase 2's `--fix`). The class tells a user or a coding agent which issues
//! are mechanical, which need a human decision, and which are advice-only.
//!
//! No advice ever fabricates content: a missing or over-long `description`, a security finding, a
//! broken link are all [`FixClass::Manual`] (the no-dummy-values norm — a value a tool cannot derive
//! losslessly is for a human to author). Only a deterministic, lossless edit is [`FixClass::Auto`].
//!
//! The classification follows the biome/ruff safe-vs-unsafe split and is kept **separate from
//! presentation** (the subcommand rule): [`plan_advice`] is pure and testable; `cmd.rs` renders it.

use std::path::PathBuf;

use crate::parse::{Problem, name_format_ok};
use crate::resolve::{Skill, Status};

/// How safely an issue can be repaired.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FixClass {
    /// Deterministic and lossless: safe to apply automatically (a future `--fix`).
    Auto,
    /// Identity-changing or needing a choice: applied only behind a future `--fix --unsafe`.
    Assisted,
    /// Advice only — no safe mechanical edit exists; a human (or agent) must decide and author it.
    Manual,
}

impl FixClass {
    pub fn as_str(self) -> &'static str {
        match self {
            FixClass::Auto => "auto",
            FixClass::Assisted => "assisted",
            FixClass::Manual => "manual",
        }
    }
}

/// One issue's repair advice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Advice {
    /// The skill the issue belongs to (its displayed name).
    pub skill: String,
    /// The skill's SKILL.md path.
    pub path: PathBuf,
    /// The diagnostic code the advice is for.
    pub code: &'static str,
    pub class: FixClass,
    /// The recommended approach.
    pub strategy: String,
}

impl Advice {
    /// Whether the fix is safe to apply without `--unsafe` (i.e. [`FixClass::Auto`]).
    pub fn safe(&self) -> bool {
        self.class == FixClass::Auto
    }
}

/// Plan repair advice for every issue in `skills`, one [`Advice`] per distinct (skill, code). Pure:
/// it reads the already-computed verdicts and lint problems and classifies them, touching nothing.
pub fn plan_advice(skills: &[&Skill]) -> Vec<Advice> {
    let mut out = Vec::new();
    for s in skills {
        let name = s.parsed.str_field("name").filter(|n| !n.is_empty());
        let push = |out: &mut Vec<Advice>,
                    seen: &mut Vec<&'static str>,
                    code: &'static str,
                    problem: Option<&Problem>| {
            if seen.contains(&code) {
                return;
            }
            seen.push(code);
            out.push(Advice {
                skill: s.name.clone(),
                path: s.path.clone(),
                code,
                class: classify(code, name),
                strategy: strategy(code, name, problem),
            });
        };
        let mut seen: Vec<&'static str> = Vec::new();
        // Agent-breaking / can't-tell verdicts carry the actionable problem.
        for v in &s.agents {
            if let Status::Skipped(p) | Status::Unknown(p) = &v.status {
                push(&mut out, &mut seen, p.code, Some(p));
            }
        }
        if s.agents
            .iter()
            .any(|v| matches!(v.status, Status::Conflict { .. }))
        {
            push(&mut out, &mut seen, "skill-conflict", None);
        }
        if !s.claude_alias_with.is_empty() {
            push(&mut out, &mut seen, "claude-alias-collision", None);
        }
        for p in &s.lint {
            push(&mut out, &mut seen, p.code, Some(p));
        }
    }
    out
}

/// The repair class for `code`, given the skill's frontmatter `name`. `name-format` is the only code
/// whose class is data-dependent: a purely-uppercase name lowercases losslessly (`Auto`), any other
/// malformation needs a human-chosen replacement (`Assisted`).
fn classify(code: &str, name: Option<&str>) -> FixClass {
    match code {
        "name-format" if lowercasing_fixes_name(name) => FixClass::Auto,
        "name-format" | "name-mismatch" | "name-missing" => FixClass::Assisted,
        _ => FixClass::Manual,
    }
}

/// True when the name's only format violation is uppercase ASCII letters, so lowercasing it yields a
/// valid name with no other change (the lossless `Auto` case).
fn lowercasing_fixes_name(name: Option<&str>) -> bool {
    let Some(name) = name else { return false };
    let lower = name.to_ascii_lowercase();
    lower != name && name_format_ok(&lower)
}

/// The recommended approach for `code`. Where a [`Problem`] exists its `fix` is the base guidance; the
/// `Auto` name-format case names the exact lowercased value, and the cross-skill codes
/// (`skill-conflict`, `claude-alias-collision`) carry their own advice since they have no single
/// `Problem`.
fn strategy(code: &str, name: Option<&str>, problem: Option<&Problem>) -> String {
    match code {
        "name-format" if lowercasing_fixes_name(name) => {
            let lower = name.unwrap_or_default().to_ascii_lowercase();
            format!("lowercase `name:` to `{lower}` (the only issue is capitalisation)")
        }
        "skill-conflict" => {
            "rename all but one of the duplicate-named skills to a unique `name:`, or remove the duplicates".to_owned()
        }
        "claude-alias-collision" => {
            "give each skill a unique `name:` (ideally its directory name), or remove `name:`".to_owned()
        }
        _ => problem
            .map(|p| p.fix.to_owned())
            .unwrap_or_else(|| "no automatic fix; review and correct manually".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uppercase_only_name_is_auto_lowercase_and_names_the_value() {
        // A name whose only fault is capitalisation lowercases losslessly: the one Auto case.
        assert_eq!(classify("name-format", Some("PDF")), FixClass::Auto);
        let s = strategy("name-format", Some("PDF"), None);
        assert!(s.contains("lowercase"), "{s}");
        assert!(s.contains("`pdf`"), "names the exact lowercased value: {s}");
    }

    #[test]
    fn structurally_bad_name_is_assisted_not_auto() {
        // Lowercasing cannot fix a leading hyphen, an internal space, or a double hyphen: a human
        // must choose a valid name, so these are Assisted (never Auto), and never Manual.
        for name in ["-bad", "a b", "a--b", "endshyphen-"] {
            assert_eq!(
                classify("name-format", Some(name)),
                FixClass::Assisted,
                "name `{name}` needs a human-chosen replacement"
            );
            assert!(
                !lowercasing_fixes_name(Some(name)),
                "`{name}` is not lowercase-fixable"
            );
        }
    }

    #[test]
    fn name_identity_codes_are_assisted() {
        assert_eq!(classify("name-mismatch", Some("x")), FixClass::Assisted);
        assert_eq!(classify("name-missing", None), FixClass::Assisted);
    }

    #[test]
    fn description_security_asset_and_shape_issues_are_manual_never_fabricated() {
        // Nothing here has a lossless mechanical fix: a description can't be invented or safely
        // truncated, a secret/broken link/over-broad Bash grant is a human decision.
        for code in [
            "description-missing",
            "description-too-long",
            "description-not-string",
            "unrestricted-bash",
            "exposed-secret",
            "trojan-source",
            "symlink-escape",
            "broken-link",
            "link-escapes-skill",
            "compatibility-invalid",
            "metadata-not-map",
            "allowed-tools-invalid",
            "unknown-frontmatter-key",
            "no-frontmatter",
            "frontmatter-invalid",
        ] {
            assert_eq!(
                classify(code, Some("x")),
                FixClass::Manual,
                "`{code}` must be advice-only"
            );
        }
    }

    #[test]
    fn strategy_uses_the_problems_own_fix_when_present() {
        let p = Problem::new("description-too-long", "too long");
        let s = strategy("description-too-long", Some("x"), Some(&p));
        assert_eq!(s, p.fix);
    }

    #[test]
    fn cross_skill_codes_have_their_own_advice_without_a_problem() {
        assert!(strategy("skill-conflict", None, None).contains("rename"));
        assert!(strategy("claude-alias-collision", None, None).contains("unique `name:`"));
    }

    #[test]
    fn safe_is_exactly_the_auto_class() {
        let auto = Advice {
            skill: "s".into(),
            path: PathBuf::new(),
            code: "name-format",
            class: FixClass::Auto,
            strategy: String::new(),
        };
        assert!(auto.safe());
        let manual = Advice {
            class: FixClass::Manual,
            ..auto.clone()
        };
        assert!(!manual.safe());
    }
}
