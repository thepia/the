//! The single agent lint-rule registry: one row per rule, carrying every attribute (label, why, fix,
//! kind, severity, stability). [`lint`](crate::lint) and the command's issue labels read this one
//! table, instead of the same code being smeared across several `match` statements that drift. A
//! feature gate is a column (`stability`), not a parallel list. This mirrors the `the-skills` rule
//! registry deliberately: WORK-0036's "share the registry/rule machinery, specialize the key sets" is
//! the same metadata-table mechanism with agent-specific rows.

/// Category of an agent-definition problem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Prevents the runtime from registering or delegating to the agent.
    Breaking,
    /// Deviation from the agent-definition conventions (addressability, delegation quality, field validity).
    Spec,
    /// Security or safety hazard in the definition text.
    Security,
}

/// Diagnostic level a rule renders at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warn,
    Note,
}

/// Whether a rule is on by default or still staged behind `--beta`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stability {
    Default,
    /// Staged behind `--beta`; graduate it by flipping this to `Default`.
    Beta,
}

/// One lint rule: its stable `code` and everything keyed off that code.
#[derive(Debug, Clone, Copy)]
pub struct Rule {
    pub code: &'static str,
    /// WHAT is wrong (the command's issue label).
    pub label: &'static str,
    /// WHY it matters: the concrete consequence for loading, delegation, or trust.
    pub why: &'static str,
    /// HOW to fix it.
    pub fix: &'static str,
    pub kind: Kind,
    pub severity: Severity,
    pub stability: Stability,
}

use Kind::{Breaking, Security, Spec};
use Severity::{Error, Note, Warn};
use Stability::{Beta, Default};

