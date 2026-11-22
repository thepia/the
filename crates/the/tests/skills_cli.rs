//! End-to-end tests: run the built `the` binary against a throwaway HOME and working directory.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::Value;

/// Run a command, retrying on Linux `ETXTBSY` (errno 26). A binary copied moments earlier can briefly
/// report "text file busy" before the writing fd is fully released by the kernel; macOS does not
/// exhibit this, so the test is otherwise identical across platforms.
fn output_retrying_busy(cmd: &mut Command) -> Output {
    for _ in 0..50 {
        match cmd.output() {
            Ok(o) => return o,
            Err(e) if e.raw_os_error() == Some(26) => {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(e) => panic!("exec failed: {e}"),
        }
    }
    panic!("command still ETXTBSY after retries");
}

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
        self.run_env(args, &[])
    }

    fn run_env(&self, args: &[&str], vars: &[(&str, &str)]) -> Output {
        let mut c = Command::new(env!("CARGO_BIN_EXE_the"));
        c.args(args)
            .current_dir(self.work())
            .env("HOME", self.home())
            .env("PATH", self.bin());
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
    assert!(err.contains("help: did you mean `pdf-tools`?"), "{err}");
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

#[test]
fn alternate_command_names_the_thepia_and_th_all_work() {
    let sb = Sandbox::new();
    sb.skill(
        &sb.home().join(".claude/skills/pdf-tools"),
        &md("pdf-tools", "Work with PDFs."),
    );

    // 1. Direct Cargo binary targets (`the`, `thepia`, `th`).
    for bin in [
        env!("CARGO_BIN_EXE_the"),
        env!("CARGO_BIN_EXE_thepia"),
        env!("CARGO_BIN_EXE_th"),
    ] {
        let out = Command::new(bin).arg("--version").output().unwrap();
        assert!(out.status.success(), "{bin} failed: {}", stderr(&out));
        assert!(
            stdout(&out).starts_with("the "),
            "unexpected version output from {bin}: {}",
            stdout(&out)
        );

        let list_out = Command::new(bin)
            .args(["skills", "list"])
            .current_dir(sb.work())
            .env("HOME", sb.home())
            .env("PATH", sb.bin())
            .env("CLAUDE_CODE_DISABLE_POLICY_SKILLS", "1")
            .output()
            .unwrap();
        assert!(
            list_out.status.success(),
            "{bin} skills list failed: {}",
            stderr(&list_out)
        );
        assert!(
            stdout(&list_out).contains("pdf-tools"),
            "missing skill in {bin} output: {}",
            stdout(&list_out)
        );
    }

    // 2. Packaged symlink layout (`the -> thepia`, `th -> thepia` as used by Nix and Homebrew).
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let pkg_bin = sb.base.join("pkg-bin");
        fs::create_dir_all(&pkg_bin).unwrap();
        let installed_thepia = pkg_bin.join("thepia");
        fs::copy(env!("CARGO_BIN_EXE_thepia"), &installed_thepia).unwrap();
        symlink("thepia", pkg_bin.join("the")).unwrap();
        symlink("thepia", pkg_bin.join("th")).unwrap();

        for name in ["thepia", "the", "th"] {
            let exe = pkg_bin.join(name);
            let mut cmd = Command::new(&exe);
            cmd.args(["skills", "list"])
                .current_dir(sb.work())
                .env("HOME", sb.home())
                .env("PATH", sb.bin())
                .env("CLAUDE_CODE_DISABLE_POLICY_SKILLS", "1");
            let out = output_retrying_busy(&mut cmd);
            assert!(
                out.status.success(),
                "{name} symlink failed: {}",
                stderr(&out)
            );
            assert!(
                stdout(&out).contains("pdf-tools"),
                "{name} symlink output missing pdf-tools: {}",
                stdout(&out)
            );
        }
    }
}

