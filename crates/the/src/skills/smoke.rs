//! Deterministic, zero-latency security and safety smoke checks (`< 1 ms`).
//!
//! Catches high-confidence hazards in `SKILL.md` and bundled files:
//! - `symlink-escape` / `broken-symlink`: symlinks inside the skill directory that point outside
//!   the canonical skill directory or to non-existent targets.
//! - `trojan-source`: hidden bidirectional override/isolation or zero-width Unicode characters
//!   (excluding a valid UTF-8 BOM at byte 0).
//! - `exposed-secret`: live-shaped credential literals or private key blocks in frontmatter/body.
//! - `unrestricted-bash`: unrestricted `Bash(*)`, `Bash(sh:*)`, or `Bash(bash:*)` grants in
//!   `allowed-tools`.

use std::fs;
use std::path::Path;

use serde_json::Value;

use super::SKILL_FILE;
use super::parse::{Parsed, Problem};

const SECRET_PREFIXES: &[&str] = &[
    "sk-ant-api03-",
    "sk-proj-",
    "ghp_",
    "github_pat_",
    "AKIA",
    "xoxb-",
    "xoxp-",
];

const PRIVATE_KEY_HEADERS: &[&str] = &[
    "-----BEGIN RSA PRIVATE KEY-----",
    "-----BEGIN OPENSSH PRIVATE KEY-----",
    "-----BEGIN EC PRIVATE KEY-----",
    "-----BEGIN PRIVATE KEY-----",
    "-----BEGIN KEY-----",
];

const PLACEHOLDER_MARKERS: &[&str] = &[
    "xxxx",
    "your_",
    "your-",
    "example",
    "placeholder",
    "dummy",
    "sample",
    "redacted",
    "replace",
    "token",
    "secret",
    "insert",
    "changeme",
    "todo",
    "fake",
    "test",
];

/// Run all deterministic security and safety smoke checks on a skill.
pub fn check_smoke(raw_text: &str, p: &Parsed, skill_dir: &Path) -> Vec<Problem> {
    let mut out = Vec::new();
    check_symlinks(skill_dir, &mut out);
    check_trojan_source(raw_text, &mut out);
    check_exposed_secrets(raw_text, &mut out);
    check_unrestricted_bash(p, &mut out);
    out
}

fn check_symlinks(skill_dir: &Path, out: &mut Vec<Problem>) {
    let Ok(canonical_skill_dir) = fs::canonicalize(skill_dir) else {
        return;
    };
    walk_symlinks(skill_dir, &canonical_skill_dir, skill_dir, out);
}

fn walk_symlinks(root: &Path, canonical_root: &Path, current: &Path, out: &mut Vec<Problem>) {
    let Ok(entries) = fs::read_dir(current) else {
        return;
    };
    let mut items: Vec<_> = entries.filter_map(Result::ok).collect();
    items.sort_by_key(|e| e.path());

    for entry in items {
        let path = entry.path();
        if path
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with('.'))
        {
            continue;
        }
        let Ok(ft) = entry.file_type() else {
            continue;
        };
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .display()
            .to_string();

        if ft.is_symlink() {
            if rel == SKILL_FILE {
                continue;
            }
            match fs::canonicalize(&path) {
                Err(_) => out.push(Problem::new(
                    "broken-symlink",
                    format!("symlink `{rel}` points to a non-existent target"),
                )),
                Ok(target) if !target.starts_with(canonical_root) => out.push(Problem::new(
                    "symlink-escape",
                    format!(
                        "symlink `{rel}` resolves outside the skill directory to `{}`",
                        target.display()
                    ),
                )),
                Ok(_) => {}
            }
        } else if ft.is_dir() {
            walk_symlinks(root, canonical_root, &path, out);
        }
    }
}

fn is_trojan_unicode(ch: char) -> bool {
    matches!(
        ch,
        '\u{202A}'..='\u{202E}'
            | '\u{2066}'..='\u{2069}'
            | '\u{200E}'
            | '\u{200F}'
            | '\u{200B}'
            | '\u{200C}'
            | '\u{200D}'
            | '\u{2060}'
            | '\u{FEFF}'
    )
}

