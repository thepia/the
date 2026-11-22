//! End-to-end CLI tests for `the skills lint`, exercising the real binary over a fixture skill tree.
//!
//! These complement the unit tests in `the-skills` by proving the rules surface through the actual
//! command path (argument parsing, resolve, the `--beta` gate, JSON rendering), not just in-process
//! helper calls. They caught a real gotcha during development: a stale compiler cache served the
//! binary old lint logic while the unit-test artifact ran fresh, so the unit tests were green but the
//! shipped command was not. `CARGO_BIN_EXE_the` is the binary cargo builds for this test run, so it
//! is always the current code.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::Value;

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    /// A fresh skills-root directory (its children are skills), unique per test.
    fn new() -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "the-lint-cli-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        Self { root }
    }

    fn write(&self, rel: &str, content: &str) {
        let p = self.root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }

    /// Run `the skills lint --dir <root> --scope dir [--beta] --format json` and return the lint
    /// codes for `skill_name`.
    fn lint_codes(&self, skill_name: &str, beta: bool) -> Vec<String> {
        let mut args = vec![
            "skills".to_string(),
            "lint".to_string(),
            "--dir".to_string(),
            self.root.to_string_lossy().into_owned(),
            "--scope".to_string(),
            "dir".to_string(),
            "--format".to_string(),
            "json".to_string(),
        ];
        if beta {
            args.push("--beta".to_string());
        }
        let out = Command::new(env!("CARGO_BIN_EXE_the"))
            .args(&args)
            .output()
            .expect("run the skills lint");
        let doc: Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
            panic!(
                "parse lint json: {e}\nstdout: {}\nstderr: {}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            )
        });
        let skill = doc["skills"]
            .as_array()
            .expect("skills array")
            .iter()
            .find(|s| s["name"].as_str() == Some(skill_name))
            .unwrap_or_else(|| panic!("skill `{skill_name}` not found in lint output"));
        skill["lint"]
            .as_array()
            .expect("lint array")
            .iter()
            .filter_map(|p| p["code"].as_str().map(str::to_owned))
            .collect()
    }

    /// Run `the skills lint <positional> --format json` and return the names of the skills found.
    fn skill_names_positional(&self, positional: &str) -> Vec<String> {
        let out = Command::new(env!("CARGO_BIN_EXE_the"))
            .args(["skills", "lint", positional, "--format", "json"])
            .output()
            .expect("run the skills lint <positional>");
        let doc: Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
            panic!(
                "parse json: {e}\nstdout: {}\nstderr: {}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            )
        });
        doc["skills"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|s| s["name"].as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Raw stderr of a `the skills lint [--beta]` run (where corpus-level diagnostics like
    /// `skill-overlap` / `redundant-with` are emitted, separate from the per-skill lint array).
    fn lint_stderr(&self, beta: bool) -> String {
        let mut args = vec![
            "skills".to_string(),
            "lint".to_string(),
            "--dir".to_string(),
            self.root.to_string_lossy().into_owned(),
            "--scope".to_string(),
            "dir".to_string(),
            "--format".to_string(),
            "json".to_string(),
        ];
        if beta {
            args.push("--beta".to_string());
        }
        let out = Command::new(env!("CARGO_BIN_EXE_the"))
            .args(&args)
            .output()
            .expect("run the skills lint");
        String::from_utf8_lossy(&out.stderr).into_owned()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// A fixture skill that trips all three Tier-3/4 Beta rules (and a default broken-link), so we can
/// prove both the firing and the `--beta` gate in one run.
fn write_tripwire_skill(fx: &Fixture) {
    fx.write(
        "demo/SKILL.md",
        "---\nname: demo\ndescription: A handy helper for various tasks and much more across your whole workflow today.\n---\n\n# Demo\n\nSee the [guide](references/guide.md#nope) and [top](#missing) section.\nAlso [gone](references/absent.md).\n",
    );
    fx.write("demo/references/guide.md", "# Guide\n\n## Overview\n");
    fx.write(
        "demo/references/orphan.txt",
        "leftover, referenced by nothing\n",
    );
}

#[test]
fn beta_rules_fire_through_the_cli_only_with_beta() {
    let fx = Fixture::new();
    write_tripwire_skill(&fx);

    // Without --beta: only the default rule (broken-link) surfaces; the three Beta rules are gated.
    let plain = fx.lint_codes("demo", false);
    assert!(
        plain.contains(&"broken-link".to_string()),
        "default broken-link must always surface: {plain:?}"
    );
    for beta_code in [
        "broken-link-fragment",
        "orphan-unreferenced-file",
        "description-vague-generic",
    ] {
        assert!(
            !plain.contains(&beta_code.to_string()),
            "{beta_code} must be gated behind --beta, but surfaced without it: {plain:?}"
        );
    }

    // With --beta: all three Beta rules surface through the real command path.
    let beta = fx.lint_codes("demo", true);
    for code in [
        "broken-link-fragment",
        "orphan-unreferenced-file",
        "description-vague-generic",
    ] {
        assert!(
            beta.contains(&code.to_string()),
            "{code} must surface with --beta: {beta:?}"
        );
    }
    // The broken fragment fires for both the cross-file and same-file anchor.
    let frags = beta.iter().filter(|c| *c == "broken-link-fragment").count();
    assert_eq!(
        frags, 2,
        "both the cross-file and same-file bad anchors must fire: {beta:?}"
    );
}

#[test]
fn a_clean_skill_has_no_beta_findings() {
    let fx = Fixture::new();
    fx.write(
        "clean/SKILL.md",
        "---\nname: clean\ndescription: Split a PDF into one file per page when a user asks to break up a document.\n---\n\n# Clean\n\nSee the [guide](references/guide.md#overview).\n",
    );
    fx.write("clean/references/guide.md", "# Guide\n\n## Overview\n");
    // The guide is referenced from SKILL.md, the anchor resolves, the description is concrete.
    let beta = fx.lint_codes("clean", true);
    assert!(
        beta.is_empty(),
        "a well-formed skill must have no lint findings even under --beta: {beta:?}"
    );
    // Guard the helper itself against a silent miss: the clean skill really is discovered.
    let _ = Path::new(env!("CARGO_BIN_EXE_the"));
}

#[test]
fn skill_overlap_fires_through_the_cli_only_with_beta() {
    // Two skills with similar (not duplicate) descriptions: a routing-collision (TRIG2). Corpus-level,
    // so it is emitted as a stderr diagnostic, gated behind --beta like the other corpus rules.
    let fx = Fixture::new();
    fx.write(
        "alpha/SKILL.md",
        "---\nname: alpha\ndescription: crawl website extract structured profile data report json\n---\n\n# Alpha\n\nBody.\n",
    );
    fx.write(
        "bravo/SKILL.md",
        "---\nname: bravo\ndescription: crawl website extract structured profile output format markdown\n---\n\n# Bravo\n\nBody.\n",
    );
    assert!(
        !fx.lint_stderr(false).contains("skill-overlap"),
        "skill-overlap must be gated behind --beta"
    );
    assert!(
        fx.lint_stderr(true).contains("skill-overlap"),
        "skill-overlap must surface with --beta"
    );
}

#[test]
fn positional_path_lints_a_project_and_a_skills_root() {
    // A PROJECT layout: the skill lives under .claude/skills, NOT as a direct child of the root. A
    // positional project path must still discover it (the user should not have to name the subdir).
    let proj = Fixture::new();
    proj.write(
        ".claude/skills/demo/SKILL.md",
        "---\nname: demo\ndescription: Split a PDF into one file per page on request.\n---\n\n# Demo\n",
    );
    let names = proj.skill_names_positional(&proj.root.to_string_lossy());
    assert!(
        names.contains(&"demo".to_string()),
        "a positional PROJECT path must discover .claude/skills/demo: {names:?}"
    );

    // A SKILLS-root layout (skills are direct children): the same positional path still works.
    let root = Fixture::new();
    root.write(
        "alpha/SKILL.md",
        "---\nname: alpha\ndescription: Crawl a site and extract a structured profile.\n---\n\n# Alpha\n",
    );
    let names = root.skill_names_positional(&root.root.to_string_lossy());
    assert!(
        names.contains(&"alpha".to_string()),
        "a positional SKILLS-root path must discover direct-child skills: {names:?}"
    );
}

#[test]
fn claude_code_field_validity_fires_through_the_cli() {
    // The agent-field-validity family is default-stability, so it must surface WITHOUT --beta through
    // the real command path. This guards against a stale local binary shipping old field-check logic
    // (a recurring build-cache gotcha the unit tests cannot see).
    let fx = Fixture::new();
    fx.write(
        "fields/SKILL.md",
        "---\nname: fields\ndescription: a clear useful tool for splitting PDFs by page\nshell: fish\nbackground: true\ndisallowed-tools: 42\n---\n\n# Fields\n\nBody.\n",
    );
    let codes = fx.lint_codes("fields", false);
    for code in [
        "shell-invalid",
        "background-without-context",
        "list-field-invalid",
    ] {
        assert!(
            codes.contains(&code.to_string()),
            "{code} must surface through the CLI by default: {codes:?}"
        );
    }
}
