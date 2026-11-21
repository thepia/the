//! The single lint-rule registry: one row per rule, carrying every attribute (label, fix, kind,
//! severity, stability). `Problem::{new, kind, is_error}` and the CLI's issue label all read this one
//! table, instead of the same code being smeared across four separate `match` statements that drift.
//! A feature gate is a column (`stability`), not a parallel list: graduating a `Beta` rule to on-by-
//! default is flipping one field. (The metadata-table-drives-functions principle.)

use crate::parse::ProblemKind;

/// Diagnostic level a rule renders at. `is_error()` is `severity == Error`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warn,
    Note,
}

/// Whether a rule is on by default or still staged behind `--beta`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stability {
    /// On by default.
    Default,
    /// Staged behind `--beta`; graduate it by flipping this to `Default`.
    Beta,
}

/// One lint rule: its stable `code` and everything keyed off that code.
#[derive(Debug, Clone, Copy)]
pub struct Rule {
    pub code: &'static str,
    /// Human one-line label (the CLI's issue label).
    pub label: &'static str,
    /// The concrete fix advice.
    pub fix: &'static str,
    pub kind: ProblemKind,
    pub severity: Severity,
    pub stability: Stability,
}

use ProblemKind::{Asset, Breaking, Security, Spec};
use Severity::{Error, Note, Warn};
use Stability::{Beta, Default};

/// The registry. One row per rule; adding or changing a rule is one row, and every consumer reads it.
pub const RULES: &[Rule] = &[
    // Frontmatter parse / load (Breaking: an agent cannot load the skill).
    Rule {
        code: "no-frontmatter",
        label: "missing YAML frontmatter",
        fix: "start SKILL.md with a `---` block holding `name:` and `description:`, closed by `---`",
        kind: Breaking,
        severity: Warn,
        stability: Default,
    },
    Rule {
        code: "frontmatter-unterminated",
        label: "unterminated YAML frontmatter",
        fix: "close the frontmatter with a `---` line",
        kind: Breaking,
        severity: Warn,
        stability: Default,
    },
    Rule {
        code: "frontmatter-invalid",
        label: "invalid YAML frontmatter",
        fix: "fix the YAML; a plain value containing `: ` must be quoted (`description: \"...\"`) or use a `>-` block",
        kind: Breaking,
        severity: Warn,
        stability: Default,
    },
    Rule {
        code: "name-missing",
        label: "missing skill name",
        fix: "set `name:` to lowercase letters, digits and single hyphens",
        kind: Breaking,
        severity: Warn,
        stability: Default,
    },
    Rule {
        code: "name-has-slash",
        label: "skill name contains '/'",
        fix: "remove the `/` from `name:`",
        kind: Breaking,
        severity: Warn,
        stability: Default,
    },
    Rule {
        code: "description-not-string",
        label: "non-string description",
        fix: "make `description:` a plain string",
        kind: Breaking,
        severity: Warn,
        stability: Default,
    },
    // Spec conformance (agentskills.io deviations; rendered as notes).
    Rule {
        code: "name-format",
        label: "invalid skill name format",
        fix: "set `name:` to lowercase letters, digits and single hyphens",
        kind: Spec,
        severity: Note,
        stability: Default,
    },
    Rule {
        code: "name-mismatch",
        label: "skill name differs from directory",
        fix: "make `name:` equal the directory name, or rename the directory",
        kind: Spec,
        severity: Note,
        stability: Default,
    },
    Rule {
        code: "description-missing",
        label: "missing description",
        fix: "add a `description:` saying what the skill does and when to use it",
        kind: Spec,
        severity: Note,
        stability: Default,
    },
    Rule {
        code: "description-too-long",
        label: "description exceeds 1024 characters",
        fix: "shorten `description` to 1024 characters or fewer",
        kind: Spec,
        severity: Note,
        stability: Default,
    },
    Rule {
        code: "description-too-terse",
        label: "description too terse for discovery",
        fix: "expand `description` to say what the skill does and when to use it, not a bare noun phrase",
        kind: Spec,
        severity: Note,
        stability: Default,
    },
    Rule {
        code: "compatibility-invalid",
        label: "invalid or overlong compatibility field",
        fix: "set `compatibility:` to a non-empty string of 500 characters or fewer",
        kind: Spec,
        severity: Note,
        stability: Default,
    },
    Rule {
        code: "license-not-string",
        label: "non-string license field",
        fix: "make `license:` a plain string (for example `MIT` or `Apache-2.0`)",
        kind: Spec,
        severity: Note,
        stability: Default,
    },
    Rule {
        code: "metadata-not-map",
        label: "metadata is not a flat scalar map",
        fix: "make `metadata:` a flat YAML mapping of `key: value` scalar pairs",
        kind: Spec,
        severity: Note,
        stability: Default,
    },
    Rule {
        code: "allowed-tools-invalid",
        label: "invalid allowed-tools field",
        fix: "set `allowed-tools:` to a space-delimited string or a list of tool strings",
        kind: Spec,
        severity: Note,
        stability: Default,
    },
    Rule {
        code: "unknown-frontmatter-key",
        label: "unknown frontmatter key",
        fix: "remove the unrecognized key or move custom fields under `metadata:`",
        kind: Spec,
        severity: Note,
        stability: Default,
    },
    // Local file references (Asset).
    Rule {
        code: "broken-link",
        label: "broken relative link in SKILL.md",
        fix: "create the missing file or update the relative link target in SKILL.md",
        kind: Asset,
        severity: Warn,
        stability: Default,
    },
    Rule {
        code: "link-escapes-skill",
        label: "relative link escapes skill directory",
        fix: "keep linked files inside the skill directory so the skill stays self-contained",
        kind: Asset,
        severity: Warn,
        stability: Default,
    },
    Rule {
        code: "broken-symlink",
        label: "dangling symlink in skill directory",
        fix: "remove the dangling symlink or point it to an existing file inside the skill directory",
        kind: Asset,
        severity: Warn,
        stability: Default,
    },
    // Security / safety.
    Rule {
        code: "symlink-escape",
        label: "symlink escapes skill directory",
        fix: "replace the symlink with a file inside the skill directory",
        kind: Security,
        severity: Error,
        stability: Default,
    },
    Rule {
        code: "trojan-source",
        label: "hidden control or bidirectional character",
        fix: "remove hidden bidirectional, zero-width, or terminal-control characters from SKILL.md",
        kind: Security,
        severity: Error,
        stability: Default,
    },
    Rule {
        code: "exposed-secret",
        label: "exposed secret or credential literal",
        fix: "remove the credential literal from SKILL.md and reference an environment variable instead",
        kind: Security,
        severity: Error,
        stability: Default,
    },
    Rule {
        code: "unrestricted-bash",
        label: "unrestricted Bash tool permission",
        fix: "scope `allowed-tools:` to specific commands (for example `Bash(git:*)`) instead of `Bash(*)`",
        kind: Security,
        severity: Warn,
        stability: Default,
    },
    // Beta (corpus-level, staged behind `--beta`; graduate by setting stability to Default).
    Rule {
        code: "redundant-with",
        label: "near-duplicate of another skill",
        fix: "merge the two skills, or differentiate their descriptions so each has a distinct trigger",
        kind: Spec,
        severity: Warn,
        stability: Beta,
    },
    Rule {
        code: "superseded-by",
        label: "scope covered by a broader skill",
        fix: "retire the narrower skill, or narrow its description so it no longer overlaps",
        kind: Spec,
        severity: Warn,
        stability: Beta,
    },
];