/// The registry. One row per rule; adding or changing a rule is one row, and every consumer reads it.
///
/// Field value sets are grounded in the Claude Code subagent frontmatter and the Agent-tool definition
/// convention (`.claude/agents/*.md`, checked 2026-11-23). `model` is deliberately an open set (any
/// alias or concrete model id, or `inherit`), so its value is never enumerated: only an empty/non-string
/// value is flagged. `effort` is a closed set, so its values are checked.
pub const RULES: &[Rule] = &[
    // Loadability: the runtime cannot register or delegate to the agent.
    Rule {
        code: "agent-no-frontmatter",
        label: "missing YAML frontmatter",
        why: "an agent definition with no frontmatter block declares no name or description, so the runtime cannot register it",
        fix: "start the file with a `---` block holding `name:` and `description:`, closed by `---`",
        kind: Breaking,
        severity: Warn,
        stability: Default,
    },
    Rule {
        code: "agent-frontmatter-invalid",
        label: "invalid YAML frontmatter",
        why: "frontmatter that does not parse is dropped, so the agent is not registered",
        fix: "fix the YAML; a plain value containing `: ` must be quoted (`description: \"...\"`) or use a `>-` block, and keys must be unique",
        kind: Breaking,
        severity: Warn,
        stability: Default,
    },
    Rule {
        code: "agent-name-missing",
        label: "missing agent name",
        why: "the runtime registers and delegates to a subagent by its name; with none it cannot be addressed",
        fix: "set `name:` to lowercase letters, digits and single hyphens (its file stem is the convention)",
        kind: Breaking,
        severity: Warn,
        stability: Default,
    },
    Rule {
        code: "agent-name-invalid",
        label: "invalid agent name format",
        why: "a name with spaces, slashes or uppercase cannot be used as the delegation key, so the agent is unaddressable",
        fix: "set `name:` to 1-64 lowercase letters, digits and single hyphens",
        kind: Breaking,
        severity: Warn,
        stability: Default,
    },
    // Addressability and delegation quality.
    Rule {
        code: "agent-name-mismatch",
        label: "agent name differs from file name",
        why: "runtimes list and address the agent by its file name, so a divergent `name:` misleads discovery and delegation",
        fix: "make `name:` equal the file stem, or rename the file to match `name:`",
        kind: Spec,
        severity: Note,
        stability: Default,
    },
    Rule {
        code: "agent-description-missing",
        label: "missing delegation description",
        why: "the orchestrator delegates on the description; with none, it never routes work to this agent",
        fix: "add a `description:` saying what the agent does and when to delegate to it",
        kind: Spec,
        severity: Warn,
        stability: Default,
    },
    Rule {
        code: "agent-description-terse",
        label: "description too terse for delegation",
        why: "too little signal for the orchestrator to delegate on, so the agent is under-used or mis-triggered",
        fix: "expand `description` to say what the agent does and when to use it, not a bare noun phrase",
        kind: Spec,
        severity: Note,
        stability: Default,
    },
    Rule {
        code: "agent-description-vague",
        label: "vague, generic description",
        why: "filler like \"various tasks\" gives the orchestrator no concrete signal for when to delegate",
        fix: "replace the generic phrase with the concrete task and the trigger for delegating to the agent",
        kind: Spec,
        severity: Note,
        stability: Beta,
    },
    // Field validity.
    Rule {
        code: "agent-tools-invalid",
        label: "invalid tools field",
        why: "a malformed tools field is ignored, so the intended tool scope is not applied and the agent may get more access than meant",
        fix: "set `tools:` to a comma-delimited string (`Read, Grep, Bash`) or a YAML list of tool-name strings",
        kind: Spec,
        severity: Note,
        stability: Default,
    },
    Rule {
        code: "agent-model-empty",
        label: "empty or non-string model value",
        why: "an empty or non-string model value is not a valid model selector, so the intended model is not applied",
        fix: "set `model:` to a model alias (`sonnet`, `opus`, `haiku`), a concrete model id, or `inherit`",
        kind: Spec,
        severity: Note,
        stability: Default,
    },
    Rule {
        code: "agent-effort-invalid",
        label: "invalid effort value",
        why: "an invalid effort value is ignored, so the reasoning effort is not applied",
        fix: "set `effort:` to one of `low`, `medium`, `high`, `xhigh`, `max`",
        kind: Spec,
        severity: Note,
        stability: Default,
    },
    Rule {
        code: "agent-maxturns-invalid",
        label: "invalid maxTurns value",
        why: "a non-positive-integer maxTurns is ignored, so the turn cap it expresses is not enforced",
        fix: "set `maxTurns:` to a positive integer",
        kind: Spec,
        severity: Note,
        stability: Default,
    },
    Rule {
        code: "agent-ignored-field",
        // Driven by the key registry's `value: PluginIgnored` data (crate::keys): hooks / mcpServers /
        // permissionMode (and initialPrompt) are read on a LOCAL subagent but unsupported on a PLUGIN
        // subagent (ST-08; docs name the first three). So this fires only when the definition is
        // plugin-packaged (a `.claude-plugin/plugin.json` ancestor; agnix `CC-PL-014` scopes it the same
        // way), never on a local agent where the fields are honored. Default-on; the plugin restriction
        // is a hard, documented fact, not a hardening opinion.
        label: "field unsupported on a plugin subagent",
        why: "a plugin subagent does not support hooks/mcpServers/permissionMode (ST-08), so the field is silently ignored there (it is read only on a local subagent)",
        fix: "remove the field from the plugin subagent, or move its intent to the plugin's own config (hooks in hooks/hooks.json, servers in .mcp.json); keep the agent local if it must read the field",
        kind: Spec,
        severity: Warn,
        stability: Default,
    },
    Rule {
        code: "agent-unknown-key",
        // A closed key set is assistant- and tool-specific: other assistants and additional tools add
        // keys that do not disrupt the agent, so flagging "unknown" is a hardening opinion, not a
        // default correctness check. Staged behind `--beta` (off by default) until the `--strict`
        // hardening mode is designed; it then moves there. See crate::keys (scope note) and WORK-0036.
        label: "unknown frontmatter key",
        why: "an unrecognized key is ignored, so its intent silently has no effect",
        fix: "remove the unrecognized key, or correct a typo'd field name",
        kind: Spec,
        severity: Note,
        stability: Beta,
    },
    Rule {
        code: "agent-skill-not-found",
        // Reference integrity across the agents+skills config codebase: the agent's `skills:` entries
        // are resolved against the real skill catalog (the-skills). `the` is uniquely placed to do this
        // cross-assistant because it owns both sides (the agents and the skill catalog).
        label: "referenced skill not found",
        why: "the agent names a skill in `skills:` that resolves to no available skill, so it cannot load it",
        fix: "install the named skill, or correct the name in `skills:` to one `the skills list` shows",
        kind: Spec,
        severity: Warn,
        stability: Default,
    },
    // Security (detection reused from the-skills smoke; agent-worded here).
    Rule {
        code: "trojan-source",
        label: "hidden control or bidirectional character",
        why: "hidden bidirectional or zero-width characters can conceal instructions from a human reviewing the agent definition",
        fix: "remove hidden bidirectional, zero-width, or terminal-control characters from the definition",
        kind: Security,
        severity: Error,
        stability: Default,
    },
    Rule {
        code: "exposed-secret",
        label: "exposed secret or credential literal",
        why: "a committed credential in an agent definition leaks a secret to anyone who reads it",
        fix: "remove the credential literal and reference an environment variable instead",
        kind: Security,
        severity: Error,
        stability: Default,
    },
];

/// The rule for `code`, if registered.
pub fn rule(code: &str) -> Option<&'static Rule> {
    RULES.iter().find(|r| r.code == code)
}

/// The human label for a rule code; a generic fallback for an unknown code.
pub fn label(code: &str) -> &'static str {
    rule(code).map_or("agent definition issue", |r| r.label)
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
    fn every_rule_states_what_why_and_fix() {
        for r in RULES {
            assert!(!r.label.trim().is_empty(), "empty label for {}", r.code);
            assert!(!r.why.trim().is_empty(), "empty why for {}", r.code);
            assert!(!r.fix.trim().is_empty(), "empty fix for {}", r.code);
        }
    }

    #[test]
    fn lookups_resolve_and_fall_back() {
        assert_eq!(label("agent-name-missing"), "missing agent name");
        assert_eq!(label("not-a-real-code"), "agent definition issue");
        assert!(is_beta("agent-description-vague"));
        assert!(!is_beta("agent-name-missing"));
        assert!(!is_beta("not-a-real-code"));
    }
}
