//! Linting one agent definition: field validity, delegation quality, and reused security smoke.
//!
//! Each check emits a [`Finding`] carrying a rule `code` from [`crate::rules`]; the command renders a
//! finding into a `the-output` diagnostic via [`diagnostic`]. The frontmatter parser and the
//! security-smoke detection are reused from `the-skills` (the generic lint core); the agent-specific
//! field rules live here.

use std::path::Path;

use serde::Serialize;
use serde_json::Value;
use the_output::diag::{Diagnostic, Exit};
use the_skills::parse::{Parsed, name_format_ok};

use crate::keys;
use crate::rules::{self, Severity};
use crate::{Assistant, assistants};

/// A single lint finding against an agent definition: the rule `code` (its `why`/`fix`/severity come
/// from the [registry](crate::rules)), a specific message, and an optional source location.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Finding {
    pub code: &'static str,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub col: Option<usize>,
}

impl Finding {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            line: None,
            col: None,
        }
    }

    pub fn is_error(&self) -> bool {
        matches!(
            rules::rule(self.code).map(|r| r.severity),
            Some(Severity::Error)
        )
    }

    pub fn is_beta(&self) -> bool {
        rules::is_beta(self.code)
    }
}

/// A present description is too terse when it is BOTH short and sparse: a bare noun phrase the
/// orchestrator cannot delegate on. Mirrors the-skills' terse heuristic.
const DESCRIPTION_MIN_WORDS: usize = 3;
const DESCRIPTION_TERSE_MAX: usize = 40;

/// Conservative generic-filler phrases for the beta `agent-description-vague` rule. Kept short to avoid
/// false positives on legitimately broad agents; it is opt-in (`--beta`) while staged.
const VAGUE_PHRASES: &[&str] = &[
    "various tasks",
    "all sorts of",
    "many things",
    "as needed",
    "anything and everything",
];

/// Lint one parsed agent definition. `raw` is the file text (for security smoke), `file_stem` is the
/// file name without `.md` (for the name-vs-file check), `parent` is the directory the file sits in, and
/// `assistant` is the runtime whose directory it came from. Returns every finding, including beta-staged
/// ones; the caller filters by `--beta`.
///
/// The **universal** checks (frontmatter parse, security smoke) run for every assistant. The
/// Claude-specific field rules (name/description quality, the Claude key schema) run only on Claude
/// definitions, because other assistants' schemas differ (OpenCode derives the name from the filename
/// and uses object-valued `tools`; Gemini uses `kind`/`mcpServers`; pi uses `thinking`). Linting them
/// with Claude rules would reintroduce false positives; each assistant's own rules light up as added.
pub fn lint(
    parsed: &Parsed,
    raw: &str,
    file_stem: &str,
    parent: &Path,
    assistant: &'static Assistant,
) -> Vec<Finding> {
    let mut out = Vec::new();

    // A frontmatter parse/structure error means no fields can be trusted: report it and stop. Universal.
    if let Some(p) = &parsed.frontmatter_error {
        let code = if p.code == "no-frontmatter" {
            "agent-no-frontmatter"
        } else {
            "agent-frontmatter-invalid"
        };
        out.push(Finding::new(code, p.message.clone()));
        return out;
    }

    // Claude-specific field rules: only for Claude definitions (see the doc comment).
    if assistant.id == assistants::claude().id {
        check_name(parsed, file_stem, &mut out);
        check_description(parsed, &mut out);
        check_fields(parsed, &mut out);
    }
    // Security smoke is universal (a secret or hidden control char is a hazard in any definition).
    check_security(parsed, raw, parent, &mut out);
    out
}

fn check_name(parsed: &Parsed, file_stem: &str, out: &mut Vec<Finding>) {
    match parsed.str_field("name").map(str::trim) {
        None | Some("") => {
            out.push(Finding::new(
                "agent-name-missing",
                "no `name:` in the frontmatter",
            ));
        }
        Some(name) if !name_format_ok(name) => {
            out.push(Finding::new(
                "agent-name-invalid",
                format!("`{name}` is not 1-64 lowercase letters, digits and single hyphens"),
            ));
        }
        Some(name) if name != file_stem => {
            out.push(Finding::new(
                "agent-name-mismatch",
                format!("`name: {name}` differs from the file name `{file_stem}`"),
            ));
        }
        Some(_) => {}
    }
}