#[test]
fn list_only_considers_installed_assistants_in_agents_column() {
    let sb = Sandbox::new();
    // Only claude and gemini are installed (neither opencode nor goose).
    sb.install_agents(&["claude", "gemini"]);

    sb.skill(
        &sb.home().join(".claude/skills/pdf-tools"),
        &md("pdf-tools", "Work with PDFs."),
    );
    sb.skill(
        &sb.work().join(".agents/skills/deploy"),
        &md("deploy", "Deploy this repo."),
    );
    sb.skill(
        &sb.work().join(".gemini/skills/gem-only"),
        &md("gem-only", "Gemini skill."),
    );
    // Goose-only directory should not be searched when goose is not installed.
    sb.skill(
        &sb.home().join(".config/goose/skills/recipes"),
        &md("recipes", "Goose recipes."),
    );

    let o = sb.run(&["skills", "list"]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert_eq!(
        stdout(&o),
        "\
NAME       SCOPE    AGENTS  LINKED  DESCRIPTION
deploy     project  gemini  no      Deploy this repo.
gem-only   project  gemini  no      Gemini skill.
pdf-tools  user     claude  no      Work with PDFs.
"
    );
    assert_eq!(stderr(&o), "");
}

#[test]
fn list_prose_form_renders_paragraph_of_skill_names() {
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

    // `the skills` (no subcommand) outputs the assistants found and the paragraph of all skills.
    let expected = "\
[assistants]
claude, opencode, goose

[all skills]
deploy, pdf-tools, recipes
";
    let top = sb.run(&["skills"]);
    assert!(top.status.success(), "{}", stderr(&top));
    assert_eq!(stdout(&top), expected);

    // `the skills list --prose` and `--form prose` produce the same output.
    let o = sb.run(&["skills", "list", "--prose"]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert_eq!(stdout(&o), expected);

    let o2 = sb.run(&["skills", "list", "--form", "prose"]);
    assert!(o2.status.success(), "{}", stderr(&o2));
    assert_eq!(stdout(&o2), expected);

    // `--by-scope` groups skills into multiple paragraphs with a heading for where/level they live.
    let by_scope = sb.run(&["skills", "--by-scope"]);
    assert!(by_scope.status.success(), "{}", stderr(&by_scope));
    assert_eq!(
        stdout(&by_scope),
        "\
project:
deploy

user:
pdf-tools, recipes
"
    );

    let list_by_scope = sb.run(&["skills", "list", "--by-scope"]);
    assert!(list_by_scope.status.success(), "{}", stderr(&list_by_scope));
    assert_eq!(stdout(&list_by_scope), stdout(&by_scope));
}

#[test]
fn prose_highlights_issues_with_bang_and_summarizes_counts_without_stderr_noise() {
    let sb = Sandbox::new();
    sb.skill(
        &sb.home().join(".claude/skills/clean"),
        &md("clean", "Clean skill."),
    );
    sb.skill(
        &sb.home().join(".claude/skills/bad-yaml"),
        "---\nname: bad-yaml\ndescription: unquoted: colon\n---\nbody\n",
    );
    sb.skill(
        &sb.home().join(".claude/skills/Upper"),
        &md("Upper", "Off-spec uppercase name."),
    );

    let o = sb.run(&["skills"]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert_eq!(
        stderr(&o),
        "",
        "prose overview summarizes issues in stdout instead of dumping per-skill stderr diagnostics"
    );
    assert_eq!(
        stdout(&o),
        "\
[assistants]
claude, opencode, goose

[all skills]
▲Upper, ▲bad-yaml, clean

3 skills (2 with issues):
  ▲ 1 invalid YAML frontmatter
  ▲ 1 invalid skill name format
"
    );

    // With forced color, error `!` is red and warning `!` is yellow.
    let colored = sb.run_env(&["skills"], &[("CLICOLOR_FORCE", "1")]);
    assert!(colored.status.success());
    assert!(stdout(&colored).contains('\u{1b}'));
}

#[test]
fn claude_code_identifies_skills_by_directory_and_flags_shared_command_names() {
    // Verified against Claude Code 2.1.258: both directories load, listed as `alpha-dir` and
    // `beta-dir`; `/shared-name` invokes only one of them.
    let sb = Sandbox::new();
    sb.skill(
        &sb.home().join(".claude/skills/alpha-dir"),
        &md("shared-name", "Alpha."),
    );
    sb.skill(
        &sb.home().join(".claude/skills/beta-dir"),
        &md("shared-name", "Beta."),
    );

    let o = sb.run(&["skills", "list", "--format", "json"]);
    let v = json(&o);
    let skills = v["skills"].as_array().unwrap();
    assert_eq!(skills.len(), 2);
    for s in skills {
        assert_eq!(s["status"]["claude"]["status"], "loaded", "{s}");
    }
    let claude_names: Vec<&str> = skills
        .iter()
        .map(|s| s["status"]["claude"]["name"].as_str().unwrap())
        .collect();
    assert_eq!(claude_names.len(), 2);
    assert!(claude_names.contains(&"alpha-dir") && claude_names.contains(&"beta-dir"));
    // Goose and OpenCode do key by frontmatter `name`.
    let goose: Vec<&str> = skills
        .iter()
        .map(|s| s["status"]["goose"]["status"].as_str().unwrap())
        .collect();
    assert!(
        goose.contains(&"loaded") && goose.contains(&"shadowed"),
        "{goose:?}"
    );

    let collisions: Vec<Value> = diags(&o)
        .into_iter()
        .filter(|d| d["code"] == "claude-alias-collision")
        .collect();
    assert_eq!(collisions.len(), 1, "one warning per shared name");
    assert_eq!(collisions[0]["level"], "warning");
    assert_eq!(collisions[0]["context"].as_array().unwrap().len(), 2);
}

#[test]
fn documented_claude_code_frontmatter_keys_are_not_unknown() {
    let sb = Sandbox::new();
    sb.skill(
        &sb.home().join(".claude/skills/modern"),
        "---\nname: modern\ndescription: Uses 2026 Claude Code fields.\neffort: xhigh\narguments: issue\ndisallowed-tools: Write\nbackground: false\npaths: \"src/**\"\nshell: bash\nwhen_to_use: When testing.\n---\nBody.\n",
    );
    let o = sb.run(&["skills", "lint", "modern", "--format", "json"]);
    let unknown: Vec<Value> = diags(&o)
        .into_iter()
        .filter(|d| {
            d.to_string().contains("unknown-frontmatter-key")
                || d.to_string().contains("unknown frontmatter key")
        })
        .collect();
    assert!(unknown.is_empty(), "{unknown:?}");
}

#[test]
fn beta_corpus_rules_are_gated_behind_the_flag() {
    let sb = Sandbox::new();
    // Two near-duplicate skills (descriptions differ by one word) -> a `redundant-with` candidate.
    sb.skill(
        &sb.home().join(".claude/skills/crawl-one"),
        &md(
            "crawl-one",
            "Crawl a website directory and extract the structured profile json data for each scraped page",
        ),
    );
    sb.skill(
        &sb.home().join(".claude/skills/crawl-two"),
        &md(
            "crawl-two",
            "Crawl a website directory and extract the structured profile json data for every scraped page",
        ),
    );

    // Default: the corpus (beta) rule is staged behind `--beta`, so it is NOT reported.
    let base = diags(&sb.run(&["--format", "json", "skills", "lint"]));
    assert!(
        !base.iter().any(|d| d["code"] == "redundant-with"),
        "redundant-with must be hidden without --beta: {base:?}"
    );

    // `--beta`: the corpus rule fires as a warning naming the counterpart skill.
    let beta = diags(&sb.run(&["--format", "json", "skills", "lint", "--beta"]));
    let red = beta
        .iter()
        .find(|d| d["code"] == "redundant-with")
        .unwrap_or_else(|| panic!("redundant-with must appear with --beta: {beta:?}"));
    assert_eq!(red["level"], "warning", "{red:?}");
}
