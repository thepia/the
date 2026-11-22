//! SKILL.md parsing, each agent's load rules, and the advisory spec lint.
//!
//! Two tiers of problems:
//! - **agent-breaking**: what makes an agent skip a skill (see [`loads`]), from each agent's source:
//!   Claude Code loads everything (invalid YAML loads with no fields, `name` defaults to the
//!   directory); Goose needs parsable frontmatter and a non-empty `name` without `/`; OpenCode needs
//!   parsable frontmatter, a string `name`, and a string `description` if present.
//! - **lint**: the published Agent Skills format (agentskills.io), which none of the three enforce:
//!   `name` 1-64 lowercase letters/digits/single hyphens equal to the directory name, `description`
//!   1-1024 characters. Advisory, so skills stay portable.

use serde::Serialize;
use serde_json::{Map, Value};
use unicode_normalization::UnicodeNormalization;
use yaml_rust2::{Yaml, YamlLoader};

use crate::Agent;

const NAME_MAX: usize = 64;
const DESCRIPTION_MAX: usize = 1024;
const COMPATIBILITY_MAX: usize = 500;
/// A present description is too terse when it is BOTH short (fewer than `DESCRIPTION_TERSE_MAX` chars)
/// and sparse (fewer than `DESCRIPTION_MIN_WORDS` meaningful words): a bare noun phrase like "PDF tools"
/// an agent cannot route on. A `note`, not an error (it still loads). Both conditions are required so a
/// minimal-but-real description ("Work with PDF files") and an oddly long low-variety one are not
/// flagged. Motivated by SkillMD SK010 and the skill-quality correlation study (description budget is
/// the top adoption signal).
const DESCRIPTION_MIN_WORDS: usize = 3;
const DESCRIPTION_TERSE_MAX: usize = 40;

/// Portable top-level keys defined by the Agent Skills (agentskills.io) specification.
const SPEC_KEYS: &[&str] = &[
    "name",
    "description",
    "license",
    "compatibility",
    "metadata",
    "allowed-tools",
];

/// Known agent-specific frontmatter extension keys. Claude Code's are its full "Frontmatter
/// reference" (code.claude.com/docs/en/skills, checked 2026-11-13) minus the portable `SPEC_KEYS`.
/// Re-check when Claude Code adds fields: a missing key here makes `unknown-frontmatter-key` fire on
/// valid skills.
const AGENT_EXTENSION_KEYS: &[&str] = &[
    // Claude Code
    "when_to_use",
    "argument-hint",
    "arguments",
    "disable-model-invocation",
    "user-invocable",
    "disallowed-tools",
    "model",
    "effort",
    "context",
    "agent",
    "background",
    "hooks",
    "paths",
    "shell",
    // Other agents
    "version",
];

/// Category of a deterministic skill problem.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProblemKind {
    /// Prevents one or more coding agents from loading the skill.
    Breaking,
    /// Security or safety hazard (`symlink-escape`, `trojan-source`, `exposed-secret`, `unrestricted-bash`).
    Security,
    /// Missing or escaping local file reference (`broken-link`, `link-escapes-skill`, `broken-symlink`).
    Asset,
    /// Deviation from the published `agentskills.io` specification.
    Spec,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Problem {
    pub code: &'static str,
    pub message: String,
    /// How to fix it.
    pub fix: &'static str,
    /// The 1-based SKILL.md line this problem points at, when it has a source location (link, asset,
    /// and security-smoke findings). Rendered as `SKILL.md:line` in a diagnostic and carried in JSON.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    /// The 1-based column on `line` where the problem begins, when precisely known (so an editor opens
    /// with the cursor there). Rendered as `SKILL.md:line:col`. Only set with `line`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub col: Option<usize>,
}

impl Problem {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        // Single source of truth: the rule registry (the-skills `rules`), not a parallel match.
        let fix = crate::rules::rule(code)
            .unwrap_or_else(|| unreachable!("every problem code is in the rule registry: {code}"))
            .fix;
        Self {
            code,
            message: message.into(),
            fix,
            line: None,
            col: None,
        }
    }

    /// Attach the 1-based SKILL.md line this problem points at (link / asset / security findings with
    /// a source location), so a diagnostic can render a clickable `SKILL.md:line`.
    pub fn at_line(mut self, line: usize) -> Self {
        self.line = Some(line);
        self
    }

    /// Attach the 1-based column where the problem begins, so an editor opens with the cursor there
    /// (`SKILL.md:line:col`). Only meaningful together with [`Self::at_line`].
    pub fn at_column(mut self, col: usize) -> Self {
        self.col = Some(col);
        self
    }

    pub fn kind(&self) -> ProblemKind {
        crate::rules::rule(self.code).map_or(ProblemKind::Spec, |r| r.kind)
    }

    /// True when this problem is a high-severity security hazard (`Diagnostic::error`).
    pub fn is_error(&self) -> bool {
        crate::rules::rule(self.code)
            .is_some_and(|r| matches!(r.severity, crate::rules::Severity::Error))
    }
}

