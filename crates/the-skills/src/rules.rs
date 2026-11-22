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
    /// Human one-line label: WHAT is wrong (the CLI's issue label).
    pub label: &'static str,
    /// WHY it matters: the concrete consequence for loading, routing, running, or trust.
    pub why: &'static str,
    /// The concrete fix advice: HOW to resolve it.
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
        why: "an agent cannot discover or load a skill with no frontmatter block",
        fix: "start SKILL.md with a `---` block holding `name:` and `description:`, closed by `---`",
        kind: Breaking,
        severity: Warn,
        stability: Default,
    },
    Rule {
        code: "frontmatter-unterminated",
        label: "unterminated YAML frontmatter",
        why: "the unterminated block makes the file unparseable, so the skill never loads",
        fix: "close the frontmatter with a `---` line",
        kind: Breaking,
        severity: Warn,
        stability: Default,
    },
    Rule {
        code: "frontmatter-invalid",
        label: "invalid YAML frontmatter",
        why: "a strict agent (Goose, OpenCode) skips a skill whose frontmatter does not parse",
        fix: "fix the YAML; a plain value containing `: ` must be quoted (`description: \"...\"`) or use a `>-` block",
        kind: Breaking,
        severity: Warn,
        stability: Default,
    },
    Rule {
        code: "duplicate-frontmatter-key",
        label: "duplicate frontmatter key",
        why: "a strict parser rejects the skill; a lenient one silently drops the earlier value",
        fix: "remove the duplicate key; a strict YAML parser rejects it and a lenient one keeps only the last value",
        kind: Breaking,
        severity: Warn,
        stability: Default,
    },
    Rule {
        code: "name-missing",
        label: "missing skill name",
        why: "Goose and OpenCode cannot load a skill with no name",
        fix: "set `name:` to lowercase letters, digits and single hyphens",
        kind: Breaking,
        severity: Warn,
        stability: Default,
    },
    Rule {
        code: "name-has-slash",
        label: "skill name contains '/'",
        why: "a `/` in the name breaks the command path, so the skill cannot be invoked",
        fix: "remove the `/` from `name:`",
        kind: Breaking,
        severity: Warn,
        stability: Default,
    },
    Rule {
        code: "description-not-string",
        label: "non-string description",
        why: "OpenCode skips a skill whose description is not a string",
        fix: "make `description:` a plain string",
        kind: Breaking,
        severity: Warn,
        stability: Default,
    },
    // Spec conformance (agentskills.io deviations; rendered as notes).
    Rule {
        code: "name-format",
        label: "invalid skill name format",
        why: "a non-conforming name is listed or invoked inconsistently across agents",
        fix: "set `name:` to lowercase letters, digits and single hyphens",
        kind: Spec,
        severity: Note,
        stability: Default,
    },
    Rule {
        code: "name-mismatch",
        label: "skill name differs from directory",
        why: "agents list the skill by its directory, so a mismatched name misleads discovery and `/name` invocation",
        fix: "make `name:` equal the directory name, or rename the directory",
        kind: Spec,
        severity: Note,
        stability: Default,
    },
    Rule {
        code: "description-missing",
        label: "missing description",
        why: "the agent routes on the description; with none it cannot decide when to use the skill",
        fix: "add a `description:` saying what the skill does and when to use it",
        kind: Spec,
        severity: Note,
        stability: Default,
    },
    Rule {
        code: "description-too-long",
        label: "description exceeds 1024 characters",
        why: "an over-budget description is truncated, losing the routing signal",
        fix: "shorten `description` to 1024 characters or fewer",
        kind: Spec,
        severity: Note,
        stability: Default,
    },
    Rule {
        code: "description-too-terse",
        label: "description too terse for discovery",
        why: "too little signal for the agent to route on, so the skill is under-used or mis-triggered",
        fix: "expand `description` to say what the skill does and when to use it, not a bare noun phrase",
        kind: Spec,
        severity: Note,
        stability: Default,
    },
    Rule {
        code: "description-echoes-name",
        label: "description only repeats the skill name",
        why: "a description that only restates the name adds no routing information",
        fix: "describe what the skill does and when to use it, beyond restating its name",
        kind: Spec,
        severity: Note,
        stability: Default,
    },
    Rule {
        code: "description-vague-generic",
        label: "vague, generic description",
        why: "filler like \"various tasks\" gives an agent no concrete signal for when to load the skill",
        fix: "replace the generic phrase with the concrete task and the trigger for using the skill",
        kind: Spec,
        severity: Note,
        stability: Beta,
    },
    Rule {
        code: "unfinished-skill",
        label: "unfinished skill (placeholder content)",
        why: "a placeholder marker means the skill was left incomplete and misbehaves if invoked",
        fix: "remove the placeholder/template markers and complete the skill's instructions",
        kind: Spec,
        severity: Note,
        stability: Default,
    },
    Rule {
        code: "compatibility-invalid",
        label: "invalid or overlong compatibility field",
        why: "an invalid compatibility field is ignored, so the constraint it expresses is lost",
        fix: "set `compatibility:` to a non-empty string of 500 characters or fewer",
        kind: Spec,
        severity: Note,
        stability: Default,
    },
    Rule {
        code: "license-not-string",
        label: "non-string license field",
        why: "a non-string value is not a valid license declaration",
        fix: "make `license:` a plain string (for example `MIT` or `Apache-2.0`)",
        kind: Spec,
        severity: Note,
        stability: Default,
    },
    Rule {
        code: "metadata-not-map",
        label: "metadata is not a flat scalar map",
        why: "non-scalar metadata is not portable and may be dropped by agents",
        fix: "make `metadata:` a flat YAML mapping of `key: value` scalar pairs",
        kind: Spec,
        severity: Note,
        stability: Default,
    },
    Rule {
        code: "allowed-tools-invalid",
        label: "invalid allowed-tools field",
        why: "a malformed allowed-tools field is ignored, so the intended tool scope is not applied",
        fix: "set `allowed-tools:` to a space-delimited string or a list of tool strings",
        kind: Spec,
        severity: Note,
        stability: Default,
    },
    Rule {
        code: "list-field-invalid",
        label: "invalid string-or-list field",
        why: "a field that should be a string or list of strings is ignored when it is neither, so its intent has no effect",
        fix: "set the field to a space/comma-delimited string or a list of strings (`disallowed-tools`, `paths`, `arguments`)",
        kind: Spec,
        severity: Note,
        stability: Default,
    },
    Rule {
        code: "unknown-frontmatter-key",
        label: "unknown frontmatter key",
        why: "an unrecognized key is ignored, so its intent silently has no effect",
        fix: "remove the unrecognized key or move custom fields under `metadata:`",
        kind: Spec,
        severity: Note,
        stability: Default,
    },
    Rule {
        code: "description-angle-brackets",
        label: "angle brackets in description",
        why: "Codex's parser breaks on `<`/`>`, so the skill fails to load there",
        fix: "remove `<`/`>` from `description`; they break Codex's skill parser (keep it plain prose)",
        kind: Spec,
        severity: Note,
        stability: Default,
    },
    // Claude Code extended frontmatter field values (code.claude.com/docs/en/skills, checked 2026-11-22).
    // Closed-set / type / dependency checks only; `model` and `agent` are open sets and are NOT enumerated.
    Rule {
        code: "context-invalid",
        label: "invalid context value",
        why: "an invalid context value is ignored, so the skill does not fork as intended",
        fix: "set `context:` to `fork`, or omit it for the default inline behavior",
        kind: Spec,
        severity: Note,
        stability: Default,
    },
    Rule {
        code: "effort-invalid",
        label: "invalid effort value",
        why: "an invalid effort value is ignored, so the effort level is not applied",
        fix: "set `effort:` to one of `low`, `medium`, `high`, `xhigh`, `max`",
        kind: Spec,
        severity: Note,
        stability: Default,
    },
    Rule {
        code: "agent-without-context",
        label: "agent without context: fork",
        why: "without `context: fork` the agent field is ignored, so the chosen subagent is never used",
        fix: "add `context: fork` (the `agent:` field is only used by a forked skill), or remove `agent:`",
        kind: Spec,
        severity: Note,
        stability: Default,
    },
    Rule {
        code: "non-boolean-field",
        label: "non-boolean value for a boolean field",
        why: "a non-boolean value for a boolean field is misread, changing the skill's behavior",
        fix: "set the field to a boolean (`true`/`false`, `yes`/`no`, `on`/`off`, or `1`/`0`)",
        kind: Spec,
        severity: Note,
        stability: Default,
    },
    Rule {
        code: "background-without-context",
        label: "background without context: fork",
        why: "without `context: fork` the background field is ignored, so the skill does not run in the background",
        fix: "add `context: fork` (the `background:` field only applies to a forked skill), or remove `background:`",
        kind: Spec,
        severity: Note,
        stability: Default,
    },
    Rule {
        code: "shell-invalid",
        label: "invalid shell value",
        why: "an invalid shell value is ignored, so `!` commands do not run in the intended shell",
        fix: "set `shell:` to `bash`, `powershell`, or `0` (to turn the shell tool off)",
        kind: Spec,
        severity: Note,
        stability: Default,
    },
    // Local file references (Asset).
    Rule {
        code: "broken-link",
        label: "broken relative link in SKILL.md",
        why: "a link to a missing file fails when the agent follows it mid-task",
        fix: "create the missing file or update the relative link target in SKILL.md",
        kind: Asset,
        severity: Warn,
        stability: Default,
    },
    Rule {
        code: "link-escapes-skill",
        label: "relative link escapes skill directory",
        why: "a link outside the skill directory is not self-contained and breaks when the skill moves",
        fix: "keep linked files inside the skill directory so the skill stays self-contained",
        kind: Asset,
        severity: Warn,
        stability: Default,
    },
    Rule {
        code: "non-portable-path",
        label: "absolute path in a link",
        why: "an absolute path does not resolve on another machine, so the skill breaks for teammates",
        fix: "use a relative link inside the skill directory so it resolves on any machine",
        kind: Asset,
        severity: Warn,
        stability: Default,
    },
    Rule {
        code: "broken-symlink",
        label: "dangling symlink in skill directory",
        why: "a dangling symlink fails when resolved and signals a missing asset",
        fix: "remove the dangling symlink or point it to an existing file inside the skill directory",
        kind: Asset,
        severity: Warn,
        stability: Default,
    },
    Rule {
        code: "broken-link-fragment",
        label: "link anchor matches no heading",
        why: "a `#anchor` with no matching heading lands the reader at the top of the file, not the section",
        fix: "point the `#anchor` at an existing heading in the target file, or drop the fragment",
        kind: Asset,
        severity: Warn,
        stability: Beta,
    },
    Rule {
        code: "orphan-unreferenced-file",
        label: "skill file referenced by nothing",
        why: "a file no instruction mentions is dead weight: either a forgotten reference or a leftover to delete",
        fix: "reference the file from SKILL.md (or another doc), or remove it from the skill directory",
        kind: Asset,
        severity: Note,
        stability: Beta,
    },
    Rule {
        code: "unclosed-code-fence",
        label: "unclosed code fence in SKILL.md",
        why: "everything after an unclosed fence is read as code, so later links and instructions vanish",
        fix: "close the ``` (or ~~~) fence; everything after an unclosed fence is read as code",
        kind: Spec,
        severity: Warn,
        stability: Default,
    },
    // Security / safety.
    Rule {
        code: "symlink-escape",
        label: "symlink escapes skill directory",
        why: "a symlink leaving the skill directory can read or expose files outside it",
        fix: "replace the symlink with a file inside the skill directory",
        kind: Security,
        severity: Error,
        stability: Default,
    },
    Rule {
        code: "trojan-source",
        label: "hidden control or bidirectional character",
        why: "hidden bidirectional or zero-width characters can conceal instructions from a human reviewer",
        fix: "remove hidden bidirectional, zero-width, or terminal-control characters from SKILL.md",
        kind: Security,
        severity: Error,
        stability: Default,
    },
    Rule {
        code: "exposed-secret",
        label: "exposed secret or credential literal",
        why: "a committed credential leaks a secret to anyone who reads the skill",
        fix: "remove the credential literal from SKILL.md and reference an environment variable instead",
        kind: Security,
        severity: Error,
        stability: Default,
    },
    Rule {
        code: "unrestricted-bash",
        label: "unrestricted Bash tool permission",
        why: "an unrestricted Bash grant lets the skill run any command, far beyond least privilege",
        fix: "scope `allowed-tools:` to specific commands (for example `Bash(git:*)`) instead of `Bash(*)`",
        kind: Security,
        severity: Warn,
        stability: Default,
    },
    // Beta (staged behind `--beta`; graduate by setting stability to Default).
    Rule {
        code: "trailing-newline",
        label: "missing or extra trailing newline",
        why: "cosmetic only: a noisy diff, with no effect on how the skill loads or runs",
        fix: "end SKILL.md with exactly one trailing newline",
        kind: Spec,
        severity: Note,
        stability: Beta,
    },
    Rule {
        code: "redundant-with",
        label: "near-duplicate of another skill",
        why: "a near-duplicate skill bloats the agent's context and splits routing between two skills",
        fix: "merge the two skills, or differentiate their descriptions so each has a distinct trigger",
        kind: Spec,
        severity: Warn,
        stability: Beta,
    },
    Rule {
        code: "superseded-by",
        label: "scope covered by a broader skill",
        why: "a skill whose scope a broader one already covers adds redundant context",
        fix: "retire the narrower skill, or narrow its description so it no longer overlaps",
        kind: Spec,
        severity: Warn,
        stability: Beta,
    },
    Rule {
        code: "skill-overlap",
        label: "descriptions overlap enough to confuse routing",
        why: "two similar descriptions make the agent pick between the skills at random, so the wrong one may fire",
        fix: "differentiate the two descriptions (distinct task, trigger, or domain terms) so each routes uniquely",
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
            assert_eq!(p.why, r.why, "why mismatch for {}", r.code);
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
    fn every_rule_states_what_why_and_fix() {
        // The "clear, fixable diagnostics" promise: every finding carries a non-empty label (what),
        // why (impact), and fix (how). An empty one would be a silent gap, so it is a hard failure.
        for r in RULES {
            assert!(!r.label.trim().is_empty(), "empty label for {}", r.code);
            assert!(!r.why.trim().is_empty(), "empty why for {}", r.code);
            assert!(!r.fix.trim().is_empty(), "empty fix for {}", r.code);
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
