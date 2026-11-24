//! Linting agent **instruction files**: `CLAUDE.md` / `CLAUDE.local.md` (Claude Code memory) and
//! `AGENTS.md` (the cross-tool instruction convention). These are the files an agent reads for project
//! context, as distinct from agent **definitions** (`.claude/agents/*.md`, linted in [`crate::lint`]).
//!
//! The checks are grounded in the Claude Code memory docs (code.claude.com/docs/en/memory, checked
//! 2026-11-24) and are deliberately only the **decidable, format-independent** ones, so nothing is
//! guessed:
//!
//! - **`@path` imports** (CLAUDE.md and AGENTS.md both expand them): a `@path` that does not resolve to
//!   a file silently drops instructions. Resolution follows the docs: relative paths resolve against the
//!   importing file's directory (not the cwd); `~` expands to home; a path in backticks (code span) or a
//!   fenced code block is NOT an import; a quoted path is NOT imported; spaces may be backslash-escaped.
//! - **Size**: Claude Code **skips a file over 4 MiB** (a hard load failure), and a file **over 200
//!   lines** reduces instruction adherence (the docs' recommended ceiling).
//! - **Security smoke**: the reused text-level smoke (hidden control/bidi characters, credential
//!   literals), via [`the_skills::smoke::check_text_smoke`] (no directory walk, so it is safe on a file
//!   at a project root).
//!
//! Deliberately deferred (not guessed): AGENTS.md convention-conformance (the format has no required
//! schema), `@import` depth > 4 chaining, and relative-link integrity. Tracked in WORK-0036.

use std::path::{Path, PathBuf};

use serde::Serialize;
use the_output::diag::{Diagnostic, Exit};
use the_skills::locate::Env;

use crate::Scope;
use crate::lint::Finding;
use crate::rules::Severity;

/// Claude Code skips an instruction file larger than this (4 MiB): a hard load failure.
const MAX_BYTES: u64 = 4 * 1024 * 1024;
/// The docs' recommended ceiling; beyond it, adherence drops (a note, not a failure).
const LONG_LINES: usize = 200;

/// One instruction-file lint rule. A small registry parallel to [`crate::rules`] (the metadata-table
/// principle), kept separate because instruction files are a different surface from agent definitions.
#[derive(Debug, Clone, Copy)]
pub struct InstructionRule {
    pub code: &'static str,
    pub label: &'static str,
    pub why: &'static str,
    pub fix: &'static str,
    pub severity: Severity,
}

/// The instruction-file rule registry. Grounded in the Claude Code memory docs (2026-11-24).
pub const INSTRUCTION_RULES: &[InstructionRule] = &[
    InstructionRule {
        code: "instruction-broken-import",
        label: "broken `@path` import",
        why: "a `@path` import that does not resolve to a file is silently dropped, so the instructions it names never load",
        fix: "correct the path (it resolves relative to this file, `~` is home), create the missing file, or wrap it in backticks if it was not meant as an import",
        severity: Severity::Warn,
    },
    InstructionRule {
        code: "instruction-file-too-large",
        label: "instruction file over 4 MiB",
        why: "Claude Code skips an instruction file larger than 4 MiB, so none of its content loads",
        fix: "split the file, or move bulk content into `@path` imports or path-scoped `.claude/rules/` files",
        severity: Severity::Error,
    },
    InstructionRule {
        code: "instruction-file-long",
        label: "instruction file over 200 lines",
        why: "a file over ~200 lines consumes more context and reduces how closely Claude follows it",
        fix: "trim to the instructions that matter every session, or move scoped content into `.claude/rules/` or `@path` imports",
        severity: Severity::Note,
    },
    InstructionRule {
        code: "trojan-source",
        label: "hidden control or bidirectional character",
        why: "hidden bidirectional or zero-width characters can conceal instructions from a human reviewing the file",
        fix: "remove hidden bidirectional, zero-width, or terminal-control characters from the file",
        severity: Severity::Error,
    },
    InstructionRule {
        code: "exposed-secret",
        label: "exposed secret or credential literal",
        why: "a committed credential in an instruction file leaks a secret to anyone who reads it",
        fix: "remove the credential literal and reference an environment variable instead",
        severity: Severity::Error,
    },
];

/// The rule for `code`, if registered.
pub fn rule(code: &str) -> Option<&'static InstructionRule> {
    INSTRUCTION_RULES.iter().find(|r| r.code == code)
}

/// True for an instruction-file name (`CLAUDE.md` / `CLAUDE.local.md` / `AGENTS.md`). Agent-definition
/// discovery excludes these: they are instruction files, linted by the instruction pass of
/// `the agents lint`, not agent definitions, so they must not be read as empty/invalid agent defs.
pub fn is_instruction_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| FILES.iter().any(|(name, _)| *name == n))
}