#[derive(Debug, Clone, Default)]
pub struct Parsed {
    /// Frontmatter missing, unterminated or not valid YAML mapping.
    pub frontmatter_error: Option<Problem>,
    pub frontmatter: Map<String, Value>,
    pub body: String,
    /// 1-based line number in `SKILL.md` where `body` starts.
    pub body_start_line: usize,
    /// Count of `\n` at the very end of the file (LF). Exactly 1 is well-formed;
    /// 0 (no final newline) or >1 (trailing blank lines) drives `trailing-newline`.
    pub trailing_newlines: usize,
}

impl Parsed {
    pub fn str_field(&self, k: &str) -> Option<&str> {
        self.frontmatter.get(k).and_then(Value::as_str)
    }
}

/// Split SKILL.md into YAML frontmatter and Markdown body.
pub fn parse(text: &str) -> Parsed {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text.split_inclusive('\n');
    if !lines.next().is_some_and(|l| l.trim_end() == "---") {
        return Parsed {
            body: text.to_owned(),
            body_start_line: 1,
            frontmatter_error: Some(Problem::new(
                "no-frontmatter",
                "SKILL.md does not start with a `---` YAML frontmatter block",
            )),
            ..Parsed::default()
        };
    }

    let mut yaml = String::new();
    let mut closed = false;
    for line in lines.by_ref() {
        if line.trim_end() == "---" {
            closed = true;
            break;
        }
        yaml.push_str(line);
    }
    if !closed {
        return Parsed {
            body_start_line: 1,
            frontmatter_error: Some(Problem::new(
                "frontmatter-unterminated",
                "the YAML frontmatter has no closing `---` line",
            )),
            ..Parsed::default()
        };
    }
    let raw_body: String = lines.collect();
    let trimmed_body = raw_body.trim_start_matches(['\r', '\n']);
    let prefix_len = raw_body.len() - trimmed_body.len();
    let stripped_newlines = raw_body[..prefix_len]
        .bytes()
        .filter(|&b| b == b'\n')
        .count();
    let yaml_lines = yaml.bytes().filter(|&b| b == b'\n').count();
    let mut out = Parsed {
        body: trimmed_body.to_owned(),
        body_start_line: 3 + yaml_lines + stripped_newlines,
        trailing_newlines: text.len() - text.trim_end_matches('\n').len(),
        ..Parsed::default()
    };
    let invalid = |m: String| Some(Problem::new("frontmatter-invalid", m));
    match YamlLoader::load_from_str(&yaml) {
        // Marker lines are 1-based within the frontmatter; +1 for the opening `---` gives the
        // SKILL.md line. Columns are 0-based.
        Err(e) => {
            out.frontmatter_error = invalid(format!(
                "invalid YAML at SKILL.md line {} column {}: {}",
                e.marker().line() + 1,
                e.marker().col() + 1,
                e.info()
            ))
        }
        Ok(docs) => match docs.into_iter().next() {
            None | Some(Yaml::Null) => {}
            Some(Yaml::Hash(h)) => {
                for (k, v) in h {
                    match k.as_str() {
                        Some(k) => {
                            out.frontmatter.insert(k.to_owned(), to_json(v));
                        }
                        None => {
                            out.frontmatter_error =
                                invalid("frontmatter has a non-string key".into())
                        }
                    }
                }
            }
            Some(_) => {
                out.frontmatter_error =
                    invalid("frontmatter is not a YAML mapping of `key: value` pairs".into())
            }
        },
    }
    out
}

fn to_json(y: Yaml) -> Value {
    match y {
        Yaml::String(s) => Value::String(s),
        Yaml::Integer(i) => Value::from(i),
        Yaml::Real(r) => r
            .parse::<f64>()
            .map(Value::from)
            .unwrap_or(Value::String(r)),
        Yaml::Boolean(b) => Value::Bool(b),
        Yaml::Array(a) => Value::Array(a.into_iter().map(to_json).collect()),
        Yaml::Hash(h) => Value::Object(
            h.into_iter()
                .map(|(k, v)| {
                    let key = match k {
                        Yaml::String(s) => s,
                        other => format!("{other:?}"),
                    };
                    (key, to_json(v))
                })
                .collect(),
        ),
        _ => Value::Null,
    }
}

/// The outcome of an agent's load decision for a skill.
#[derive(Debug, Clone)]
pub enum Load {
    /// The agent loads the skill, under this name.
    Loaded(String),
    /// The agent skips the skill and `the` is certain of it (a strict parser or
    /// a verified rule). The `Problem` is the actionable cause.
    Skipped(Problem),
    /// `the` cannot predict this agent's verdict. Gemini CLI has a lenient
    /// fallback parser `the` does not model, so when a frontmatter block is
    /// PRESENT but not strict YAML the real Gemini may still load the skill.
    /// The `Problem` (the malformed frontmatter) is still reported so it gets
    /// fixed; `the` only declines to assert load-or-skip. An ABSENT block is a
    /// certain skip, not Unknown.
    Unknown(Problem),
}

