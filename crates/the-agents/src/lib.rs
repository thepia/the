//! Core agent-definition logic shared by the Thepia command lines (`the agents`, `thepia agents`).
//!
//! An **agent definition** is a single Markdown file with YAML frontmatter that declares a subagent a
//! coding assistant can delegate to: its `name`, its `description` (which drives *delegation* the way a
//! skill's description drives *routing*), and optional `tools` / `model` / `effort` fields. Claude Code
//! reads them from `.claude/agents/*.md` (project and user scope); other runtimes use equivalent
//! locations. This crate finds those files, reports them, and lints them for loadability, delegation
//! quality, field validity, and security hazards.
//!
//! This crate is the **self-contained `agents` command group**: besides the logic modules it owns its
//! clap grammar ([`cli`]) and its command adapter ([`cmd`], which implements
//! [`the_output::command::Target`] and renders through `the-output`). The binary just registers
//! [`Agents`]. A command group is a crate.
//!
//! It reuses the generic lint core from `the-skills` (the frontmatter parser, the security-smoke
//! detection, the environment probe) and owns the agent-specific rule registry ([`rules`]), per
//! WORK-0036's "share the registry/rule machinery, specialize the key sets". The adjacent
//! instruction-file surfaces named in WORK-0036 (`AGENTS.md`, `CLAUDE.md`) are **not** covered here:
//! their conformance checks depend on normative standards that are not yet pinned, so adding them is a
//! later increment, not a guess.
//!
//! - [`discover`]: where each runtime keeps agent definitions, and reading them into [`AgentDef`]s.
//! - [`rules`]: the single agent lint-rule registry (one row per rule, every attribute keyed off the code).
//! - [`lint`]: field validity, delegation quality, and reused security smoke for one definition.
//! - [`cli`] / [`cmd`]: the command's arguments, execution, and rendering.

pub mod assistants;
pub mod cli;
pub mod cmd;
pub mod discover;
pub mod keys;
pub mod lint;
pub mod rules;

pub use assistants::Assistant;
pub use cmd::Agents;

use std::path::PathBuf;

use the_skills::parse::Parsed;

// Scope (`project` / `user` / `dir`) is a general location vocabulary; reuse it rather than minting a
// parallel enum. `System` is unused for agent definitions (no machine-wide managed agents today).
pub use the_skills::Scope;

/// One discovered agent definition: a single `*.md` file, parsed, with its computed lint findings.
#[derive(Debug)]
pub struct AgentDef {
    /// The addressable name: the frontmatter `name`, falling back to the file stem when absent.
    pub name: String,
    /// The file name without its `.md` extension (the assistant lists the agent by this).
    pub file_stem: String,
    /// The definition file.
    pub path: PathBuf,
    pub scope: Scope,
    /// The assistant whose directory this definition was found in (and whose field rules apply).
    pub assistant: &'static Assistant,
    /// The parsed frontmatter and body.
    pub parsed: Parsed,
    /// The lint findings for this definition (computed once at discovery).
    pub findings: Vec<lint::Finding>,
}

impl AgentDef {
    /// The `description` frontmatter value, if present and a string.
    pub fn description(&self) -> Option<&str> {
        self.parsed.str_field("description")
    }

    /// The `model` frontmatter value, if present and a string.
    pub fn model(&self) -> Option<&str> {
        self.parsed.str_field("model")
    }

    /// The declared tools as a flat list: a comma/space-delimited string (`Read, Grep`) or a YAML list
    /// both flatten to the same vector. Empty when the field is absent (the runtime grants all tools).
    pub fn tools(&self) -> Vec<String> {
        match self.parsed.frontmatter.get("tools") {
            Some(serde_json::Value::String(s)) => s
                .split([',', ' '])
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(ToOwned::to_owned)
                .collect(),
            Some(serde_json::Value::Array(a)) => a
                .iter()
                .filter_map(|v| v.as_str())
                .map(ToOwned::to_owned)
                .collect(),
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use the_skills::parse::parse;

    fn def(frontmatter_and_body: &str) -> AgentDef {
        AgentDef {
            name: "x".into(),
            file_stem: "x".into(),
            path: PathBuf::from("/tmp/x.md"),
            scope: Scope::Dir,
            assistant: assistants::claude(),
            parsed: parse(frontmatter_and_body),
            findings: Vec::new(),
        }
    }

    #[test]
    fn accessors_read_frontmatter() {
        let d = def(
            "---\nname: x\ndescription: does the thing well\nmodel: opus\ntools: Read, Grep\n---\n",
        );
        assert_eq!(d.description(), Some("does the thing well"));
        assert_eq!(d.model(), Some("opus"));
        assert_eq!(d.tools(), vec!["Read", "Grep"]);
    }

    #[test]
    fn tools_accepts_a_yaml_list_and_defaults_to_empty() {
        let list = def("---\nname: x\ntools:\n  - Read\n  - Bash\n---\n");
        assert_eq!(list.tools(), vec!["Read", "Bash"]);
        let none = def("---\nname: x\n---\n");
        assert!(none.tools().is_empty());
        assert_eq!(none.model(), None);
    }
}