/// Which instruction file this is (drives only the displayed kind; the checks are shared).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    ClaudeMd,
    ClaudeLocalMd,
    AgentsMd,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::ClaudeMd => "CLAUDE.md",
            Kind::ClaudeLocalMd => "CLAUDE.local.md",
            Kind::AgentsMd => "AGENTS.md",
        }
    }
}

/// The instruction-file names discovered in each directory, with their kind.
const FILES: &[(&str, Kind)] = &[
    ("CLAUDE.md", Kind::ClaudeMd),
    ("CLAUDE.local.md", Kind::ClaudeLocalMd),
    ("AGENTS.md", Kind::AgentsMd),
];

/// One discovered instruction file, parsed and linted.
#[derive(Debug)]
pub struct InstructionFile {
    pub path: PathBuf,
    pub kind: Kind,
    pub scope: Scope,
    pub findings: Vec<Finding>,
}

/// Discover instruction files across the project chain (the cwd and its ancestors up to, but not
/// including, the home directory), the user scope (`$HOME/.claude/CLAUDE.md`, unless `no_user`), and each
/// `--dir`. The home directory ends the project walk so a home-level file is the single User root.
pub fn discover(env: &Env, dirs: &[PathBuf], no_user: bool) -> Vec<InstructionFile> {
    let mut out = Vec::new();
    let home = env.home.clone();

    // Project: the cwd and each ancestor, stopping at the home directory (exclusive).
    for dir in env.cwd.ancestors() {
        if home.as_deref() == Some(dir) {
            break;
        }
        collect_dir(dir, Scope::Project, home.as_deref(), &mut out);
    }
    // User: ~/.claude/CLAUDE.md (the user memory file).
    if !no_user && let Some(h) = &home {
        let user_claude = h.join(".claude").join("CLAUDE.md");
        push_if_file(
            &user_claude,
            Kind::ClaudeMd,
            Scope::User,
            home.as_deref(),
            &mut out,
        );
    }
    // Each --dir: the three instruction files directly in it.
    for d in dirs {
        collect_dir(d, Scope::Dir, home.as_deref(), &mut out);
    }
    out
}

fn collect_dir(dir: &Path, scope: Scope, home: Option<&Path>, out: &mut Vec<InstructionFile>) {
    for (name, kind) in FILES {
        push_if_file(&dir.join(name), *kind, scope, home, out);
    }
}

fn push_if_file(
    path: &Path,
    kind: Kind,
    scope: Scope,
    home: Option<&Path>,
    out: &mut Vec<InstructionFile>,
) {
    if !path.is_file() {
        return;
    }
    let raw = std::fs::read_to_string(path).unwrap_or_default();
    let findings = lint_file(path, &raw, home);
    out.push(InstructionFile {
        path: path.to_path_buf(),
        kind,
        scope,
        findings,
    });
}

/// Lint one instruction file's text: size, `@path` import resolution, and security smoke.
pub fn lint_file(path: &Path, raw: &str, home: Option<&Path>) -> Vec<Finding> {
    let mut out = Vec::new();

    // Size: a >4 MiB file is skipped entirely (error); a >200-line file reduces adherence (note).
    if raw.len() as u64 > MAX_BYTES {
        out.push(finding(
            "instruction-file-too-large",
            "the file is over 4 MiB",
            None,
        ));
    } else if raw.lines().count() > LONG_LINES {
        out.push(finding(
            "instruction-file-long",
            format!("the file is {} lines", raw.lines().count()),
            None,
        ));
    }

    // Imports: each `@path` that does not resolve to a file.
    let dir = path.parent().unwrap_or(Path::new("."));
    for imp in extract_imports(raw) {
        if !resolve_import(&imp.path, dir, home).is_file() {
            out.push(finding(
                "instruction-broken-import",
                format!("`@{}` does not resolve to a file", imp.path),
                Some(imp.line),
            ));
        }
    }

    // Security smoke (text-only: no directory walk).
    for p in the_skills::smoke::check_text_smoke(raw) {
        let message = match p.code {
            "trojan-source" => "a hidden control or bidirectional character is present",
            "exposed-secret" => "a credential literal is present",
            _ => continue,
        };
        out.push(Finding {
            code: p.code,
            message: message.to_owned(),
            line: p.line,
            col: p.col,
        });
    }

    out
}

fn finding(code: &'static str, message: impl Into<String>, line: Option<usize>) -> Finding {
    Finding {
        code,
        message: message.into(),
        line,
        col: None,
    }
}

/// A `@path` import found in an instruction file.
#[derive(Debug, PartialEq, Eq)]
struct Import {
    path: String,
    line: usize,
}

