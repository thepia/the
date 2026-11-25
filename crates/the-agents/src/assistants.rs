//! The coding-assistant runtimes `the agents` discovers agent definitions for.
//!
//! A descriptor table (the metadata-table-drives-functions principle): one row per assistant naming
//! where it keeps agent-definition files, the `project_subdir` (relative to a project root: the current
//! directory, its ancestors, or a `--dir`) and the `user_subdir` (relative to `$HOME`). Discovery folds
//! over this table, so adding an assistant is a row, not new discovery code. Grounded in each
//! assistant's authoritative docs (checked 2026-11-23); the same facts are recorded, per-assistant, in
//! the spec's `assistants/` dimension.
//!
//! Lint scoping: an assistant's frontmatter schema differs (OpenCode derives the name from the filename
//! and uses object-valued `tools`; Gemini uses `kind`/`mcpServers`; pi uses `thinking`), so the
//! Claude-specific field rules run only on Claude definitions. Every definition still gets the universal
//! checks (frontmatter parse, security smoke). As each assistant's own rule set is added, it lights up.

/// One coding-assistant runtime and where it keeps agent definitions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Assistant {
    /// Stable id shown in output and used to look the row up (e.g. `"claude"`).
    pub id: &'static str,
    /// The agent-definitions directory relative to a project root (e.g. `.claude/agents`).
    pub project_subdir: &'static str,
    /// The agent-definitions directory relative to `$HOME` (e.g. `.claude/agents`, or
    /// `.config/opencode/agents` where the assistant uses an XDG config path).
    pub user_subdir: &'static str,
}

/// The registry. Claude is first (the reference, and the `--dir` default). The trailing `agents` row is
/// the emerging cross-tool `.agents/agents` convention (read by Goose), not a single assistant.
pub const ASSISTANTS: &[Assistant] = &[
    Assistant {
        id: "claude",
        project_subdir: ".claude/agents",
        user_subdir: ".claude/agents",
    },
    Assistant {
        id: "opencode",
        project_subdir: ".opencode/agents",
        user_subdir: ".config/opencode/agents",
    },
    Assistant {
        id: "gemini",
        project_subdir: ".gemini/agents",
        user_subdir: ".gemini/agents",
    },
    Assistant {
        id: "goose",
        project_subdir: ".goose/agents",
        user_subdir: ".goose/agents",
    },
    Assistant {
        id: "pi",
        project_subdir: ".pi/agents",
        user_subdir: ".pi/agent/agents",
    },
    Assistant {
        id: "agents",
        project_subdir: ".agents/agents",
        user_subdir: ".agents/agents",
    },
];

/// The assistant whose field rules `the agents` applies fully today, and the default for a bare `--dir`
/// scan (a directory of agent files is most likely Claude-style).
pub fn claude() -> &'static Assistant {
    &ASSISTANTS[0]
}

/// The assistant with id `id`, if registered.
pub fn by_id(id: &str) -> Option<&'static Assistant> {
    ASSISTANTS.iter().find(|a| a.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_claude_is_the_reference() {
        let mut seen = std::collections::HashSet::new();
        for a in ASSISTANTS {
            assert!(seen.insert(a.id), "duplicate assistant id: {}", a.id);
        }
        assert_eq!(claude().id, "claude");
        assert_eq!(
            by_id("opencode").unwrap().user_subdir,
            ".config/opencode/agents"
        );
        assert!(by_id("nope").is_none());
    }
}
