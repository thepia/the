//! End-to-end CLI integration tests for `the skills lint` (human and `--output json`).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::Value;

struct Sandbox {
    base: PathBuf,
}

impl Sandbox {
    fn new() -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let base = std::env::temp_dir().join(format!(
            "the-lint-it-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("home")).unwrap();
        fs::create_dir_all(base.join("work/.git")).unwrap();
        let sb = Self {
            base: fs::canonicalize(base).unwrap(),
        };
        sb.install_agents(&["claude", "opencode", "goose"]);
        sb
    }

    fn home(&self) -> PathBuf {
        self.base.join("home")
    }

    fn work(&self) -> PathBuf {
        self.base.join("work")
    }

    fn bin(&self) -> PathBuf {
        self.base.join("bin")
    }

    fn install_agents(&self, names: &[&str]) {
        let _ = fs::remove_dir_all(self.bin());
        fs::create_dir_all(self.bin()).unwrap();
        for name in names {
            let p = self.bin().join(name);
            fs::write(&p, "#!/bin/sh\necho 1.0.0\n").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = fs::metadata(&p).unwrap().permissions();
                perms.set_mode(0o755);
                fs::set_permissions(&p, perms).unwrap();
            }
        }
    }

    fn skill(&self, dir: &Path, skill_md: &str) {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join("SKILL.md"), skill_md).unwrap();
    }

    fn run(&self, args: &[&str]) -> Output {
        let mut c = Command::new(env!("CARGO_BIN_EXE_the"));
        c.args(args)
            .current_dir(self.work())
            .env("HOME", self.home())
            .env("PATH", self.bin());
        for k in [
            "XDG_CONFIG_HOME",
            "GOOSE_PATH_ROOT",
            "OPENCODE_CONFIG_DIR",
            "OPENCODE_DISABLE_EXTERNAL_SKILLS",
            "OPENCODE_DISABLE_CLAUDE_CODE",
            "OPENCODE_DISABLE_CLAUDE_CODE_SKILLS",
            "OPENCODE_DISABLE_PROJECT_CONFIG",
            "CLICOLOR_FORCE",
            "FORCE_COLOR",
            "NO_COLOR",
            "CLICOLOR",
        ] {
            c.env_remove(k);
        }
        c.env("CLAUDE_CODE_DISABLE_POLICY_SKILLS", "1");
        c.output().unwrap()
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

fn stdout(o: &Output) -> String {
    String::from_utf8(o.stdout.clone()).unwrap()
}

fn stderr(o: &Output) -> String {
    String::from_utf8(o.stderr.clone()).unwrap()
}

fn json(o: &Output) -> Value {
    serde_json::from_str(&stdout(o)).unwrap_or_else(|e| panic!("{e}: {}", stdout(o)))
}

fn diags(o: &Output) -> Vec<Value> {
    stderr(o)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn skills_lint_exits_zero_when_clean_or_only_spec_notes() {
    let sb = Sandbox::new();
    let clean_dir = sb.home().join(".claude/skills/clean-skill");
    sb.skill(
        &clean_dir,
        "---\nname: clean-skill\ndescription: Clean skill.\nallowed-tools: Read Bash(git:*)\n---\n\nSee [guide](references/guide.md).\n",
    );
    fs::create_dir_all(clean_dir.join("references")).unwrap();
    fs::write(clean_dir.join("references/guide.md"), "# Guide\n").unwrap();

    // Clean skill exits 0 with no stderr diagnostics.
    let o = sb.run(&["skills", "lint"]);
    assert_eq!(o.status.code(), Some(0), "stderr: {}", stderr(&o));
    assert_eq!(stderr(&o), "");
    assert_eq!(
        stdout(&o),
        "\
NAME         SCOPE  STATUS  AGENTS                   ISSUES
clean-skill  user   ok      claude, opencode, goose  -
"
    );

    // Add a skill with only an advisory spec lint (`unknown-frontmatter-key`); still exits 0.
    sb.skill(
        &sb.home().join(".claude/skills/spec-note"),
        "---\nname: spec-note\ndescription: Advisory only.\ncustom-key: true\n---\n\nBody.\n",
    );
    let o_note = sb.run(&["skills", "lint"]);
    assert_eq!(o_note.status.code(), Some(0), "stderr: {}", stderr(&o_note));
    assert!(
        stderr(&o_note).contains(
            "note[skill-lint]: skill `spec-note` does not follow the Agent Skills format"
        ),
        "stderr: {}",
        stderr(&o_note)
    );
    assert!(
        stdout(&o_note).contains(
            "spec-note    user   note    claude, opencode, goose  unknown-frontmatter-key"
        ),
        "stdout: {}",
        stdout(&o_note)
    );
}

#[test]
fn skills_lint_reports_errors_and_warnings_in_human_and_json_and_exits_one() {
    let sb = Sandbox::new();

    // 1. Skill with broken link and unrestricted bash (warnings).
    sb.skill(
        &sb.home().join(".claude/skills/broken-asset"),
        "---\nname: broken-asset\ndescription: Has missing link and wildcard bash.\nallowed-tools: Bash(*)\n---\n\nRead [missing](references/missing.md) and [escape](../outside.md).\n",
    );

    // 2. Skill with trojan source and exposed secret (errors).
    sb.skill(
        &sb.home().join(".claude/skills/leaked-skill"),
        "---\nname: leaked-skill\ndescription: Has secret and hidden bidi.\n---\n\nKey: sk-ant-api03-9kLmN2pQrS4tUvWxYz87654321 and \u{202E}hidden\n",
    );

    // Human output: exits 1, emits error/warning diagnostics on stderr, prints table on stdout.
    let human = sb.run(&["skills", "lint"]);
    assert_eq!(human.status.code(), Some(1));
    let err = stderr(&human);
    assert!(err.contains("warning[broken-link]:"), "{err}");
    assert!(err.contains("warning[link-escapes-skill]:"), "{err}");
    assert!(err.contains("warning[unrestricted-bash]:"), "{err}");
    assert!(err.contains("error[trojan-source]:"), "{err}");
    assert!(err.contains("error[exposed-secret]:"), "{err}");

    let out = stdout(&human);
    assert!(
        out.contains("broken-asset  user   warning"),
        "unexpected human stdout:\n{out}"
    );
    assert!(
        out.contains("leaked-skill  user   error"),
        "unexpected human stdout:\n{out}"
    );

    // JSON output via `--output json` (and single-skill filter):
    let json_out = sb.run(&["skills", "lint", "leaked-skill", "--output", "json"]);
    assert_eq!(json_out.status.code(), Some(1));
    let v = json(&json_out);
    let skills = v["skills"].as_array().unwrap();
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0]["name"], "leaked-skill");
    assert_eq!(skills[0]["status"], "error");

    let report_diags = v["diagnostics"].as_array().unwrap();
    let codes: Vec<&str> = report_diags
        .iter()
        .map(|d| d["code"].as_str().unwrap())
        .collect();
    assert_eq!(codes, ["trojan-source", "exposed-secret"]);

    let stderr_diags = diags(&json_out);
    assert_eq!(stderr_diags.len(), 2);
    assert_eq!(stderr_diags[0]["level"], "error");
    assert_eq!(stderr_diags[0]["code"], "trojan-source");
    assert_eq!(stderr_diags[1]["level"], "error");
    assert_eq!(stderr_diags[1]["code"], "exposed-secret");
}

