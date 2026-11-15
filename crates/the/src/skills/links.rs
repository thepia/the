//! Fence-aware local link and asset integrity checks for `SKILL.md`.
//!
//! Scans the Markdown body of a skill while tracking fenced code blocks (` ``` ` and `~~~`) and
//! stripping inline code spans (`` `...` ``) so placeholder links inside code examples are ignored.
//! Relative links outside code are resolved against the skill directory to detect missing local
//! files (`broken-link`) and path traversal outside the skill directory (`link-escapes-skill`).

use std::ffi::OsString;
use std::fs;
use std::path::{Component, Path, PathBuf};

use super::parse::{Parsed, Problem};

const EXTERNAL_SCHEMES: &[&str] = &[
    "http://",
    "https://",
    "mailto:",
    "mdc:",
    "conversation://",
    "file://",
];

/// Check all Markdown links in `p.body` against `skill_dir`.
pub fn check_links(p: &Parsed, skill_dir: &Path) -> Vec<Problem> {
    check_body_links(&p.body, p.body_start_line.max(1), skill_dir)
}

/// Scan `body` starting at 1-based `body_start_line` in `SKILL.md`.
pub fn check_body_links(body: &str, body_start_line: usize, skill_dir: &Path) -> Vec<Problem> {
    let mut out = Vec::new();
    let canonical_skill_dir =
        fs::canonicalize(skill_dir).unwrap_or_else(|_| skill_dir.to_path_buf());
    let mut active_fence: Option<(char, usize)> = None;
    // CommonMark indented code blocks (>= 4 columns of leading whitespace) are not fenced but still
    // code, so links inside them are not real. An indented block cannot interrupt a paragraph, so it
    // only starts after a blank line (or continues a block already in indented code).
    let mut prev_blank = true;
    let mut in_indented_code = false;

    for (idx, raw_line) in body.lines().enumerate() {
        let line_num = body_start_line.max(1) + idx;
        let trimmed = raw_line.trim_start();

        if let Some((fence_ch, fence_len)) = active_fence {
            if is_closing_fence(trimmed, fence_ch, fence_len) {
                active_fence = None;
            }
            prev_blank = false;
            continue;
        }
        if let Some(fence) = parse_opening_fence(trimmed) {
            active_fence = Some(fence);
            prev_blank = false;
            in_indented_code = false;
            continue;
        }
        if trimmed.is_empty() {
            prev_blank = true;
            continue;
        }
        if indent_columns(raw_line) >= 4 && (prev_blank || in_indented_code) {
            in_indented_code = true;
            prev_blank = false;
            continue;
        }
        in_indented_code = false;
        prev_blank = false;

        let prose = strip_inline_code(raw_line);
        for raw_target in extract_markdown_links(&prose) {
            let (angle, target) = normalize_target(raw_target);
            if should_ignore_target(target) {
                continue;
            }
            let Some(clean_path) = local_path(strip_fragment_and_query(target), angle) else {
                continue;
            };
            if clean_path.is_empty() {
                continue;
            }
            let rel_path = Path::new(clean_path);
            let Some(lexical_target) = resolve_within(&canonical_skill_dir, rel_path) else {
                out.push(Problem::new(
                    "link-escapes-skill",
                    format!(
                        "link `{raw_target}` at SKILL.md line {line_num} resolves outside the skill directory"
                    ),
                ));
                continue;
            };
            if !lexical_target.exists() {
                out.push(Problem::new(
                    "broken-link",
                    format!(
                        "broken link `{raw_target}` at SKILL.md line {line_num}: `{clean_path}` does not exist"
                    ),
                ));
                continue;
            }
            if let Ok(canon_target) = fs::canonicalize(&lexical_target)
                && !canon_target.starts_with(&canonical_skill_dir)
            {
                out.push(Problem::new(
                    "link-escapes-skill",
                    format!(
                        "link `{raw_target}` at SKILL.md line {line_num} resolves outside the skill directory"
                    ),
                ));
            }
        }
    }

    out
}

fn parse_opening_fence(trimmed: &str) -> Option<(char, usize)> {
    let first = trimmed.chars().next()?;
    if first != '`' && first != '~' {
        return None;
    }
    let len = trimmed.chars().take_while(|&c| c == first).count();
    if len < 3 {
        return None;
    }
    let rest = &trimmed[len * first.len_utf8()..];
    // In CommonMark, a backtick fence info string cannot contain backticks (that is an inline code span).
    if first == '`' && rest.contains('`') {
        return None;
    }
    Some((first, len))
}

/// Leading-whitespace width in columns, counting a tab as 4 (CommonMark tab stop). Used to detect
/// indented code blocks.
fn indent_columns(line: &str) -> usize {
    let mut cols = 0;
    for c in line.chars() {
        match c {
            ' ' => cols += 1,
            '\t' => cols += 4,
            _ => break,
        }
    }
    cols
}