/// Whether `agent` loads this skill, and under which name; `dir_name` is the skill directory's
/// name as the agent reached it.
pub fn loads(agent: Agent, p: &Parsed, dir_name: &str) -> Load {
    let fm_name = p.frontmatter.get("name");
    match agent {
        // Claude Code loads every skill directory and identifies it by the directory name: two
        // directories sharing a frontmatter `name` both load (verified on 2.1.258). The frontmatter
        // `name` only sets the `/` command, unless another skill already took it ([`claude_alias`]).
        Agent::Claude => Load::Loaded(dir_name.to_owned()),
        Agent::Gemini | Agent::Goose | Agent::Opencode => {
            if let Some(e) = &p.frontmatter_error {
                // Gemini: an ABSENT block (`no-frontmatter`) is a certain skip, but a block that is
                // present yet not strict YAML (`frontmatter-invalid`, `frontmatter-unterminated`)
                // may be recovered by Gemini's lenient fallback parser, which `the` does not model,
                // so the verdict is Unknown (the malformed frontmatter is still reported for a fix).
                // Goose and OpenCode use strict parsers, so any frontmatter error is a real skip.
                if agent == Agent::Gemini && e.code != "no-frontmatter" {
                    return Load::Unknown(e.clone());
                }
                return Load::Skipped(e.clone());
            }
            let name = match fm_name {
                Some(Value::String(n)) if agent == Agent::Opencode || !n.is_empty() => n.clone(),
                Some(Value::String(_)) | None => {
                    return Load::Skipped(Problem::new(
                        "name-missing",
                        "frontmatter has no `name`",
                    ));
                }
                Some(_) => {
                    return Load::Skipped(Problem::new(
                        "name-missing",
                        "frontmatter `name` is not a string",
                    ));
                }
            };
            if agent == Agent::Goose && name.contains('/') {
                return Load::Skipped(Problem::new(
                    "name-has-slash",
                    format!("`name: {name}` contains `/`"),
                ));
            }
            if agent == Agent::Gemini
                && p.str_field("description")
                    .map(str::trim)
                    .unwrap_or("")
                    .is_empty()
            {
                return Load::Skipped(Problem::new(
                    "description-missing",
                    "frontmatter has no `description`",
                ));
            }
            if agent == Agent::Opencode
                && p.frontmatter
                    .get("description")
                    .is_some_and(|d| !d.is_string())
            {
                return Load::Skipped(Problem::new(
                    "description-not-string",
                    "frontmatter `description` is not a string",
                ));
            }
            Load::Loaded(name)
        }
    }
}

/// The name `the` shows for a skill: its frontmatter `name` when that parsed and is non-empty,
/// else the directory name.
pub fn display_name(p: &Parsed, dir_name: &str) -> String {
    match (&p.frontmatter_error, p.str_field("name")) {
        (None, Some(n)) if !n.is_empty() => n.to_owned(),
        _ => dir_name.to_owned(),
    }
}

/// The individual tool grants in an `allowed-tools` value. Claude Code accepts a YAML list or a
/// space- or comma-separated string; a string is split only outside parentheses, so
/// `"Bash(git status:*) Read"` is two grants (`Bash(git status:*)`, `Read`), not three. Non-string
/// list items are ignored (`allowed-tools-invalid` reports them).
pub fn allowed_tools(val: &Value) -> Vec<String> {
    match val {
        Value::String(s) => {
            let mut out = Vec::new();
            let mut cur = String::new();
            let mut depth = 0usize;
            for c in s.chars() {
                match c {
                    '(' => {
                        depth += 1;
                        cur.push(c);
                    }
                    ')' => {
                        depth = depth.saturating_sub(1);
                        cur.push(c);
                    }
                    c if depth == 0 && (c.is_whitespace() || c == ',') => {
                        if !cur.is_empty() {
                            out.push(std::mem::take(&mut cur));
                        }
                    }
                    c => cur.push(c),
                }
            }
            if !cur.is_empty() {
                out.push(cur);
            }
            out
        }
        Value::Array(items) => items
            .iter()
            .filter_map(Value::as_str)
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

/// The `/` command a skill's frontmatter `name` claims in Claude Code, if it declares one. Claude Code
/// gives the command to only one skill when several claim it; the directory name always invokes too.
pub fn claude_alias(p: &Parsed) -> Option<&str> {
    match (&p.frontmatter_error, p.str_field("name")) {
        (None, Some(n)) if !n.is_empty() => Some(n),
        _ => None,
    }
}

/// Whether `name` satisfies the Agent Skills name format: 1-64 characters, only lowercase ASCII
/// letters, digits and single hyphens, not starting or ending with a hyphen. Shared by the lint
/// (`name-format`) and the fix advisor (to tell a mechanically-lowercasable name from one that needs
/// a human-chosen replacement).
pub fn name_format_ok(name: &str) -> bool {
    let chars_ok = name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    !name.is_empty()
        && name.chars().count() <= NAME_MAX
        && chars_ok
        && !name.starts_with('-')
        && !name.ends_with('-')
        && !name.contains("--")
}

/// Count of meaningful words in a description: whitespace tokens that carry a letter or digit and are
/// at least two characters. Drives the `description-too-terse` discovery-quality lint.
fn description_word_count(desc: &str) -> usize {
    desc.split_whitespace()
        .filter(|w| w.chars().count() >= 2 && w.chars().any(char::is_alphanumeric))
        .count()
}

/// Meaningful tokens of a string: lowercase alphanumeric runs of at least two characters, splitting on
/// anything else (so `pdf-tools` -> {pdf, tools}). Drives `description-echoes-name`.
fn meaningful_tokens(s: &str) -> std::collections::BTreeSet<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 2)
        .map(str::to_lowercase)
        .collect()
}