/// The rule for `code`, if registered.
pub fn rule(code: &str) -> Option<&'static Rule> {
    RULES.iter().find(|r| r.code == code)
}

/// The human label for a rule code; a generic fallback for an unknown code.
pub fn label(code: &str) -> &'static str {
    rule(code).map_or("skill format issue", |r| r.label)
}

/// True while `code` is still staged behind `--beta`.
pub fn is_beta(code: &str) -> bool {
    rule(code).is_some_and(|r| r.stability == Stability::Beta)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_code_is_unique() {
        let mut seen = std::collections::HashSet::new();
        for r in RULES {
            assert!(seen.insert(r.code), "duplicate rule code: {}", r.code);
        }
    }

    #[test]
    fn every_rule_drives_problem_consistently() {
        // The registry is the single source of truth: Problem::new / kind / is_error for every code
        // must come straight off its row (and no code may trip the `unreachable!` in Problem::new).
        for r in RULES {
            let p = crate::parse::Problem::new(r.code, "msg");
            assert_eq!(p.fix, r.fix, "fix mismatch for {}", r.code);
            assert_eq!(p.kind(), r.kind, "kind mismatch for {}", r.code);
            assert_eq!(
                p.is_error(),
                matches!(r.severity, Severity::Error),
                "is_error mismatch for {}",
                r.code
            );
        }
    }

    #[test]
    fn lookups_resolve_and_fall_back() {
        assert_eq!(
            label("unrestricted-bash"),
            "unrestricted Bash tool permission"
        );
        assert_eq!(label("not-a-real-code"), "skill format issue");
        assert!(is_beta("redundant-with"));
        assert!(!is_beta("no-frontmatter"));
        assert!(!is_beta("not-a-real-code"));
    }
}