fn is_closing_fence(trimmed: &str, fence_ch: char, min_len: usize) -> bool {
    let count = trimmed.chars().take_while(|&c| c == fence_ch).count();
    if count < min_len {
        return false;
    }
    let rest = &trimmed[count * fence_ch.len_utf8()..];
    rest.trim().is_empty()
}

/// Remove inline code spans (`` `...` `` or `` ``...`` ``) from a single line.
fn strip_inline_code(line: &str) -> String {
    let bytes = line.as_bytes();
    let mut out = String::with_capacity(line.len());
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] == b'`' {
            let start = i;
            while i < bytes.len() && bytes[i] == b'`' {
                i += 1;
            }
            let tick_len = i - start;
            let mut cursor = i;
            let mut closed_at = None;
            while cursor < bytes.len() {
                if bytes[cursor] == b'`' {
                    let run_start = cursor;
                    while cursor < bytes.len() && bytes[cursor] == b'`' {
                        cursor += 1;
                    }
                    if cursor - run_start == tick_len {
                        closed_at = Some(cursor);
                        break;
                    }
                } else {
                    cursor += 1;
                }
            }
            if let Some(after_close) = closed_at {
                i = after_close;
            } else {
                out.push_str(&line[start..i]);
            }
        } else {
            let ch = line[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }

    out
}

/// Extract the `target` portion of every `[label](target)` in `line`.
fn extract_markdown_links(line: &str) -> Vec<&str> {
    let mut targets = Vec::new();
    let bytes = line.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] == b'\\' {
            i += 2;
            continue;
        }
        if bytes[i] == b'[' {
            let mut j = i + 1;
            let mut bracket_depth = 1usize;
            while j < bytes.len() && bracket_depth > 0 {
                if bytes[j] == b'\\' {
                    j += 2;
                    continue;
                }
                if bytes[j] == b'[' {
                    bracket_depth += 1;
                } else if bytes[j] == b']' {
                    bracket_depth -= 1;
                }
                j += 1;
            }
            if bracket_depth == 0 && j < bytes.len() && bytes[j] == b'(' {
                let target_start = j + 1;
                let mut k = target_start;
                let mut paren_depth = 1usize;
                while k < bytes.len() && paren_depth > 0 {
                    if bytes[k] == b'\\' {
                        k += 2;
                        continue;
                    }
                    if bytes[k] == b'(' {
                        paren_depth += 1;
                    } else if bytes[k] == b')' {
                        paren_depth -= 1;
                    }
                    if paren_depth > 0 {
                        k += 1;
                    }
                }
                if paren_depth == 0 {
                    targets.push(&line[target_start..k]);
                    i = k + 1;
                    continue;
                }
            }
        }
        i += 1;
    }

    targets
}

/// The link destination, and whether it was written in CommonMark's `<...>` form.
fn normalize_target(raw: &str) -> (bool, &str) {
    let trimmed = raw.trim();
    if let Some(inner) = trimmed.strip_prefix('<').and_then(|s| s.strip_suffix('>')) {
        return (true, inner.trim());
    }
    // Strip optional Markdown link title: `path/to/file.md "Title"` or `path/to/file.md 'Title'`.
    if let Some((path_part, rest)) = trimmed.split_once(char::is_whitespace) {
        let rest = rest.trim_start();
        if rest.starts_with('"') || rest.starts_with('\'') {
            return (false, path_part);
        }
    }
    (false, trimmed)
}

const SKILL_DIR_VAR: &str = "${CLAUDE_SKILL_DIR}";
const PROJECT_DIR_VAR: &str = "${CLAUDE_PROJECT_DIR}";

/// The path to check inside the skill, or `None` when the target is not a checkable local path.
///
/// - `${CLAUDE_SKILL_DIR}` (Claude Code's variable for the skill directory) expands to the skill
///   directory, so `${CLAUDE_SKILL_DIR}/scripts/x.sh` is checked like `scripts/x.sh`.
/// - `${CLAUDE_PROJECT_DIR}` targets are skipped: the project is whichever one the session runs in,
///   so they point outside the skill on purpose and cannot be resolved here.
/// - Anything still holding `{`, `}` or `$` is a template or an unknown variable (`{{name}}.md`).
/// - A `<...>` destination is CommonMark's form for a path with spaces (`<my file.md>`), so it is
///   checked; only a bare word with no `/` or `.` (`<template>`, `<name>`) is a placeholder.
fn local_path(path: &str, angle: bool) -> Option<&str> {
    if path.starts_with(PROJECT_DIR_VAR) {
        return None;
    }
    let path = match path.strip_prefix(SKILL_DIR_VAR) {
        Some(rest) => rest.trim_start_matches('/'),
        None => path,
    };
    if path.contains(['{', '}', '$']) {
        return None;
    }
    if angle && !path.contains(['/', '.']) {
        return None;
    }
    Some(path)
}

