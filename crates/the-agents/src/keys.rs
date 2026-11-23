//! The agent-definition frontmatter **key registry**: one row per known key, carrying its purpose,
//! which assistants/tools read it, how its value is validated, and the risk if it is wrong. This is the
//! single source of truth for "is this key known, what is it for, who uses it, how to check its value"
//! (the metadata-table-drives-functions principle): [`crate::lint`] reads this table instead of a
//! scattered `KNOWN_KEYS` list plus one `match` arm per field.
//!
//! Scope note: `the agents` is a **general** agents+skills config manager, not Claude-specific. Keys are
//! therefore recorded with their consumers ([`used_by`](KeyProp::used_by)), so that when the
//! `--assistant` dimension lands the same table can scope "known / expected" per assistant. Until then
//! discovery only scans Claude's `.claude/agents`, and the `agent-unknown-key` check is off by default
//! (staged behind `--beta`) precisely because a closed key set is assistant- and tool-specific: other
//! assistants and additional tools add keys that do not disrupt the agent, so flagging "unknown" is a
//! hardening opinion, not a default correctness check.

/// Who reads a frontmatter key. An empty [`used_by`](KeyProp::used_by) slice means the key is parsed but
/// **ignored by the loader** (it looks valid yet has no effect), which is a distinct finding from an
/// unknown key (a typo) once the hardening checks are enabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Consumer {
    /// A coding assistant that reads this field when loading the agent.
    Assistant(&'static str),
    /// A tool / integration that reads this field (not the assistant itself). The forward slot for your
    /// point that additional tools add their own keys that do not disrupt the agent; no current row uses
    /// it (populated when the first tool-owned key is pinned), so it is `allow(dead_code)` for now.
    #[allow(dead_code)]
    Tool(&'static str),
}

/// How a key's value is validated. The dedicated name/description checks in [`crate::lint`] own the
/// `Name` / `Text` keys; the generic field pass validates the rest off this descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueCheck {
    /// The agent name (kebab-case 1-64); validated by `check_name`, skipped by the generic pass.
    Name,
    /// Free-text delegation description; quality validated by `check_description`, skipped generically.
    Text,
    /// Any non-empty string (e.g. `model`: an open set of aliases / ids / `inherit`).
    OpenString,
    /// A closed set of allowed string values (e.g. `effort`).
    Closed(&'static [&'static str]),
    /// A boolean (recorded as the field's true type; enforcement is deferred, see per-row `invalid_code`).
    Bool,
    /// A positive integer (e.g. `maxTurns`).
    PosInt,
    /// A space/comma-delimited string OR a list of strings (e.g. `tools`).
    StringOrList,
    /// Any string; no value constraint (e.g. `color`).
    AnyString,
    /// The loader ignores this key; no value constraint. Recorded so it is not reported as "unknown".
    Ignored,
}

/// One frontmatter key and everything keyed off it.
#[derive(Debug, Clone, Copy)]
pub struct KeyProp {
    pub key: &'static str,
    /// What the field does.
    pub purpose: &'static str,
    /// The assistants/tools that read it. Empty = parsed but ignored by the loader.
    pub used_by: &'static [Consumer],
    /// How the value is validated.
    pub value: ValueCheck,
    /// The lint code emitted when `value` fails, or `""` when the field carries no generic value check
    /// (its validity is handled elsewhere, is unconstrained, or its enforcement is deferred).
    pub invalid_code: &'static str,
    /// The consequence if the field is wrong or misused (the "risk" column).
    pub risk: &'static str,
}

use Consumer::Assistant;
use ValueCheck::{AnyString, Bool, Closed, Ignored, Name, OpenString, PosInt, StringOrList, Text};

/// Claude Code `effort:` levels (closed set; frontmatter reference, checked 2026-11-23).
pub const EFFORT_LEVELS: &[&str] = &["low", "medium", "high", "xhigh", "max"];

const CLAUDE: Consumer = Assistant("claude");