fn check_description(parsed: &Parsed, out: &mut Vec<Finding>) {
    let Some(desc) = parsed.str_field("description").map(str::trim) else {
        // A non-string description is caught here as missing too (routing still cannot use it).
        out.push(Finding::new(
            "agent-description-missing",
            "no `description:` string in the frontmatter",
        ));
        return;
    };
    if desc.is_empty() {
        out.push(Finding::new(
            "agent-description-missing",
            "the `description:` is empty",
        ));
        return;
    }
    let words = desc
        .split_whitespace()
        .filter(|w| w.chars().any(char::is_alphanumeric))
        .count();
    if desc.chars().count() < DESCRIPTION_TERSE_MAX && words < DESCRIPTION_MIN_WORDS {
        out.push(Finding::new(
            "agent-description-terse",
            format!("the description is only {words} word(s): `{desc}`"),
        ));
    }
    let lower = desc.to_lowercase();
    if let Some(phrase) = VAGUE_PHRASES.iter().find(|p| lower.contains(**p)) {
        out.push(Finding::new(
            "agent-description-vague",
            format!("the description leans on the generic phrase `{phrase}`"),
        ));
    }
}

/// Validate every frontmatter key off the [key registry](crate::keys): a key with no prop is unknown
/// (the `agent-unknown-key` rule, staged behind `--beta`); a known key with a value constraint is
/// checked against it. `name`/`description` carry their own dedicated checks and are skipped here.
fn check_fields(parsed: &Parsed, out: &mut Vec<Finding>) {
    for (key, val) in &parsed.frontmatter {
        match keys::prop(key) {
            None => out.push(Finding::new(
                "agent-unknown-key",
                format!("`{key}` is not a known agent frontmatter key"),
            )),
            Some(prop) => check_value(prop, key, val, out),
        }
    }
}

/// Check one known key's value against its [`ValueCheck`](crate::keys::ValueCheck). Emits the key's
/// `invalid_code` on failure; keys with no generic value check (name/description handled elsewhere,
/// unconstrained, or deferred-enforcement) carry an empty code and never fire here.
fn check_value(prop: &keys::KeyProp, key: &str, val: &Value, out: &mut Vec<Finding>) {
    use keys::ValueCheck::{
        AnyString, Bool, Closed, Ignored, Name, OpenString, PosInt, StringOrList, Text,
    };
    let (ok, message) = match prop.value {
        // Handled by check_name / check_description, or carrying no value constraint.
        Name | Text | AnyString | Bool | Ignored => return,
        OpenString => (
            matches!(val, Value::String(s) if !s.trim().is_empty()),
            format!("`{key}:` is empty or not a string"),
        ),
        Closed(set) => (val.as_str().is_some_and(|s| set.contains(&s)), {
            let shown = val
                .as_str()
                .map_or_else(|| val.to_string(), ToOwned::to_owned);
            format!("`{key}: {shown}` is not one of {}", set.join(", "))
        }),
        PosInt => (
            val.as_u64().is_some_and(|n| n > 0),
            format!("`{key}: {val}` is not a positive integer"),
        ),
        StringOrList => (
            match val {
                Value::String(s) => !s.trim().is_empty(),
                Value::Array(a) => a.iter().all(Value::is_string),
                _ => false,
            },
            format!("`{key}:` is neither a non-empty string nor a list of strings"),
        ),
    };
    if !ok && !prop.invalid_code.is_empty() {
        out.push(Finding::new(prop.invalid_code, message));
    }
}

/// Reuse the-skills' text-level security smoke (hidden control/bidi characters and credential
/// literals). The skill smoke also scans the directory for escaping symlinks and inspects
/// `allowed-tools`; those are skill-directory concepts, so only the file-text findings
/// (`trojan-source`, `exposed-secret`) are kept here.
fn check_security(parsed: &Parsed, raw: &str, parent: &Path, out: &mut Vec<Finding>) {
    for p in the_skills::smoke::check_smoke(raw, parsed, parent) {
        // The reused detection carries a skill-framed message (it names SKILL.md); restate it for an
        // agent definition. The precise location travels in `line`/`col` (rendered as `path:line:col`),
        // so the message stays short and surface-agnostic.
        let message = match p.code {
            "trojan-source" => "a hidden control or bidirectional character is present",
            "exposed-secret" => "a credential literal is present",
            _ => continue,
        };
        let mut f = Finding::new(p.code, message);
        f.line = p.line;
        f.col = p.col;
        out.push(f);
    }
}