fn should_ignore_target(target: &str) -> bool {
    if target.is_empty() || target.starts_with('#') {
        return true;
    }
    let lower = target.to_ascii_lowercase();
    EXTERNAL_SCHEMES
        .iter()
        .any(|scheme| lower.starts_with(scheme))
}

fn strip_fragment_and_query(target: &str) -> &str {
    let without_fragment = target.split('#').next().unwrap_or(target);
    without_fragment
        .split('?')
        .next()
        .unwrap_or(without_fragment)
        .trim()
}

/// Lexically resolve `rel` against `skill_dir`, returning `None` if `rel` escapes `skill_dir`.
fn resolve_within(skill_dir: &Path, rel: &Path) -> Option<PathBuf> {
    if rel.is_absolute() {
        return if rel.starts_with(skill_dir) {
            Some(rel.to_path_buf())
        } else {
            None
        };
    }

    let mut stack: Vec<OsString> = Vec::new();
    for comp in rel.components() {
        match comp {
            Component::CurDir => {}
            Component::Normal(part) => stack.push(part.to_os_string()),
            Component::ParentDir => {
                stack.pop()?;
            }
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }

    let mut out = skill_dir.to_path_buf();
    for part in stack {
        out.push(part);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::parse::parse;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct TempSkill {
        dir: PathBuf,
    }

    impl TempSkill {
        fn new() -> Self {
            static N: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "the-links-ut-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::SeqCst)
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self {
                dir: fs::canonicalize(dir).unwrap(),
            }
        }

        fn write(&self, rel: &str, content: &str) {
            let p = self.dir.join(rel);
            if let Some(parent) = p.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(p, content).unwrap();
        }
    }

    impl Drop for TempSkill {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn variables_and_placeholders_are_resolved_or_skipped_not_reported() {
        let sk = TempSkill::new();
        sk.write("scripts/x.sh", "#!/bin/sh\n");
        sk.write("my file.md", "spaces\n");
        let body = "\
[ok](${CLAUDE_SKILL_DIR}/scripts/x.sh) [proj](${CLAUDE_PROJECT_DIR}/docs/a.md)
[t](<template>) [n]({{name}}.md) [e]($HOME/x.md) [spaced](<my file.md>)
[gone](${CLAUDE_SKILL_DIR}/scripts/missing.sh) [gone2](<missing file.md>)
[out](${CLAUDE_SKILL_DIR}/../escape.md)
";
        let codes: Vec<(&str, String)> = check_body_links(body, 1, &sk.dir)
            .into_iter()
            .map(|p| (p.code, p.message))
            .collect();
        assert_eq!(codes.len(), 3, "{codes:?}");
        assert!(
            codes[0].0 == "broken-link" && codes[0].1.contains("missing.sh"),
            "{codes:?}"
        );
        assert!(
            codes[1].0 == "broken-link" && codes[1].1.contains("missing file.md"),
            "{codes:?}"
        );
        assert_eq!(codes[2].0, "link-escapes-skill", "{codes:?}");
    }

    #[test]
    fn skips_fenced_and_inline_code_and_checks_real_links() {
        let sk = TempSkill::new();
        sk.write("references/guide.md", "# Guide\n");
        sk.write("scripts/run.sh", "#!/bin/sh\n");

        let doc = "\
---
name: demo
description: Demo skill
---

# Demo

Valid links: [guide](references/guide.md#overview) and [script](scripts/run.sh?mode=fast).
External and anchor links: [web](https://example.com/a) [mail](mailto:a@b.c) [sec](#intro) [file](file:///tmp/x) [mdc](mdc:rule) [conv](conversation://123).
Inline code ignored: `[ghost](missing-inline.md)` and ``[ghost2](missing-inline-2.md)``.

```markdown
[fenced-backtick](missing-fenced-1.md)
```

~~~
[fenced-tilde](missing-fenced-2.md)
~~~

Broken link on line 20: [missing](references/absent.md)
Escaping link on line 21: [escape](../../etc/passwd)
Sibling escape on line 22: [other](../other-skill/SKILL.md)
";

        let parsed = parse(doc);
        let problems = check_links(&parsed, &sk.dir);
        let codes: Vec<&str> = problems.iter().map(|p| p.code).collect();
        assert_eq!(
            codes,
            ["broken-link", "link-escapes-skill", "link-escapes-skill"],
            "{problems:?}"
        );
        assert!(
            problems[0].message.contains("SKILL.md line 20")
                && problems[0].message.contains("references/absent.md"),
            "unexpected broken-link message: {}",
            problems[0].message
        );
        assert!(
            problems[1].message.contains("../../etc/passwd"),
            "unexpected escape message: {}",
            problems[1].message
        );
    }

    #[test]
    fn skips_links_in_four_space_indented_code() {
        let sk = TempSkill::new();
        let doc = "\
---
name: a
description: d
---

Example:

    [ghost](missing-indented.md)
";
        assert!(
            check_links(&parse(doc), &sk.dir).is_empty(),
            "links inside a 4-space indented code block must be ignored"
        );
    }
}