#[test]
fn skills_lint_advice_classes_each_issue_and_never_fabricates() {
    let sb = Sandbox::new();
    let root = sb.home().join(".claude/skills");
    // name: PDF in directory `pdf` → name-format (auto: pure capitalisation) + name-mismatch (assisted).
    sb.skill(
        &root.join("pdf"),
        "---\nname: PDF\ndescription: Extract tables from PDF files. Use when working with PDFs.\n---\n\nBody.\n",
    );
    // Over-long description → manual (a description is never fabricated or auto-truncated).
    let long = "x".repeat(1100);
    sb.skill(
        &root.join("docs"),
        &format!("---\nname: docs\ndescription: {long}\n---\n\nBody.\n"),
    );
    // Unrestricted Bash grant → manual (a security finding).
    sb.skill(
        &root.join("runner"),
        "---\nname: runner\ndescription: Runs commands.\nallowed-tools: Bash(*)\n---\n\nBody.\n",
    );

    // JSON: the `advice` array carries a structured object per issue a coding agent can branch on.
    let o = sb.run(&["skills", "lint", "--advice", "--format", "json"]);
    assert_eq!(o.status.code(), Some(1), "stderr: {}", stderr(&o));
    let v = json(&o);
    let advice = v["advice"].as_array().expect("advice array present");
    let find = |name: &str, code: &str| {
        advice
            .iter()
            .find(|a| a["name"] == name && a["code"] == code)
            .unwrap_or_else(|| panic!("no advice for {name}/{code} in {advice:#?}"))
    };

    let pdf_fmt = find("PDF", "name-format");
    assert_eq!(pdf_fmt["class"], "auto");
    assert_eq!(pdf_fmt["safe"], true);
    assert_eq!(pdf_fmt["command"], Value::Null, "no fabricated command yet");
    assert!(
        pdf_fmt["strategy"].as_str().unwrap().contains("`pdf`"),
        "auto strategy names the lowercased value: {pdf_fmt:#?}"
    );

    let pdf_mismatch = find("PDF", "name-mismatch");
    assert_eq!(pdf_mismatch["class"], "assisted");
    assert_eq!(pdf_mismatch["safe"], false);

    // No lossless mechanical fix exists for these, so they are manual, never auto.
    for (name, code) in [
        ("docs", "description-too-long"),
        ("runner", "unrestricted-bash"),
    ] {
        let a = find(name, code);
        assert_eq!(a["class"], "manual", "{name}/{code} must be manual");
        assert_eq!(a["safe"], false);
        // The description advice recommends shortening, never inventing content.
        if code == "description-too-long" {
            assert!(a["strategy"].as_str().unwrap().contains("shorten"));
        }
    }

    // Human output adds the advice table (sorted safest-first) after the overview table.
    let human = sb.run(&["skills", "lint", "--advice"]);
    let out = stdout(&human);
    let has_cols = |line: &str, cols: &[&str]| cols.iter().all(|c| line.contains(c));
    assert!(
        out.lines().any(|l| has_cols(l, &["CLASS", "CODE", "FIX"])),
        "advice header missing:\n{out}"
    );
    assert!(
        out.lines()
            .any(|l| has_cols(l, &["PDF", "auto", "name-format", "`pdf`"])),
        "auto row for PDF missing:\n{out}"
    );
    let auto_at = out.find("auto").unwrap();
    let manual_at = out.find("manual").unwrap();
    assert!(
        auto_at < manual_at,
        "advice must sort auto before manual:\n{out}"
    );

    // Without --advice there is no advice section at all.
    let plain = sb.run(&["skills", "lint"]);
    assert!(
        !stdout(&plain).contains("CLASS"),
        "no advice without the flag:\n{}",
        stdout(&plain)
    );
}