/// Extract `@path` imports per the Claude Code memory rules: skip fenced code blocks and inline code
/// spans; a quoted path is not imported; a path ends at the first unescaped space (`\ ` is a literal
/// space); the `@` must start a token (start of line or after whitespace), so `user@host` is not an
/// import.
fn extract_imports(raw: &str) -> Vec<Import> {
    let mut out = Vec::new();
    let mut in_fence = false;
    for (i, line) in raw.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        for path in imports_in_line(&strip_code_spans(line)) {
            out.push(Import { path, line: i + 1 });
        }
    }
    out
}

/// Drop inline code spans (backtick-delimited) so a `` `@README` `` mention is not read as an import.
/// Segments between the 1st/2nd, 3rd/4th, ... backticks are inside spans and are removed.
fn strip_code_spans(line: &str) -> String {
    line.split('`')
        .enumerate()
        .filter(|(i, _)| i % 2 == 0)
        .map(|(_, s)| s)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Find each `@path` token in one already-code-span-stripped line.
fn imports_in_line(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < bytes.len() {
        // The `@` must start a token: start of line, after whitespace, or after an opening bracket (so
        // a parenthetical `(@path)` is caught). After an alphanumeric it is not an import (`user@host`).
        let at_boundary =
            i == 0 || bytes[i - 1].is_whitespace() || matches!(bytes[i - 1], '(' | '[' | '{' | '<');
        if bytes[i] == '@' && at_boundary {
            // A quoted path right after `@` is not imported.
            if matches!(bytes.get(i + 1), Some('"') | Some('\'')) {
                i += 1;
                continue;
            }
            let mut path = String::new();
            let mut j = i + 1;
            while j < bytes.len() {
                let c = bytes[j];
                if c == '\\' && bytes.get(j + 1).is_some_and(|n| n.is_whitespace()) {
                    path.push(bytes[j + 1]);
                    j += 2;
                    continue;
                }
                if c.is_whitespace() {
                    break;
                }
                path.push(c);
                j += 1;
            }
            // Trailing sentence punctuation is almost always prose, not part of the path (the import
            // mechanism ends the path at whitespace, so `See @a/b.md.` would include the period; a lint
            // that flagged that resolvable file would be a false positive). Trim it for resolution.
            let trimmed = path
                .trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']', '}', '"', '\''])
                .to_string();
            if !trimmed.is_empty() {
                out.push(trimmed);
            }
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

/// Resolve an import path to a filesystem path per the docs: `~`/`~/...` expands to home; an absolute
/// path is used as-is; a relative path resolves against the importing file's directory (not the cwd).
fn resolve_import(path: &str, file_dir: &Path, home: Option<&Path>) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(h) = home
    {
        return h.join(rest);
    }
    if path == "~"
        && let Some(h) = home
    {
        return h.to_path_buf();
    }
    let p = Path::new(path);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        file_dir.join(p)
    }
}