fn check_trojan_source(raw_text: &str, out: &mut Vec<Problem>) {
    for (line_idx, line) in raw_text.split('\n').enumerate() {
        let line_num = line_idx + 1;
        for (char_idx, ch) in line.chars().enumerate() {
            let col = char_idx + 1;
            // A UTF-8 BOM at byte 0 (line 1, column 1) is valid and stripped by parse.rs.
            if line_num == 1 && col == 1 && ch == '\u{FEFF}' {
                continue;
            }
            if is_trojan_unicode(ch) {
                out.push(Problem::new(
                    "trojan-source",
                    format!(
                        "hidden Unicode character U+{:04X} at SKILL.md line {line_num} column {col}",
                        ch as u32
                    ),
                ));
            }
        }
    }
}

fn check_exposed_secrets(raw_text: &str, out: &mut Vec<Problem>) {
    let lines: Vec<&str> = raw_text.lines().collect();
    for (idx, &line) in lines.iter().enumerate() {
        let line_num = idx + 1;

        for &header in PRIVATE_KEY_HEADERS {
            if let Some(pos) = line.find(header) {
                let after = line[pos + header.len()..].trim();
                let next_line = lines.get(idx + 1).copied().unwrap_or("").trim();
                let sample = if !after.is_empty() { after } else { next_line };
                if !is_private_key_placeholder(sample) {
                    out.push(Problem::new(
                        "exposed-secret",
                        format!("private key header `{header}` at SKILL.md line {line_num}"),
                    ));
                }
                break;
            }
        }

        for &prefix in SECRET_PREFIXES {
            let mut search_from = 0;
            while let Some(rel_pos) = line[search_from..].find(prefix) {
                let pos = search_from + rel_pos;
                let prev_char = line[..pos].chars().next_back();
                let after_prefix = pos + prefix.len();
                search_from = after_prefix;

                if prev_char.is_some_and(|c| c.is_ascii_alphanumeric()) {
                    continue;
                }
                let rest = &line[after_prefix..];
                if rest.starts_with('<') || rest.starts_with('{') || rest.starts_with('$') {
                    continue;
                }
                let suffix: String = rest
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
                    .collect();

                if is_live_shaped_token(prefix, &suffix) {
                    out.push(Problem::new(
                        "exposed-secret",
                        format!("credential literal `{prefix}...` at SKILL.md line {line_num}"),
                    ));
                }
            }
        }
    }
}

fn is_private_key_placeholder(sample: &str) -> bool {
    if sample.is_empty()
        || sample.starts_with("...")
        || sample.starts_with('<')
        || sample.starts_with("-----END")
    {
        return true;
    }
    let lower = sample.to_ascii_lowercase();
    PLACEHOLDER_MARKERS.iter().any(|m| lower.contains(m))
}

fn is_live_shaped_token(prefix: &str, suffix: &str) -> bool {
    if prefix == "AKIA" {
        // AWS access key IDs are `AKIA` + 16 uppercase alphanumeric characters.
        if suffix.len() < 12
            || !suffix
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        {
            return false;
        }
    } else if suffix.len() < 8 {
        return false;
    }

    let lower = suffix.to_ascii_lowercase();
    if PLACEHOLDER_MARKERS.iter().any(|m| lower.contains(m)) {
        return false;
    }
    // Reject single-character repeats like `aaaaaaaaaaaa` or `xxxxxxxxxxxx`.
    if let Some(first) = suffix.chars().next()
        && suffix.chars().all(|c| c == first)
    {
        return false;
    }
    true
}

