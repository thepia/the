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
use yaml_rust2::{Yaml, YamlLoader};

use super::Agent;

const NAME_MAX: usize = 64;
const DESCRIPTION_MAX: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Problem {
    pub code: &'static str,
    pub message: String,
    /// How to fix it.
    pub fix: &'static str,
}

impl Problem {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        let fix = match code {
            "no-frontmatter" => {
                "start SKILL.md with a `---` block holding `name:` and `description:`, closed by `---`"
            }
            "frontmatter-unterminated" => "close the frontmatter with a `---` line",
            "frontmatter-invalid" => {
                "fix the YAML; a plain value containing `: ` must be quoted (`description: \"...\"`) or use a `>-` block"
            }
            "name-missing" | "name-format" => {
                "set `name:` to lowercase letters, digits and single hyphens"
            }
            "name-has-slash" => "remove the `/` from `name:`",
            "name-mismatch" => "make `name:` equal the directory name, or rename the directory",
            "description-missing" => {
                "add a `description:` saying what the skill does and when to use it"
            }
            "description-not-string" => "make `description:` a plain string",
            "description-too-long" => "shorten `description` to 1024 characters or fewer",
            _ => unreachable!("every problem code has a fix"),
        };
        Self {
            code,
            message: message.into(),
            fix,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Parsed {
    /// Frontmatter missing, unterminated or not valid YAML mapping.
    pub frontmatter_error: Option<Problem>,
    pub frontmatter: Map<String, Value>,
    pub body: String,
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
            frontmatter_error: Some(Problem::new(
                "frontmatter-unterminated",
                "the YAML frontmatter has no closing `---` line",
            )),
            ..Parsed::default()
        };
    }
    let body: String = lines.collect();
    let mut out = Parsed {
        body: body.trim_start_matches(['\r', '\n']).to_owned(),
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

/// Whether `agent` loads this skill, and under which name; `dir_name` is the skill directory's
/// name as the agent reached it.
pub fn loads(agent: Agent, p: &Parsed, dir_name: &str) -> Result<String, Problem> {
    let fm_name = p.frontmatter.get("name");
    match agent {
        Agent::Claude => Ok(match (&p.frontmatter_error, p.str_field("name")) {
            (None, Some(n)) if !n.is_empty() => n.to_owned(),
            _ => dir_name.to_owned(),
        }),
        Agent::Goose | Agent::Opencode => {
            if let Some(e) = &p.frontmatter_error {
                return Err(e.clone());
            }
            let name = match fm_name {
                Some(Value::String(n)) if agent == Agent::Opencode || !n.is_empty() => n.clone(),
                Some(Value::String(_)) | None => {
                    return Err(Problem::new("name-missing", "frontmatter has no `name`"));
                }
                Some(_) => {
                    return Err(Problem::new(
                        "name-missing",
                        "frontmatter `name` is not a string",
                    ));
                }
            };
            if agent == Agent::Goose && name.contains('/') {
                return Err(Problem::new(
                    "name-has-slash",
                    format!("`name: {name}` contains `/`"),
                ));
            }
            if agent == Agent::Opencode
                && p.frontmatter
                    .get("description")
                    .is_some_and(|d| !d.is_string())
            {
                return Err(Problem::new(
                    "description-not-string",
                    "frontmatter `description` is not a string",
                ));
            }
            Ok(name)
        }
    }
}

/// Advisory Agent Skills format checks; only run on frontmatter that parsed.
pub fn lint(p: &Parsed, dir_name: &str) -> Vec<Problem> {
    let mut out = Vec::new();
    if p.frontmatter_error.is_some() {
        return out;
    }
    if let Some(n) = p.str_field("name").filter(|n| !n.is_empty()) {
        let chars_ok = n
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        if n.chars().count() > NAME_MAX
            || !chars_ok
            || n.starts_with('-')
            || n.ends_with('-')
            || n.contains("--")
        {
            out.push(Problem::new(
                "name-format",
                format!(
                    "`name: {n}` should be 1-{NAME_MAX} lowercase letters, digits and single hyphens, not starting or ending with a hyphen"
                ),
            ));
        }
        if n != dir_name {
            out.push(Problem::new(
                "name-mismatch",
                format!("`name: {n}` differs from its directory name `{dir_name}`"),
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
        Some(_) => {}
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = "---\nname: pdf-tools\ndescription: Work with PDFs.\nmetadata:\n  version: 2\n---\n\n# PDF tools\n\nDo things.\n";

    fn codes(p: &[Problem]) -> Vec<&'static str> {
        p.iter().map(|p| p.code).collect()
    }

    fn err_code(p: &Parsed) -> Option<&'static str> {
        p.frontmatter_error.as_ref().map(|e| e.code)
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
    fn agent_load_rules() {
        let ok = parse(GOOD);
        for a in [Agent::Claude, Agent::Goose, Agent::Opencode] {
            assert_eq!(loads(a, &ok, "dir").unwrap(), "pdf-tools");
        }
        let bad_yaml = parse("---\nname: a\ndescription: rule: x\n---\n");
        assert_eq!(loads(Agent::Claude, &bad_yaml, "dir").unwrap(), "dir");
        assert_eq!(
            loads(Agent::Goose, &bad_yaml, "dir").unwrap_err().code,
            "frontmatter-invalid"
        );
        assert_eq!(
            loads(Agent::Opencode, &bad_yaml, "dir").unwrap_err().code,
            "frontmatter-invalid"
        );

        let no_name = parse("---\ndescription: d\n---\n");
        assert_eq!(loads(Agent::Claude, &no_name, "dir").unwrap(), "dir");
        assert_eq!(
            loads(Agent::Goose, &no_name, "dir").unwrap_err().code,
            "name-missing"
        );
        assert_eq!(
            loads(Agent::Opencode, &no_name, "dir").unwrap_err().code,
            "name-missing"
        );

        let slash = parse("---\nname: a/b\ndescription: d\n---\n");
        assert_eq!(
            loads(Agent::Goose, &slash, "dir").unwrap_err().code,
            "name-has-slash"
        );
        assert_eq!(loads(Agent::Opencode, &slash, "dir").unwrap(), "a/b");

        let num_desc = parse("---\nname: a\ndescription: 5\n---\n");
        assert_eq!(
            loads(Agent::Opencode, &num_desc, "a").unwrap_err().code,
            "description-not-string"
        );
        assert!(loads(Agent::Goose, &num_desc, "a").is_ok());
    }

    #[test]
    fn lint_is_the_published_format() {
        let l = |s: &str, dir: &str| codes(&lint(&parse(s), dir));
        assert!(l(GOOD, "pdf-tools").is_empty());
        assert_eq!(
            l("---\nname: PDF\ndescription: x\n---\n", "PDF"),
            ["name-format"]
        );
        assert_eq!(
            l("---\nname: a--b\ndescription: x\n---\n", "a--b"),
            ["name-format"]
        );
        assert_eq!(
            l("---\nname: a\ndescription: x\n---\n", "b"),
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
}