/// Render an instruction finding as a `the-output` diagnostic (the same shape as a definition finding:
/// label leads the title, message + why form the cause, fix is the help, `path:line` is the context).
pub fn diagnostic(f: &Finding, file: &InstructionFile) -> Diagnostic {
    let r = rule(f.code);
    let label = r.map_or("instruction file issue", |r| r.label);
    let severity = r.map_or(Severity::Note, |r| r.severity);

    let title = format!("{}: {label}", file.kind.as_str());
    let d = match severity {
        Severity::Error => Diagnostic::error(f.code, title).exit(Exit::Generic),
        Severity::Warn => Diagnostic::warning(f.code, title),
        Severity::Note => Diagnostic::note(f.code, title),
    };
    let location = match f.line {
        Some(line) => format!("{}:{line}", file.path.display()),
        None => file.path.display().to_string(),
    };
    let cause = match r.map(|r| r.why) {
        Some(why) => format!("{}; {why}", f.message),
        None => f.message.clone(),
    };
    let d = d.context(location).cause(cause);
    match r.map(|r| r.fix) {
        Some(fix) => d.help(fix),
        None => d,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_rule_is_unique_and_states_what_why_fix() {
        let mut seen = std::collections::HashSet::new();
        for r in INSTRUCTION_RULES {
            assert!(
                seen.insert(r.code),
                "duplicate instruction rule: {}",
                r.code
            );
            assert!(!r.label.is_empty() && !r.why.is_empty() && !r.fix.is_empty());
        }
    }

    #[test]
    fn extracts_plain_and_home_imports_and_skips_code_and_quotes() {
        let raw = "\
See @docs/guide.md for setup.
Also @~/.claude/shared.md loads from home.
Inline `@README` is not an import.
A quoted @\"not imported.md\" is skipped.
An email user@example.com is not an import.
```
@inside/fence.md is not an import.
```
";
        let imps: Vec<String> = extract_imports(raw).into_iter().map(|i| i.path).collect();
        assert_eq!(
            imps,
            vec![
                "docs/guide.md".to_string(),
                "~/.claude/shared.md".to_string()
            ],
            "only the two real imports, not code/quote/email/fence"
        );
    }

    #[test]
    fn trailing_sentence_punctuation_is_trimmed_not_part_of_the_path() {
        // Prose "See @docs/guide.md." must yield `docs/guide.md`, not `docs/guide.md.` (a false broken
        // import on a file that exists).
        let imps: Vec<String> = extract_imports("See @docs/guide.md. Also (@x/y.md).")
            .into_iter()
            .map(|i| i.path)
            .collect();
        assert_eq!(
            imps,
            vec!["docs/guide.md".to_string(), "x/y.md".to_string()]
        );
    }

    #[test]
    fn backslash_escaped_space_is_part_of_the_path() {
        let imps: Vec<String> = extract_imports("@Design\\ Docs/spec.md here")
            .into_iter()
            .map(|i| i.path)
            .collect();
        assert_eq!(imps, vec!["Design Docs/spec.md".to_string()]);
    }

    #[test]
    fn broken_and_present_imports_are_distinguished() {
        let dir = std::env::temp_dir().join(format!("the-instr-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("docs")).unwrap();
        std::fs::write(dir.join("docs/guide.md"), "hi").unwrap();
        let claude = dir.join("CLAUDE.md");
        std::fs::write(
            &claude,
            "See @docs/guide.md (exists) and @docs/missing.md (absent).\n",
        )
        .unwrap();

        let findings = lint_file(&claude, &std::fs::read_to_string(&claude).unwrap(), None);
        let broken: Vec<_> = findings
            .iter()
            .filter(|f| f.code == "instruction-broken-import")
            .collect();
        assert_eq!(
            broken.len(),
            1,
            "only the absent import is flagged: {findings:?}"
        );
        assert!(broken[0].message.contains("docs/missing.md"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn oversized_by_lines_is_a_note() {
        let raw = "line\n".repeat(LONG_LINES + 5);
        let findings = lint_file(Path::new("/tmp/CLAUDE.md"), &raw, None);
        assert!(findings.iter().any(|f| f.code == "instruction-file-long"));
    }

    #[test]
    fn exposed_secret_in_an_instruction_file_is_flagged() {
        let raw = "Token: ghp_abcdefghijklmnopqrstuvwxyz0123456789\n";
        let findings = lint_file(Path::new("/tmp/AGENTS.md"), raw, None);
        assert!(findings.iter().any(|f| f.code == "exposed-secret"));
    }

    #[test]
    fn oversized_by_bytes_is_a_hard_error() {
        let raw = "x".repeat((MAX_BYTES + 1) as usize);
        let findings = lint_file(Path::new("/tmp/CLAUDE.md"), &raw, None);
        assert!(
            findings
                .iter()
                .any(|f| f.code == "instruction-file-too-large"),
            "a >4 MiB file is a hard error"
        );
        // The too-large error pre-empts the long-lines note (one size finding, not both).
        assert!(!findings.iter().any(|f| f.code == "instruction-file-long"));
    }

    #[test]
    fn resolve_import_handles_home_and_absolute_and_relative() {
        let home = Path::new("/home/u");
        assert_eq!(
            resolve_import("~/.claude/x.md", Path::new("/proj"), Some(home)),
            PathBuf::from("/home/u/.claude/x.md")
        );
        assert_eq!(
            resolve_import("~", Path::new("/proj"), Some(home)),
            PathBuf::from("/home/u")
        );
        assert_eq!(
            resolve_import("/abs/x.md", Path::new("/proj"), Some(home)),
            PathBuf::from("/abs/x.md")
        );
        assert_eq!(
            resolve_import("docs/x.md", Path::new("/proj"), Some(home)),
            PathBuf::from("/proj/docs/x.md")
        );
    }

    #[test]
    fn diagnostic_renders_each_severity_with_label_cause_and_fix() {
        let file = InstructionFile {
            path: PathBuf::from("/proj/CLAUDE.md"),
            kind: Kind::ClaudeMd,
            scope: Scope::Project,
            findings: Vec::new(),
        };
        // Warning (broken import), with a line -> rendered as path:line.
        let warn = diagnostic(
            &finding(
                "instruction-broken-import",
                "`@x` does not resolve to a file",
                Some(3),
            ),
            &file,
        );
        assert_eq!(warn.code, "instruction-broken-import");
        assert!(warn.context.iter().any(|c| c.contains("CLAUDE.md:3")));
        assert!(warn.help.is_some(), "the fix is the help");
        // Error (too large) and note (long) resolve their severities from the registry.
        let err = diagnostic(&finding("instruction-file-too-large", "big", None), &file);
        assert_eq!(err.exit, Exit::Generic);
        let note = diagnostic(&finding("instruction-file-long", "long", None), &file);
        assert_eq!(note.level, the_output::diag::Level::Note);
    }
}