/// The registry. One row per known key. Grounded in the Claude Code subagent frontmatter and the
/// Agent-tool definition convention (checked 2026-11-23); `used_by` is forward metadata for the
/// `--assistant` dimension. Add a row to teach the tool a new key; a value constraint is a column, not a
/// new `match` arm.
pub const KEYS: &[KeyProp] = &[
    KeyProp {
        key: "name",
        purpose: "the addressable id the orchestrator delegates to",
        used_by: &[
            CLAUDE,
            Assistant("opencode"),
            Assistant("gemini"),
            Assistant("goose"),
        ],
        value: Name,
        invalid_code: "",
        risk: "unaddressable agent if malformed",
    },
    KeyProp {
        key: "description",
        purpose: "the delegation/routing signal (when to hand work to this agent)",
        used_by: &[
            CLAUDE,
            Assistant("opencode"),
            Assistant("gemini"),
            Assistant("goose"),
        ],
        value: Text,
        invalid_code: "",
        risk: "never delegated to if missing or too thin",
    },
    KeyProp {
        key: "tools",
        purpose: "the tool allow-list the agent may use",
        used_by: &[CLAUDE],
        value: StringOrList,
        invalid_code: "agent-tools-invalid",
        risk: "over-broad tool access if malformed or omitted",
    },
    KeyProp {
        key: "disallowedTools",
        purpose: "the tool deny-list subtracted from the allow-list",
        used_by: &[CLAUDE],
        value: StringOrList,
        invalid_code: "agent-tools-invalid",
        risk: "intended restriction not applied if malformed",
    },
    KeyProp {
        key: "model",
        purpose: "the model selector (alias, concrete id, or `inherit`)",
        used_by: &[CLAUDE, Assistant("opencode")],
        value: OpenString,
        invalid_code: "agent-model-empty",
        risk: "intended model not applied if empty/non-string",
    },
    KeyProp {
        key: "effort",
        purpose: "the reasoning-effort level",
        used_by: &[CLAUDE],
        value: Closed(EFFORT_LEVELS),
        invalid_code: "agent-effort-invalid",
        risk: "effort not applied if invalid",
    },
    KeyProp {
        key: "maxTurns",
        purpose: "the cap on agent turns",
        used_by: &[CLAUDE],
        value: PosInt,
        invalid_code: "agent-maxturns-invalid",
        risk: "turn cap not enforced if not a positive integer",
    },
    KeyProp {
        key: "memory",
        purpose: "memory scoping for the agent",
        used_by: &[CLAUDE],
        value: AnyString,
        invalid_code: "",
        risk: "low",
    },
    KeyProp {
        key: "background",
        purpose: "run the agent in the background",
        used_by: &[CLAUDE],
        value: Bool,
        invalid_code: "",
        risk: "low",
    },
    KeyProp {
        key: "isolation",
        purpose: "run the agent in an isolated git worktree",
        used_by: &[CLAUDE],
        value: AnyString,
        invalid_code: "",
        risk: "low",
    },
    KeyProp {
        key: "color",
        purpose: "display colour in the agent list",
        used_by: &[CLAUDE],
        value: AnyString,
        invalid_code: "",
        risk: "cosmetic",
    },
    KeyProp {
        key: "skills",
        purpose: "skills the agent is allowed to load",
        used_by: &[CLAUDE],
        value: StringOrList,
        invalid_code: "",
        risk: "named skill not available if unresolved",
    },
    KeyProp {
        key: "omitClaudeMd",
        purpose: "skip loading CLAUDE.md for this agent",
        used_by: &[CLAUDE],
        value: Bool,
        invalid_code: "",
        risk: "low",
    },
    // Fields that look valid but the Claude plugin loader silently ignores (agent37's linter flags
    // these). Recorded with an empty `used_by` so they are NOT reported as unknown; emitting a
    // "declared but ignored" finding is deferred to the hardening (`--strict`) design.
    KeyProp {
        key: "permissionMode",
        purpose: "(declared but ignored by the subagent loader)",
        used_by: &[],
        value: Ignored,
        invalid_code: "",
        risk: "silently has no effect",
    },
    KeyProp {
        key: "hooks",
        purpose: "(declared but ignored by the subagent loader)",
        used_by: &[],
        value: Ignored,
        invalid_code: "",
        risk: "silently has no effect",
    },
    KeyProp {
        key: "mcpServers",
        purpose: "(declared but ignored by the subagent loader)",
        used_by: &[],
        value: Ignored,
        invalid_code: "",
        risk: "silently has no effect",
    },
    KeyProp {
        key: "initialPrompt",
        purpose: "(declared but ignored by the subagent loader)",
        used_by: &[],
        value: Ignored,
        invalid_code: "",
        risk: "silently has no effect",
    },
];

/// The prop for `key`, if it is a known frontmatter key.
pub fn prop(key: &str) -> Option<&'static KeyProp> {
    KEYS.iter().find(|p| p.key == key)
}

/// Whether the loader reads `key` (false = parsed but ignored, i.e. empty `used_by`).
#[allow(dead_code)]
pub fn is_consumed(prop: &KeyProp) -> bool {
    !prop.used_by.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_key_is_unique() {
        let mut seen = std::collections::HashSet::new();
        for p in KEYS {
            assert!(seen.insert(p.key), "duplicate key: {}", p.key);
        }
    }

    #[test]
    fn every_value_check_code_resolves_to_a_rule() {
        // A key's `invalid_code`, when set, must be a real lint rule (the table drives the findings).
        for p in KEYS {
            if !p.invalid_code.is_empty() {
                assert!(
                    crate::rules::rule(p.invalid_code).is_some(),
                    "{} references unknown rule `{}`",
                    p.key,
                    p.invalid_code
                );
            }
        }
    }

    #[test]
    fn the_real_claude_subagent_fields_are_known() {
        // The schema gap that caused `agent-unknown-key` false positives: these must all be known now.
        for k in [
            "name",
            "description",
            "tools",
            "disallowedTools",
            "model",
            "effort",
            "maxTurns",
            "memory",
            "background",
            "isolation",
            "color",
            "skills",
            "omitClaudeMd",
        ] {
            assert!(prop(k).is_some(), "`{k}` must be a known key");
        }
    }

    #[test]
    fn ignored_fields_have_empty_used_by() {
        for k in ["permissionMode", "hooks", "mcpServers", "initialPrompt"] {
            let p = prop(k).unwrap();
            assert!(!is_consumed(p), "`{k}` should be recorded as ignored");
        }
    }
}