/// Render a finding as a `the-output` diagnostic: the what (label) leads the title, the specific
/// message and the rule's why form the cause, the fix is the help, and the source location (with any
/// `:line:col`) is the context, so it renders as a clickable `path:line:col`.
pub fn diagnostic(f: &Finding, agent_name: &str, path: &Path) -> Diagnostic {
    let rule = rules::rule(f.code);
    let label = rule.map_or("agent definition issue", |r| r.label);
    let why = rule.map(|r| r.why);
    let fix = rule.map(|r| r.fix);
    let severity = rule.map_or(Severity::Note, |r| r.severity);

    let title = format!("agent `{agent_name}`: {label}");
    let d = match severity {
        Severity::Error => Diagnostic::error(f.code, title).exit(Exit::Generic),
        Severity::Warn => Diagnostic::warning(f.code, title),
        Severity::Note => Diagnostic::note(f.code, title),
    };
    let location = match (f.line, f.col) {
        (Some(line), Some(col)) => format!("{}:{line}:{col}", path.display()),
        (Some(line), None) => format!("{}:{line}", path.display()),
        _ => path.display().to_string(),
    };
    let cause = match why {
        Some(why) => format!("{}; {why}", f.message),
        None => f.message.clone(),
    };
    let d = d.context(location).cause(cause);
    match fix {
        Some(fix) => d.help(fix),
        None => d,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use the_skills::parse::parse;

    fn codes(fm_and_body: &str, stem: &str) -> Vec<&'static str> {
        let parsed = parse(fm_and_body);
        lint(
            &parsed,
            fm_and_body,
            stem,
            Path::new("/tmp"),
            assistants::claude(),
        )
        .into_iter()
        .map(|f| f.code)
        .collect()
    }

    #[test]
    fn a_clean_definition_has_no_findings() {
        let text = "---\nname: reviewer\ndescription: Review changed Rust files for correctness and surface the riskiest diffs\ntools: Read, Grep\nmodel: sonnet\neffort: high\n---\n# Reviewer\n";
        assert!(codes(text, "reviewer").is_empty());
    }

    #[test]
    fn missing_frontmatter_short_circuits() {
        assert_eq!(codes("# just a body\n", "x"), ["agent-no-frontmatter"]);
    }

    #[test]
    fn name_checks_fire() {
        // Missing name.
        assert!(
            codes(
                "---\ndescription: a real sentence about what it does and when\n---\n",
                "x"
            )
            .contains(&"agent-name-missing")
        );
        // Invalid format.
        assert!(
            codes("---\nname: Not Valid\ndescription: a real sentence about what it does and when\n---\n", "x")
                .contains(&"agent-name-invalid")
        );
        // Name vs file mismatch.
        assert!(
            codes(
                "---\nname: alpha\ndescription: a real sentence about what it does and when\n---\n",
                "beta"
            )
            .contains(&"agent-name-mismatch")
        );
    }

    #[test]
    fn description_quality_fires() {
        assert!(codes("---\nname: x\n---\n", "x").contains(&"agent-description-missing"));
        assert!(
            codes("---\nname: x\ndescription: PDF tools\n---\n", "x")
                .contains(&"agent-description-terse")
        );
        assert!(
            codes("---\nname: x\ndescription: handles various tasks across the whole codebase as required\n---\n", "x")
                .contains(&"agent-description-vague")
        );
    }

    #[test]
    fn field_validity_fires() {
        let base = "---\nname: x\ndescription: a real sentence about what it does and when\n";
        assert!(
            codes(&format!("{base}effort: turbo\n---\n"), "x").contains(&"agent-effort-invalid")
        );
        assert!(
            codes(&format!("{base}maxTurns: zero\n---\n"), "x").contains(&"agent-maxturns-invalid")
        );
        assert!(codes(&format!("{base}model: \"\"\n---\n"), "x").contains(&"agent-model-empty"));
        assert!(codes(&format!("{base}wat: 1\n---\n"), "x").contains(&"agent-unknown-key"));
    }

    #[test]
    fn valid_optional_fields_do_not_fire() {
        let text = "---\nname: x\ndescription: a real sentence about what it does and when\nmaxTurns: 12\neffort: max\ncolor: blue\nisolation: worktree\n---\n";
        assert!(codes(text, "x").is_empty(), "{:?}", codes(text, "x"));
    }

    #[test]
    fn exposed_secret_is_a_security_error() {
        // A live-shaped AWS key (no placeholder marker like `example`/`test`, which smoke excludes).
        let text = "---\nname: x\ndescription: a real sentence about what it does and when\n---\nUse AWS key AKIA2E4RZ7KQ9X1V3B8N here\n";
        let parsed = parse(text);
        let fs = lint(&parsed, text, "x", Path::new("/tmp"), assistants::claude());
        let secret = fs.iter().find(|f| f.code == "exposed-secret");
        assert!(secret.is_some(), "a credential literal is flagged: {fs:?}");
        assert!(secret.unwrap().is_error());
    }

    #[test]
    fn diagnostic_carries_label_cause_and_fix() {
        let f = Finding::new("agent-name-missing", "no name");
        let d = diagnostic(&f, "x", Path::new("/tmp/x.md"));
        assert_eq!(d.code, "agent-name-missing");
        assert!(d.title.contains("missing agent name"));
        assert!(d.cause.unwrap().contains("no name"));
        assert!(d.help.is_some());
    }
}