fn check_unrestricted_bash(p: &Parsed, out: &mut Vec<Problem>) {
    let Some(val) = p.frontmatter.get("allowed-tools") else {
        return;
    };
    let mut tokens: Vec<&str> = Vec::new();
    match val {
        Value::String(s) => {
            tokens.extend(
                s.split(|c: char| c.is_whitespace() || c == ',')
                    .map(str::trim)
                    .filter(|t| !t.is_empty()),
            );
        }
        Value::Array(items) => {
            for item in items {
                if let Some(s) = item.as_str() {
                    tokens.push(s.trim());
                }
            }
        }
        _ => {}
    }

    for token in tokens {
        if matches!(token, "Bash(*)" | "Bash(sh:*)" | "Bash(bash:*)") {
            out.push(Problem::new(
                "unrestricted-bash",
                format!("frontmatter `allowed-tools` grants unrestricted shell wildcard `{token}`"),
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::parse::parse;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct TempDir {
        base: PathBuf,
    }

    impl TempDir {
        fn new() -> Self {
            static N: AtomicUsize = AtomicUsize::new(0);
            let base = std::env::temp_dir().join(format!(
                "the-smoke-ut-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::SeqCst)
            ));
            let _ = fs::remove_dir_all(&base);
            fs::create_dir_all(base.join("skill")).unwrap();
            Self {
                base: fs::canonicalize(base).unwrap(),
            }
        }

        fn skill_dir(&self) -> PathBuf {
            self.base.join("skill")
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.base);
        }
    }

    #[test]
    fn detects_trojan_unicode_with_exact_line_and_column() {
        let td = TempDir::new();
        // Leading BOM at byte 0 is allowed; mid-file RLO (U+202E) and ZWSP (U+200B) are flagged.
        let text = "\u{FEFF}---\nname: a\ndescription: clean\n---\nLine five has \u{202E}bidi and \u{200B}zwsp\n";
        let parsed = parse(text);
        let problems = check_smoke(text, &parsed, &td.skill_dir());
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert_eq!(problems[0].code, "trojan-source");
        assert!(
            problems[0]
                .message
                .contains("U+202E at SKILL.md line 5 column 15"),
            "got: {}",
            problems[0].message
        );
        assert!(
            problems[1]
                .message
                .contains("U+200B at SKILL.md line 5 column 25"),
            "got: {}",
            problems[1].message
        );
    }

    #[test]
    fn distinguishes_live_shaped_secrets_from_placeholders() {
        let td = TempDir::new();
        let clean = "\
---
name: a
description: Desc
---
Use `sk-ant-api03-xxxx` or `ghp_YOUR_TOKEN_HERE` or `sk-proj-<token>` or `AKIAIOSFODNN7EXAMPLE`.
";
        let p_clean = parse(clean);
        assert!(
            check_smoke(clean, &p_clean, &td.skill_dir()).is_empty(),
            "placeholders must not trigger exposed-secret"
        );

        let leaked = "\
---
name: a
description: Desc
---
Export ANTHROPIC_API_KEY=sk-ant-api03-9kLmN2pQrS4tUvWxYz87654321
And GITHUB_TOKEN=ghp_7mPqR9sTuVwXyZ1234567890AbCdEfGhIjKl
-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW
";
        let p_leaked = parse(leaked);
        let probs = check_smoke(leaked, &p_leaked, &td.skill_dir());
        let codes: Vec<&str> = probs.iter().map(|p| p.code).collect();
        assert_eq!(
            codes,
            ["exposed-secret", "exposed-secret", "exposed-secret"],
            "{probs:?}"
        );
    }

    #[test]
    fn detects_unrestricted_bash_in_allowed_tools() {
        let td = TempDir::new();
        let scoped = "---\nname: a\ndescription: x\nallowed-tools: Read Bash(git:*) Write\n---\n";
        assert!(check_smoke(scoped, &parse(scoped), &td.skill_dir()).is_empty());

        let wild = "---\nname: a\ndescription: x\nallowed-tools: Read Bash(*) Write\n---\n";
        let probs = check_smoke(wild, &parse(wild), &td.skill_dir());
        assert_eq!(probs.len(), 1);
        assert_eq!(probs[0].code, "unrestricted-bash");

        let wild_arr =
            "---\nname: a\ndescription: x\nallowed-tools:\n  - Bash(sh:*)\n  - Bash(bash:*)\n---\n";
        let probs_arr = check_smoke(wild_arr, &parse(wild_arr), &td.skill_dir());
        assert_eq!(probs_arr.len(), 2);
        assert!(probs_arr.iter().all(|p| p.code == "unrestricted-bash"));
    }

    #[cfg(unix)]
    #[test]
    fn detects_symlink_escape_and_broken_symlink() {
        use std::os::unix::fs::symlink;
        let td = TempDir::new();
        let outside = td.base.join("outside-secret.txt");
        fs::write(&outside, "secret").unwrap();
        let inside = td.skill_dir().join("inside.txt");
        fs::write(&inside, "ok").unwrap();

        symlink(&inside, td.skill_dir().join("valid-link.txt")).unwrap();
        symlink(&outside, td.skill_dir().join("escape-link.txt")).unwrap();
        symlink(
            td.base.join("does-not-exist.txt"),
            td.skill_dir().join("dangling-link.txt"),
        )
        .unwrap();

        let doc = "---\nname: skill\ndescription: x\n---\n";
        let probs = check_smoke(doc, &parse(doc), &td.skill_dir());
        let codes: Vec<&str> = probs.iter().map(|p| p.code).collect();
        assert_eq!(codes, ["broken-symlink", "symlink-escape"], "{probs:?}");
    }

    // --- Benchmark-ported cases (see cli/docs/scratch/skills-lint-test-plan.md, Appendix A.3) ---

    #[test]
    fn distinguishes_more_secret_prefixes_from_placeholders() {
        let td = TempDir::new();
        // Placeholders across every prefix stay silent (AKIAIOSFODNN7EXAMPLE is the canonical AWS doc key).
        let clean = "---\nname: a\ndescription: d\n---\n\
`sk-proj-<token>` `github_pat_YOUR_TOKEN_HERE` `xoxb-your-token` `AKIAIOSFODNN7EXAMPLE`\n";
        assert!(
            check_smoke(clean, &parse(clean), &td.skill_dir()).is_empty(),
            "placeholders must stay silent"
        );

        // Live-shaped literals for each prefix plus an RSA private-key header all fire.
        let leaked = "---\nname: a\ndescription: d\n---\n\
sk-proj-abcdEFGH1234ijklMNOP\n\
github_pat_11ABCDE0abcdef1234ghijkl\n\
xoxb-1234567890-ABCDEFGHIJ\n\
AKIAZ1B2C3D4E5F6G7H8\n\
-----BEGIN RSA PRIVATE KEY-----\n\
MIIEowIBAAKCAQEA1234567890abcdefNOTAREALKEY\n";
        let codes: Vec<&str> = check_smoke(leaked, &parse(leaked), &td.skill_dir())
            .iter()
            .map(|p| p.code)
            .collect();
        assert_eq!(
            codes, ["exposed-secret"; 5],
            "each live secret must fire once"
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlink_cycle_terminates_as_broken() {
        use std::os::unix::fs::symlink;
        let td = TempDir::new();
        // a -> b -> a: canonicalize hits ELOOP, so each reports broken-symlink and the walk ends.
        symlink(td.skill_dir().join("b"), td.skill_dir().join("a")).unwrap();
        symlink(td.skill_dir().join("a"), td.skill_dir().join("b")).unwrap();
        let doc = "---\nname: skill\ndescription: x\n---\n";
        let probs = check_smoke(doc, &parse(doc), &td.skill_dir());
        assert!(
            !probs.is_empty() && probs.iter().all(|p| p.code == "broken-symlink"),
            "a symlink cycle must terminate as broken-symlink, not hang: {probs:?}"
        );
    }

    #[test]
    #[ignore = "RED gap (plan §8.0): trojan-source does not yet catch ANSI/CSI/OSC/BEL terminal \
                escapes that vercel-labs/skills strips in sanitize-terminal"]
    fn detects_terminal_escape_sequences() {
        let td = TempDir::new();
        let text = "---\nname: a\ndescription: d\n---\nRed \u{1b}[31minjected\u{1b}[0m and bell \u{07} here\n";
        let probs = check_smoke(text, &parse(text), &td.skill_dir());
        assert!(
            probs.iter().any(|p| p.code == "trojan-source"),
            "ANSI/terminal escapes must be flagged: {probs:?}"
        );
    }
}