/// The first placeholder / template marker found in a skill body, if any: an unambiguous sign the
/// skill was left unfinished. Deliberately conservative (no bare `TODO`/`FIXME`, which legitimately
/// appear in skills about task tracking, and no `{{` which appears in templating skills).
fn unfinished_marker(body: &str) -> Option<&'static str> {
    const MARKERS: &[&str] = &[
        "lorem ipsum",
        "<placeholder",
        "replace_me",
        "replace_this",
        "your-name-here",
        "your name here",
    ];
    let lower = body.to_lowercase();
    MARKERS.iter().copied().find(|m| lower.contains(m))
}

/// Advisory Agent Skills format checks; only run on frontmatter that parsed.
pub fn lint(p: &Parsed, dir_name: &str) -> Vec<Problem> {
    let mut out = Vec::new();
    if p.frontmatter_error.is_some() {
        return out;
    }
    if let Some(n) = p.str_field("name").filter(|n| !n.is_empty()) {
        if !name_format_ok(n) {
            out.push(Problem::new(
                "name-format",
                format!(
                    "`name: {n}` should be 1-{NAME_MAX} lowercase letters, digits and single hyphens, not starting or ending with a hyphen"
                ),
            ));
        }
        // Compare under Unicode NFKC so a composed name and a canonically/compatibility-equivalent
        // directory name (e.g. `café` vs `cafe` + combining acute) are not a spurious mismatch, the
        // same normalization the reference validator (skills-ref) applies.
        if n.nfkc().ne(dir_name.nfkc()) {
            out.push(Problem::new(
                "name-mismatch",
                format!(
                    "`name: {n}` differs from its directory name `{dir_name}` (Claude Code lists the skill as `{dir_name}`; `/{n}` invokes it only if no other skill claims that command)"
                ),
            ));
        }
    }
    match p.str_field("description").map(str::trim) {
        None | Some("") => out.push(Problem::new(
            "description-missing",
            "frontmatter has no `description`",
        )),
        Some(d) if d.chars().count() > DESCRIPTION_MAX => out.push(Problem::new(
            "description-too-long",
            format!(
                "`description` is {} characters; the format allows {DESCRIPTION_MAX}",
                d.chars().count()
            ),
        )),
        Some(d)
            if d.chars().count() < DESCRIPTION_TERSE_MAX
                && description_word_count(d) < DESCRIPTION_MIN_WORDS =>
        {
            out.push(Problem::new(
                "description-too-terse",
                format!(
                    "`description` has {} meaningful words; too terse for an agent to route on, say what the skill does and when to use it",
                    description_word_count(d)
                ),
            ));
        }
        Some(_) => {}
    }
    if let Some(v) = p.frontmatter.get("license")
        && !v.is_string()
    {
        out.push(Problem::new(
            "license-not-string",
            "frontmatter `license` must be a string",
        ));
    }
    if let Some(v) = p.frontmatter.get("compatibility") {
        match v.as_str() {
            Some(s) if !s.trim().is_empty() && s.chars().count() <= COMPATIBILITY_MAX => {}
            Some(s) if s.trim().is_empty() => out.push(Problem::new(
                "compatibility-invalid",
                "frontmatter `compatibility` must not be empty",
            )),
            Some(s) => out.push(Problem::new(
                "compatibility-invalid",
                format!(
                    "`compatibility` is {} characters; the format allows {COMPATIBILITY_MAX}",
                    s.chars().count()
                ),
            )),
            None => out.push(Problem::new(
                "compatibility-invalid",
                "frontmatter `compatibility` must be a string",
            )),
        }
    }
    if let Some(v) = p.frontmatter.get("metadata") {
        let valid_map = match v.as_object() {
            Some(map) => map.values().all(|val| !val.is_object() && !val.is_array()),
            None => false,
        };
        if !valid_map {
            out.push(Problem::new(
                "metadata-not-map",
                "frontmatter `metadata` must be a mapping of string keys to scalar values",
            ));
        }
    }
    if let Some(v) = p.frontmatter.get("allowed-tools") {
        let valid = match v {
            Value::String(_) => true,
            Value::Array(items) => items.iter().all(Value::is_string),
            _ => false,
        };
        if !valid {
            out.push(Problem::new(
                "allowed-tools-invalid",
                "frontmatter `allowed-tools` must be a space-delimited string or list of strings",
            ));
        }
    }
    let mut unknown: Vec<&str> = p
        .frontmatter
        .keys()
        .map(String::as_str)
        .filter(|k| !SPEC_KEYS.contains(k) && !AGENT_EXTENSION_KEYS.contains(k))
        .collect();
    unknown.sort_unstable();
    for k in unknown {
        out.push(Problem::new(
            "unknown-frontmatter-key",
            format!("unknown frontmatter key `{k}`"),
        ));
    }
    // A description whose meaningful words are all already in the name adds no routing information.
    if let (Some(name), Some(desc)) = (
        p.str_field("name").filter(|n| !n.is_empty()),
        p.str_field("description")
            .map(str::trim)
            .filter(|d| !d.is_empty()),
    ) {
        let desc_tokens = meaningful_tokens(desc);
        if !desc_tokens.is_empty() && desc_tokens.is_subset(&meaningful_tokens(name)) {
            out.push(Problem::new(
                "description-echoes-name",
                "`description` only repeats the skill name; it adds no routing information",
            ));
        }
    }
    // Placeholder / template markers left in the body: the skill looks unfinished.
    if let Some(marker) = unfinished_marker(&p.body) {
        out.push(Problem::new(
            "unfinished-skill",
            format!("body contains the placeholder `{marker}`; the skill looks unfinished"),
        ));
    }
    // Exactly one trailing newline is well-formed (Beta: staged behind `--beta`).
    if p.trailing_newlines != 1 {
        let detail = if p.trailing_newlines == 0 {
            "SKILL.md does not end with a newline".to_owned()
        } else {
            format!(
                "SKILL.md ends with {} trailing newlines",
                p.trailing_newlines
            )
        };
        out.push(Problem::new("trailing-newline", detail));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = "---\nname: pdf-tools\ndescription: Work with PDFs.\nmetadata:\n  version: 2\n---\n\n# PDF tools\n\nDo things.\n";

    impl Load {
        fn name(&self) -> &str {
            match self {
                Load::Loaded(n) => n,
                other => panic!("expected Loaded, got {other:?}"),
            }
        }
        fn skip_code(&self) -> &str {
            match self {
                Load::Skipped(p) => p.code,
                other => panic!("expected Skipped, got {other:?}"),
            }
        }
        fn unknown_code(&self) -> &str {
            match self {
                Load::Unknown(p) => p.code,
                other => panic!("expected Unknown, got {other:?}"),
            }
        }
        fn is_loaded(&self) -> bool {
            matches!(self, Load::Loaded(_))
        }
    }

    fn codes(p: &[Problem]) -> Vec<&'static str> {
        p.iter().map(|p| p.code).collect()
    }

    fn err_code(p: &Parsed) -> Option<&'static str> {
        p.frontmatter_error.as_ref().map(|e| e.code)
    }

    #[test]
    fn trailing_newline_flags_missing_and_extra_only() {
        let base = "---\nname: a\ndescription: a clear useful tool\n---\n\nBody.";
        // Exactly one trailing newline: well-formed, no flag.
        assert!(
            !codes(&lint(&parse(&format!("{base}\n")), "a")).contains(&"trailing-newline"),
            "one trailing newline must be accepted"
        );
        // No final newline: flagged.
        assert!(
            codes(&lint(&parse(base), "a")).contains(&"trailing-newline"),
            "a missing final newline must be flagged"
        );
        // Two trailing newlines: flagged.
        assert!(
            codes(&lint(&parse(&format!("{base}\n\n")), "a")).contains(&"trailing-newline"),
            "extra trailing newlines must be flagged"
        );
        // A frontmatter-only skill ending in one newline is well-formed (counted from the
        // file, not the empty body).
        assert!(
            !codes(&lint(
                &parse("---\nname: a\ndescription: a clear useful tool\n---\n"),
                "a"
            ))
            .contains(&"trailing-newline"),
            "a body-less skill with one trailing newline must be accepted"
        );
    }

    #[test]
    fn parses_frontmatter_and_body() {
        let p = parse(GOOD);
        assert!(p.frontmatter_error.is_none());
        assert_eq!(p.frontmatter["name"], "pdf-tools");
        assert_eq!(p.frontmatter["metadata"]["version"], 2);
        assert_eq!(p.body, "# PDF tools\n\nDo things.\n");
    }

    #[test]
    fn crlf_and_bom_are_accepted() {
        let p = parse("\u{feff}---\r\nname: a\r\ndescription: b\r\n---\r\nbody\r\n");
        assert_eq!(err_code(&p), None);
        assert_eq!(p.frontmatter["description"], "b");
        assert_eq!(p.body, "body\r\n");
    }

    #[test]
    fn broken_frontmatter_is_classified_with_file_line_numbers() {
        assert_eq!(
            err_code(&parse("# just markdown\n")),
            Some("no-frontmatter")
        );
        assert_eq!(
            err_code(&parse("---\nname: a\n")),
            Some("frontmatter-unterminated")
        );
        assert_eq!(
            err_code(&parse("---\n- a\n---\n")),
            Some("frontmatter-invalid")
        );
        let p = parse("---\nname: a\ndescription: rule: x\n---\n");
        assert_eq!(err_code(&p), Some("frontmatter-invalid"));
        assert!(
            p.frontmatter_error
                .unwrap()
                .message
                .contains("SKILL.md line 3")
        );
    }

    #[test]
    fn allowed_tools_splits_outside_parentheses_only() {
        let v = |s: &str| allowed_tools(&Value::String(s.into()));
        assert_eq!(v("Bash(git status:*) Read"), ["Bash(git status:*)", "Read"]);
        assert_eq!(
            v("Read, Grep,Bash(npm run *)"),
            ["Read", "Grep", "Bash(npm run *)"]
        );
        assert_eq!(v("  "), Vec::<String>::new());
        let list = serde_json::json!(["Read", " Bash(git log) ", 5]);
        assert_eq!(allowed_tools(&list), ["Read", "Bash(git log)"]);
    }

    #[test]
    fn agent_load_rules() {
        let ok = parse(GOOD);
        for a in [Agent::Goose, Agent::Opencode] {
            assert_eq!(loads(a, &ok, "dir").name(), "pdf-tools");
        }
        // Claude Code identifies a skill by its directory; `name` is the `/` command it claims.
        assert_eq!(loads(Agent::Claude, &ok, "dir").name(), "dir");
        assert_eq!(claude_alias(&ok), Some("pdf-tools"));
        assert_eq!(display_name(&ok, "dir"), "pdf-tools");
        let bad_yaml = parse("---\nname: a\ndescription: rule: x\n---\n");
        assert_eq!(loads(Agent::Claude, &bad_yaml, "dir").name(), "dir");
        assert_eq!(
            loads(Agent::Goose, &bad_yaml, "dir").skip_code(),
            "frontmatter-invalid"
        );
        assert_eq!(
            loads(Agent::Opencode, &bad_yaml, "dir").skip_code(),
            "frontmatter-invalid"
        );

        let no_name = parse("---\ndescription: d\n---\n");
        assert_eq!(loads(Agent::Claude, &no_name, "dir").name(), "dir");
        assert_eq!(
            loads(Agent::Goose, &no_name, "dir").skip_code(),
            "name-missing"
        );
        assert_eq!(
            loads(Agent::Opencode, &no_name, "dir").skip_code(),
            "name-missing"
        );

        let slash = parse("---\nname: a/b\ndescription: d\n---\n");
        assert_eq!(
            loads(Agent::Goose, &slash, "dir").skip_code(),
            "name-has-slash"
        );
        assert_eq!(loads(Agent::Opencode, &slash, "dir").name(), "a/b");

        let num_desc = parse("---\nname: a\ndescription: 5\n---\n");
        assert_eq!(
            loads(Agent::Opencode, &num_desc, "a").skip_code(),
            "description-not-string"
        );
        assert!(loads(Agent::Goose, &num_desc, "a").is_loaded());

        // Gemini CLI's load rules (ST-02): a valid skill loads under its `name`;
        // a missing `name` or a blank `description` is a certain skip. Previously
        // untested, so the audit's Gemini defects had no guard.
        assert_eq!(loads(Agent::Gemini, &ok, "dir").name(), "pdf-tools");
        assert_eq!(
            loads(Agent::Gemini, &no_name, "dir").skip_code(),
            "name-missing"
        );
        let blank_desc = parse("---\nname: a\ndescription: \"\"\n---\n");
        assert_eq!(
            loads(Agent::Gemini, &blank_desc, "a").skip_code(),
            "description-missing"
        );
    }

    // A1 (spec audit): the real Gemini CLI loads a skill whose frontmatter block
    // is present but not strict YAML (it falls back to a lenient line parser);
    // `the` does not model that fallback. Rather than wrongly assert a skip (or
    // build the fallback parser), `the` returns Unknown for Gemini on a
    // broken-but-present block, while still reporting the malformed frontmatter
    // so it gets fixed. An ABSENT block is a certain skip. Goose/OpenCode use
    // strict parsers, so for them a broken block is a real skip (covered above).
    #[test]
    fn gemini_broken_frontmatter_is_unknown_absent_is_skip() {
        let invalid = parse("---\nname: a\ndescription: rule: x\n---\n");
        assert_eq!(
            loads(Agent::Gemini, &invalid, "dir").unknown_code(),
            "frontmatter-invalid"
        );
        let unterminated = parse("---\nname: a\n");
        assert_eq!(
            loads(Agent::Gemini, &unterminated, "dir").unknown_code(),
            "frontmatter-unterminated"
        );
        let absent = parse("no frontmatter here\n");
        assert_eq!(
            loads(Agent::Gemini, &absent, "dir").skip_code(),
            "no-frontmatter"
        );
    }

    #[test]
    fn lint_is_the_published_format() {
        let l = |s: &str, dir: &str| codes(&lint(&parse(s), dir));
        assert!(l(GOOD, "pdf-tools").is_empty());
        assert_eq!(
            l(
                "---\nname: PDF\ndescription: a clear useful tool\n---\n",
                "PDF"
            ),
            ["name-format"]
        );
        assert_eq!(
            l(
                "---\nname: a--b\ndescription: a clear useful tool\n---\n",
                "a--b"
            ),
            ["name-format"]
        );
        assert_eq!(
            l("---\nname: a\ndescription: a clear useful tool\n---\n", "b"),
            ["name-mismatch"]
        );
        assert_eq!(l("---\nname: a\n---\n", "a"), ["description-missing"]);
        let long = format!(
            "---\nname: a\ndescription: {}\n---\n",
            "x".repeat(DESCRIPTION_MAX + 1)
        );
        assert_eq!(l(&long, "a"), ["description-too-long"]);
        assert!(
            l("no frontmatter", "a").is_empty(),
            "breaking problems are not lint"
        );
    }

    #[test]
    fn extended_frontmatter_lint_checks() {
        let l = |s: &str, dir: &str| codes(&lint(&parse(s), dir));

        // Known agent extension keys pass cleanly.
        let extensions = "\
---
name: a
description: a clear useful tool
license: MIT
compatibility: Requires git and python3
allowed-tools: Read Grep Glob
argument-hint: \"[file]\"
disable-model-invocation: true
user-invocable: true
model: sonnet
context: fork
agent: general-purpose
hooks: {}
version: 1.0.0
when_to_use: When testing
metadata:
  author: thepia
  tier: 1
---
";
        assert!(l(extensions, "a").is_empty());

        // Array of strings in allowed-tools is also accepted.
        assert!(
            l(
                "---\nname: a\ndescription: a clear useful tool\nallowed-tools:\n  - Read\n  - Write\n---\n",
                "a"
            )
            .is_empty()
        );

        // Invalid compatibility: empty, non-string, or > 500 chars.
        assert_eq!(
            l(
                "---\nname: a\ndescription: a clear useful tool\ncompatibility: \"   \"\n---\n",
                "a"
            ),
            ["compatibility-invalid"]
        );
        assert_eq!(
            l(
                "---\nname: a\ndescription: a clear useful tool\ncompatibility: 123\n---\n",
                "a"
            ),
            ["compatibility-invalid"]
        );
        let long_compat = format!(
            "---\nname: a\ndescription: a clear useful tool\ncompatibility: {}\n---\n",
            "c".repeat(COMPATIBILITY_MAX + 1)
        );
        assert_eq!(l(&long_compat, "a"), ["compatibility-invalid"]);

        // Invalid license: non-string.
        assert_eq!(
            l(
                "---\nname: a\ndescription: a clear useful tool\nlicense: 42\n---\n",
                "a"
            ),
            ["license-not-string"]
        );

        // Invalid metadata: non-mapping or nested array/object values.
        assert_eq!(
            l(
                "---\nname: a\ndescription: a clear useful tool\nmetadata: plain\n---\n",
                "a"
            ),
            ["metadata-not-map"]
        );
        assert_eq!(
            l(
                "---\nname: a\ndescription: a clear useful tool\nmetadata:\n  nested:\n    k: v\n---\n",
                "a"
            ),
            ["metadata-not-map"]
        );
        assert_eq!(
            l(
                "---\nname: a\ndescription: a clear useful tool\nmetadata:\n  list:\n    - item\n---\n",
                "a"
            ),
            ["metadata-not-map"]
        );

        // Invalid allowed-tools: non-string / non-string-array.
        assert_eq!(
            l(
                "---\nname: a\ndescription: a clear useful tool\nallowed-tools: 99\n---\n",
                "a"
            ),
            ["allowed-tools-invalid"]
        );
        assert_eq!(
            l(
                "---\nname: a\ndescription: a clear useful tool\nallowed-tools:\n  - 99\n---\n",
                "a"
            ),
            ["allowed-tools-invalid"]
        );

        // Unknown top-level frontmatter key.
        assert_eq!(
            l(
                "---\nname: a\ndescription: a clear useful tool\ncustom-key: val\n---\n",
                "a"
            ),
            ["unknown-frontmatter-key"]
        );
    }

    // --- Benchmark-ported cases (see cli/docs/scratch/skills-lint-test-plan.md, Appendix A) ---

    #[test]
    fn name_format_accepts_what_the_normative_validator_accepts() {
        // skills-ref accepts 1-char, digit-leading, all-digit, and exactly-64-char names; swarmclaw
        // wrongly rejects digit-leading (`3d-tools`). `the` must stay silent on all of these.
        let l = |s: &str, dir: &str| codes(&lint(&parse(s), dir));
        for name in ["a", "3d-tools", "1", "a1", &"a".repeat(NAME_MAX)] {
            assert!(
                l(
                    &format!("---\nname: {name}\ndescription: a clear useful tool\n---\n"),
                    name
                )
                .is_empty(),
                "name `{name}` must be accepted"
            );
        }
        // And fires on the real violations.
        for name in ["-a", "a-", "a b", "A", &"a".repeat(NAME_MAX + 1)] {
            assert_eq!(
                l(
                    &format!("---\nname: {name}\ndescription: a clear useful tool\n---\n"),
                    name
                ),
                ["name-format"],
                "name `{name}` must be rejected"
            );
        }
    }

    #[test]
    fn description_length_boundary_is_chars_not_bytes() {
        let l = |s: &str, dir: &str| codes(&lint(&parse(s), dir));
        // Exactly 1024 is clean; 1025 fires.
        let at = format!(
            "---\nname: a\ndescription: {}\n---\n",
            "x".repeat(DESCRIPTION_MAX)
        );
        assert!(l(&at, "a").is_empty());
        let over = format!(
            "---\nname: a\ndescription: {}\n---\n",
            "x".repeat(DESCRIPTION_MAX + 1)
        );
        assert_eq!(l(&over, "a"), ["description-too-long"]);
        // Counted in chars: 1024 multibyte chars stay clean (would be >1024 bytes).
        let multibyte = format!(
            "---\nname: a\ndescription: {}\n---\n",
            "é".repeat(DESCRIPTION_MAX)
        );
        assert!(
            l(&multibyte, "a").is_empty(),
            "length must be counted in chars"
        );
    }

    #[test]
    fn terse_description_is_flagged_but_minimal_real_ones_pass() {
        let l = |s: &str, dir: &str| codes(&lint(&parse(s), dir));
        // Bare noun phrase: short + sparse -> flagged.
        assert_eq!(
            l("---\nname: a\ndescription: PDF tools\n---\n", "a"),
            ["description-too-terse"]
        );
        assert_eq!(
            l("---\nname: a\ndescription: database\n---\n", "a"),
            ["description-too-terse"]
        );
        // A minimal-but-real description (>=3 meaningful words) is clean.
        assert!(l("---\nname: a\ndescription: Work with PDF files\n---\n", "a").is_empty());
        // Short but 3 words (the canonical GOOD description) is clean.
        assert!(l("---\nname: a\ndescription: Work with PDFs.\n---\n", "a").is_empty());
        // Long low-variety descriptions are not "terse" (the char guard): clean here.
        let long_one_word = format!("---\nname: a\ndescription: {}\n---\n", "x".repeat(50));
        assert!(l(&long_one_word, "a").is_empty());
    }

    #[test]
    fn description_echoing_the_name_is_flagged() {
        let l = |s: &str, dir: &str| codes(&lint(&parse(s), dir));
        // Description only restates the name (3 words, so not also terse) -> flagged.
        assert_eq!(
            l(
                "---\nname: pdf-document-tools\ndescription: PDF document tools\n---\n",
                "pdf-document-tools"
            ),
            ["description-echoes-name"]
        );
        // Adds information beyond the name -> clean.
        assert!(
            l(
                "---\nname: pdf-tools\ndescription: Fill and merge PDF forms\n---\n",
                "pdf-tools"
            )
            .is_empty()
        );
    }

    #[test]
    fn placeholder_body_markers_flag_an_unfinished_skill() {
        let terse = codes(&lint(
            &parse(
                "---\nname: a\ndescription: a clear useful tool\n---\n\nTODO: lorem ipsum dolor.\n",
            ),
            "a",
        ));
        assert_eq!(terse, ["unfinished-skill"]);
        // A skill whose body merely mentions tasks is not flagged (no template marker).
        assert!(
            codes(&lint(
                &parse("---\nname: a\ndescription: a clear useful tool\n---\n\nManage your TODO list.\n"),
                "a",
            ))
            .is_empty()
        );
    }

    #[test]
    fn whitespace_only_description_is_missing() {
        // skill-check `description.non_empty`: whitespace-only counts as empty.
        let l = |s: &str, dir: &str| codes(&lint(&parse(s), dir));
        assert_eq!(
            l("---\nname: a\ndescription: \"   \"\n---\n", "a"),
            ["description-missing"]
        );
    }

    #[test]
    fn a_comment_mentioning_description_above_name_is_not_a_false_positive() {
        // skill-check's `field_order` rule string-matches raw YAML and false-positives when a comment
        // containing `description:` precedes `name:`. `the` parses the AST, so this must be clean.
        let p = parse(
            "---\n# note: description: comes later\nname: a\ndescription: a real useful thing\n---\n",
        );
        assert_eq!(err_code(&p), None);
        assert!(lint(&p, "a").is_empty());
    }

    #[test]
    fn empty_frontmatter_has_no_parse_error_and_no_fields() {
        let p = parse("---\n---\n\nbody\n");
        assert_eq!(err_code(&p), None);
        assert!(p.frontmatter.is_empty());
        // Only the advisory description-missing applies (no name checks without a name).
        assert_eq!(codes(&lint(&p, "a")), ["description-missing"]);
    }

    #[test]
    fn top_level_scalar_and_non_string_key_are_invalid() {
        assert_eq!(
            err_code(&parse("---\nhello\n---\n")),
            Some("frontmatter-invalid")
        );
        assert_eq!(
            err_code(&parse("---\n? [a, b]: v\n---\n")),
            Some("frontmatter-invalid")
        );
    }

    #[test]
    fn name_matches_directory_under_nfkc() {
        // `name: café` with a composed é (U+00E9) vs a directory using a decomposed é (U+0065 U+0301).
        // Under NFKC they are equal, so no name-mismatch should be reported.
        let p = parse("---\nname: caf\u{e9}\ndescription: a clear useful tool\n---\n");
        let dir = "cafe\u{301}";
        assert!(
            lint(&p, dir).iter().all(|pr| pr.code != "name-mismatch"),
            "NFKC-equal name and directory must not be a mismatch"
        );
    }
}
