//! End-to-end tests: run the built `the` binary against a throwaway HOME and working directory.

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
            "the-it-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("home")).unwrap();
        // A repository root, so the project walks (Claude Code, OpenCode) stop here.
        fs::create_dir_all(base.join("work/.git")).unwrap();
        Self {
            base: fs::canonicalize(base).unwrap(),
        }
    }

    fn home(&self) -> PathBuf {
        self.base.join("home")
    }

    fn work(&self) -> PathBuf {
        self.base.join("work")
    }

    fn skill(&self, dir: &Path, skill_md: &str) {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join("SKILL.md"), skill_md).unwrap();
    }

    fn run(&self, args: &[&str]) -> Output {
        self.run_env(args, &[])
    }

    fn run_env(&self, args: &[&str], vars: &[(&str, &str)]) -> Output {
        let mut c = Command::new(env!("CARGO_BIN_EXE_the"));
        c.args(args)
            .current_dir(self.work())
            .env("HOME", self.home());
        // Pin every variable that changes the search plan or color; skip the host's managed Claude
        // dir. stdout/stderr are pipes here, so output is the plain (non-terminal) rendering.
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
        c.envs(vars.iter().copied());
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

fn md(name: &str, description: &str) -> String {
    format!("---\nname: {name}\ndescription: {description}\n---\n\n# {name}\n\nInstructions.\n")
}

#[test]
fn list_shows_which_agents_load_each_skill() {
    let sb = Sandbox::new();
    sb.skill(
        &sb.home().join(".claude/skills/pdf-tools"),
        &md("pdf-tools", "Work with PDFs."),
    );
    sb.skill(
        &sb.work().join(".agents/skills/deploy"),
        &md("deploy", "Deploy this repo."),
    );
    sb.skill(
        &sb.home().join(".config/goose/skills/recipes"),
        &md("recipes", "Goose recipes."),
    );
    // Nested below a skills dir: OpenCode and Goose scan recursively, Claude Code one level only.
    sb.skill(
        &sb.home().join(".claude/skills/group/nested"),
        &md("nested", "Nested."),
    );

    let o = sb.run(&["skills", "list"]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert_eq!(
        stdout(&o),
        "\
NAME       SCOPE    AGENTS                   LINKED  DESCRIPTION
deploy     project  opencode, goose          no      Deploy this repo.
nested     user     opencode, goose          no      Nested.
pdf-tools  user     claude, opencode, goose  no      Work with PDFs.
recipes    user     goose                    no      Goose recipes.
"
    );
    assert_eq!(stderr(&o), "");

    let o = sb.run(&["skills", "list", "--agent", "claude"]);
    assert!(
        stdout(&o)
            .lines()
            .skip(1)
            .all(|l| l.starts_with("pdf-tools")),
        "{}",
        stdout(&o)
    );
}

#[test]
fn list_json_carries_status_locations_and_searched_dirs() {
    let sb = Sandbox::new();
    sb.skill(
        &sb.home().join(".claude/skills/pdf-tools"),
        &md("pdf-tools", "Work with PDFs."),
    );

    let o = sb.run(&["skills", "list", "--format", "json"]);
    assert!(o.status.success());
    let v = json(&o);
    let s = &v["skills"][0];
    assert_eq!(s["name"], "pdf-tools");
    assert_eq!(
        s["agents"],
        serde_json::json!(["claude", "opencode", "goose"])
    );
    assert_eq!(s["status"]["goose"]["status"], "loaded");
    assert_eq!(s["linked"], false);
    let loc = &s["locations"][0];
    assert_eq!(
        loc["agents"],
        serde_json::json!(["claude", "opencode", "goose"])
    );
    assert_eq!(loc["link"], Value::Null);
    assert!(v["searched"].as_array().unwrap().iter().any(|r| {
        r["path"].as_str().unwrap().ends_with("home/.claude/skills") && r["exists"] == true
    }));
}

#[test]
fn empty_list_prints_nothing_and_notes_on_stderr() {
    let sb = Sandbox::new();
    let o = sb.run(&["skills", "list"]);
    assert!(o.status.success());
    assert_eq!(stdout(&o), "");
    assert!(
        stderr(&o).starts_with("note[no-skills]: no skills found\n"),
        "{}",
        stderr(&o)
    );
}

#[test]
fn agent_breaking_problems_warn_and_lint_is_a_note() {
    let sb = Sandbox::new();
    sb.skill(
        &sb.home().join(".claude/skills/colon"),
        "---\nname: colon\ndescription: rule: x\n---\nbody\n",
    );
    sb.skill(
        &sb.home().join(".claude/skills/Upper"),
        &md("Upper", "Valid YAML, off-spec name."),
    );

    let o = sb.run(&["skills", "list", "--format", "json"]);
    let v = json(&o);
    let colon = v["skills"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "colon")
        .unwrap();
    assert_eq!(
        colon["agents"],
        serde_json::json!(["claude"]),
        "Claude Code loads invalid YAML by directory name"
    );
    assert_eq!(
        colon["status"]["goose"]["problem"]["code"],
        "frontmatter-invalid"
    );
    assert_eq!(colon["status"]["opencode"]["status"], "skipped");

    let d = diags(&o);
    assert_eq!(d.len(), 2, "{d:?}");
    let skipped = d.iter().find(|d| d["code"] == "skill-skipped").unwrap();
    assert_eq!(skipped["level"], "warning");
    assert_eq!(skipped["title"], "opencode and goose skip skill `colon`");
    assert!(
        skipped["cause"]
            .as_str()
            .unwrap()
            .contains("SKILL.md line 3")
    );
    let lint = d.iter().find(|d| d["code"] == "skill-lint").unwrap();
    assert_eq!(lint["level"], "note");
    assert_eq!(
        lint["title"],
        "skill `Upper` does not follow the Agent Skills format"
    );
}

#[cfg(unix)]
#[test]
fn skills_shared_between_agents_by_symlink_are_one_entry_and_flagged() {
    use std::os::unix::fs::symlink;
    let sb = Sandbox::new();
    // One skill linked into a project's .claude/skills from a shared checkout.
    let shared = sb.base.join("shared/review");
    sb.skill(&shared, &md("review", "Review code."));
    fs::create_dir_all(sb.work().join(".claude/skills")).unwrap();
    symlink(&shared, sb.work().join(".claude/skills/review")).unwrap();
    // A whole skills dir linked: ~/.agents/skills -> ~/.claude/skills.
    sb.skill(
        &sb.home().join(".claude/skills/pdf-tools"),
        &md("pdf-tools", "Work with PDFs."),
    );
    fs::create_dir_all(sb.home().join(".agents")).unwrap();
    symlink(
        sb.home().join(".claude/skills"),
        sb.home().join(".agents/skills"),
    )
    .unwrap();

    let o = sb.run(&["skills", "list"]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert_eq!(
        stdout(&o),
        "\
NAME       SCOPE    AGENTS                   LINKED  DESCRIPTION
pdf-tools  user     claude, opencode, goose  yes     Work with PDFs.
review     project  claude, opencode, goose  yes     Review code.
"
    );

    let o = sb.run(&["skills", "show", "pdf-tools", "--format", "json"]);
    let v = json(&o);
    let locs = v["locations"].as_array().unwrap();
    assert_eq!(locs.len(), 2, "{locs:?}");
    assert_eq!(locs[0]["links_to"], Value::Null);
    let linked = locs
        .iter()
        .find(|l| l["path"].as_str().unwrap().contains(".agents"))
        .unwrap();
    assert_eq!(
        linked["links_to"],
        sb.home().join(".claude/skills").display().to_string()
    );
    assert_eq!(linked["agents"], serde_json::json!(["opencode", "goose"]));

    let o = sb.run(&["skills", "dirs", "--format", "json"]);
    let dirs = json(&o)["dirs"].as_array().unwrap().clone();
    let agents_dir = dirs
        .iter()
        .find(|d| d["path"] == sb.home().join(".agents/skills").display().to_string())
        .unwrap();
    assert_eq!(
        agents_dir["links_to"],
        sb.home().join(".claude/skills").display().to_string()
    );
}

#[test]
fn same_name_resolves_per_agent_precedence() {
    let sb = Sandbox::new();
    sb.skill(
        &sb.home().join(".claude/skills/deploy"),
        &md("deploy", "user"),
    );
    sb.skill(
        &sb.work().join(".claude/skills/deploy"),
        &md("deploy", "project"),
    );

    let o = sb.run(&["skills", "list", "--format", "json"]);
    let v = json(&o);
    let by_scope = |scope: &str| {
        v["skills"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["scope"] == scope)
            .unwrap()
            .clone()
    };
    let (user, project) = (by_scope("user"), by_scope("project"));
    // Claude Code: personal beats project. Goose: first found, and it searches the project first.
    assert_eq!(user["status"]["claude"]["status"], "loaded");
    assert_eq!(project["status"]["claude"]["status"], "shadowed");
    assert_eq!(project["status"]["goose"]["status"], "loaded");
    assert_eq!(user["status"]["goose"]["status"], "shadowed");
    // OpenCode: last parsed wins, concurrently: unpredictable.
    assert_eq!(user["status"]["opencode"]["status"], "conflict");
    assert_eq!(project["status"]["opencode"]["status"], "conflict");
    let conflicts: Vec<Value> = diags(&o)
        .into_iter()
        .filter(|d| d["code"] == "skill-conflict")
        .collect();
    assert_eq!(conflicts.len(), 1);

    let o = sb.run(&["skills", "show", "deploy"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(
        stderr(&o).contains("error[skill-ambiguous]: 2 skills are named `deploy`"),
        "{}",
        stderr(&o)
    );

    let o = sb.run(&[
        "skills", "show", "deploy", "--agent", "claude", "--scope", "user",
    ]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(stdout(&o).contains("description: user\n"));
    assert!(stdout(&o).contains("claude    loaded"), "{}", stdout(&o));
}

#[test]
fn show_prints_record_agents_locations_and_body() {
    let sb = Sandbox::new();
    let dir = sb.home().join(".claude/skills/pdf-tools");
    sb.skill(&dir, "---\nname: pdf-tools\ndescription: Work with PDFs.\nlicense: MIT\n---\n\n# PDF\n\nUse scripts/fill.py.\n");
    fs::create_dir_all(dir.join("scripts")).unwrap();
    fs::write(dir.join("scripts/fill.py"), "").unwrap();

    let o = sb.run(&["skills", "show", "pdf-tools"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let path = dir.join("SKILL.md");
    assert_eq!(
        stdout(&o),
        format!(
            "\
name:        pdf-tools
description: Work with PDFs.
scope:       user
path:        {p}
license:     MIT
files:       scripts/fill.py

AGENT     STATUS  DETAIL
claude    loaded  -
opencode  loaded  -
goose     loaded  -

LOCATION  AGENTS                   LINKS TO
{p}  claude, opencode, goose  -

# PDF

Use scripts/fill.py.
",
            p = path.display()
        )
        .replace(
            "LOCATION  AGENTS",
            &format!(
                "{:w$}  AGENTS",
                "LOCATION",
                w = path.display().to_string().len()
            )
        )
    );

    let v = json(&sb.run(&["skills", "show", "pdf-tools", "--format", "json"]));
    assert_eq!(v["skill"]["frontmatter"]["license"], "MIT");
    assert_eq!(v["skill"]["files"][0], "scripts/fill.py");
    assert_eq!(v["agents"][0]["agent"], "claude");
    assert_eq!(v["body"], "# PDF\n\nUse scripts/fill.py.\n");
}

#[test]
fn show_unknown_skill_exits_not_found_with_a_suggestion() {
    let sb = Sandbox::new();
    sb.skill(
        &sb.home().join(".claude/skills/pdf-tools"),
        &md("pdf-tools", "x"),
    );

    let o = sb.run(&["skills", "show", "pdf"]);
    assert_eq!(o.status.code(), Some(3));
    assert_eq!(stdout(&o), "");
    let err = stderr(&o);
    assert!(
        err.starts_with("error[skill-not-found]: no skill named `pdf`\n"),
        "{err}"
    );
    assert!(err.contains("= help: did you mean `pdf-tools`?"), "{err}");
}

#[test]
fn dir_and_scope_filters() {
    let sb = Sandbox::new();
    let extra = sb.base.join("extra");
    sb.skill(&sb.home().join(".claude/skills/a"), &md("a", "A."));
    sb.skill(&extra.join("b"), &md("b", "B."));

    let o = sb.run(&[
        "skills",
        "list",
        "--scope",
        "dir",
        "--dir",
        extra.to_str().unwrap(),
    ]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert_eq!(
        stdout(&o),
        "NAME  SCOPE  AGENTS  LINKED  DESCRIPTION\nb     dir    -       no      B.\n"
    );

    let o = sb.run(&["skills", "dirs", "--agent", "goose", "--format", "json"]);
    assert_eq!(json(&o)["dirs"].as_array().unwrap().len(), 7);
}

#[test]
fn piped_tables_are_aligned_plain_and_untruncated() {
    let sb = Sandbox::new();
    let long = "Fill, merge and extract PDF forms, then check every field against the source document before signing.";
    sb.skill(
        &sb.home().join(".claude/skills/pdf-tools"),
        &md("pdf-tools", long),
    );

    let o = sb.run(&["skills", "list"]);
    let out = stdout(&o);
    assert!(!out.contains('\u{1b}'), "no color on a pipe: {out:?}");
    assert!(!out.contains('─'), "no rule on a pipe: {out:?}");
    assert!(out.contains(long), "no truncation on a pipe: {out:?}");
    assert!(out.starts_with("NAME       SCOPE  AGENTS"), "{out}");
}

#[test]
fn no_header_omits_the_header_row() {
    let sb = Sandbox::new();
    sb.skill(&sb.home().join(".claude/skills/a"), &md("a", "A."));
    sb.skill(&sb.home().join(".claude/skills/b"), &md("b", "B."));

    let o = sb.run(&["skills", "list", "--no-header"]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert_eq!(
        stdout(&o),
        "a  user  claude, opencode, goose  no  A.\nb  user  claude, opencode, goose  no  B.\n"
    );
    // Global, so it also works before the subcommand, and JSON is unaffected.
    let o = sb.run(&["--no-header", "skills", "list", "--format", "json"]);
    assert_eq!(json(&o)["skills"].as_array().unwrap().len(), 2);
}

#[test]
fn color_follows_clicolor_force_and_no_color() {
    let sb = Sandbox::new();
    sb.skill(&sb.home().join(".claude/skills/a"), &md("a", "A."));
    sb.skill(
        &sb.home().join(".claude/skills/Bad"),
        &md("Bad", "Off-spec name."),
    );

    let forced = sb.run_env(&["skills", "list"], &[("CLICOLOR_FORCE", "1")]);
    assert!(stdout(&forced).contains('\u{1b}'), "forced color on a pipe");
    assert!(
        stderr(&forced).contains('\u{1b}'),
        "diagnostics colored too"
    );
    assert!(
        !stdout(&forced).contains('─'),
        "the rule is for terminals only"
    );

    let json_forced = sb.run_env(
        &["skills", "list", "--format", "json"],
        &[("CLICOLOR_FORCE", "1")],
    );
    assert!(
        !stdout(&json_forced).contains('\u{1b}') && !stderr(&json_forced).contains('\u{1b}'),
        "JSON is never colored"
    );

    let off = sb.run_env(&["skills", "list"], &[("NO_COLOR", "1")]);
    assert!(!stdout(&off).contains('\u{1b}') && !stderr(&off).contains('\u{1b}'));
}
