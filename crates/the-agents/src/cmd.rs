//! `the agents list | show | lint`. Each builds a [`Report`]; rendering is format-independent. The
//! command group is a routable [`Target`]: the pipeline parses the grammar [`crate::cli`] contributes,
//! routes to [`Agents`], runs [`Target::execute`], and renders the returned [`AgentsResults`].

use std::io::{self, Write};
use std::path::PathBuf;

use clap::{Args, Command, FromArgMatches};
use serde_json::json;
use the_output::command::{
    CommandDescriptor, Ctx, Form, Invocation, Results, Target, Tier, is_path_like,
};
use the_output::diag::{Diagnostic, Exit, Level, summarize};
use the_output::output::Format;
use the_output::style::Term;
use the_output::table::{Block, Cell, Column, Human, Record, Report, Table, Tone};

use crate::cli::{AgentsArgs, AgentsCmd, SearchArgs};
use crate::discover::{self, Root};
use crate::lint::{self, Finding};
use crate::{AgentDef, Scope};

use the_skills::locate::Env;

const DESCRIPTION_WIDTH: usize = 72;

/// Cap on urgent issues shown after the lint table before the summary suggests `--all`.
const ISSUE_LIMIT: usize = 5;

/// The `agents` command group as a routable [`Target`]. Registered into the pipeline's registry.
pub struct Agents;

/// The commands this target owns, for help / governance and the registry drift test. Summaries follow
/// the style guide (lowercase, imperative, no trailing period, <= 60 chars).
const DESCRIPTORS: &[CommandDescriptor] = &[
    CommandDescriptor {
        path: "agents",
        group: "agents",
        tier: Tier::Core,
        summary: "inspect and lint local agent definitions",
    },
    CommandDescriptor {
        path: "agents list",
        group: "agents",
        tier: Tier::Core,
        summary: "list agent definitions, their model and tools",
    },
    CommandDescriptor {
        path: "agents show",
        group: "agents",
        tier: Tier::Aux,
        summary: "show one agent definition's metadata and body",
    },
    CommandDescriptor {
        path: "agents lint",
        group: "agents",
        tier: Tier::Core,
        summary: "lint definitions for loadability, delegation, fields",
    },
];

impl Target for Agents {
    fn id(&self) -> &'static str {
        "agents"
    }

    fn descriptors(&self) -> &'static [CommandDescriptor] {
        DESCRIPTORS
    }

    fn augment(&self, root: Command) -> Command {
        root.subcommand(
            AgentsArgs::augment_args(Command::new("agents"))
                .about("Agent/subagent definitions (`.claude/agents/*.md`) inspected and linted."),
        )
    }

    fn execute(&self, inv: &Invocation, ctx: &Ctx) -> Result<Box<dyn Results>, Diagnostic> {
        let args = AgentsArgs::from_arg_matches(&inv.matches).map_err(|e| {
            Diagnostic::error("agents-args", "could not parse `agents` arguments")
                .cause(e.to_string())
                .exit(Exit::Usage)
        })?;
        // Build the report with the Human matching the resolved stdout form (prose blocks bake color at
        // build time). The pipeline renders exactly once, with `ctx.form()`.
        let colored = matches!(ctx.form(), Form::Ansi);
        let h = Human {
            term: Term {
                color: colored,
                tty: ctx.stdout().tty,
            },
            header: !ctx.no_header(),
        };
        let built = build_agents(args, h)?;
        Ok(Box::new(AgentsResults {
            built,
            h,
            err_term: Term {
                color: ctx.stderr().color,
                tty: ctx.stderr().tty,
            },
        }))
    }
}

/// Route a parsed `agents` invocation to its subcommand builder.
fn build_agents(args: AgentsArgs, h: Human) -> Result<Built, Diagnostic> {
    // A leading path given in place of a subcommand (`the agents ..`) scans it like the bare `agents`,
    // additively. A non-path token here is a typo'd subcommand, not a directory.
    if let Some(path) = &args.path {
        let token = path.to_string_lossy();
        if !is_path_like(&token) {
            return Err(unknown_subcommand(&token));
        }
        let search = with_root(&args.search, path.clone());
        return build_list(&search, h, args.beta);
    }
    match args.command {
        Some(AgentsCmd::List { path }) => {
            let search = with_positional(&args.search, path);
            build_list(&search, h, args.beta)
        }
        Some(AgentsCmd::Show { name }) => build_show(&name, &args.search, args.beta),
        Some(AgentsCmd::Lint { target, all }) => {
            build_lint(target.as_deref(), &args.search, all, args.beta)
        }
        None => build_list(&args.search, h, args.beta),
    }
}

/// A token in the subcommand slot that is neither a known subcommand nor a path.
fn unknown_subcommand(name: &str) -> Diagnostic {
    Diagnostic::error(
        "unknown-subcommand",
        format!("unrecognized subcommand `{name}`"),
    )
    .cause("not a known `agents` subcommand, and not a path")
    .help("run `the agents --help`, or pass a path like `the agents .` to scan a directory")
    .exit(Exit::Usage)
}

/// `search` with `root` added as a `--dir` scan root (additive, exactly like `--dir`).
fn with_root(search: &SearchArgs, root: PathBuf) -> SearchArgs {
    let mut s = search.clone();
    s.dirs.push(root);
    s
}

fn with_positional(search: &SearchArgs, path: Option<PathBuf>) -> SearchArgs {
    match path {
        Some(p) => with_root(search, p),
        None => search.clone(),
    }
}

/// How a subcommand's issue diagnostics are presented on stderr.
#[derive(Clone, Copy)]
enum DiagMode {
    /// Emit each diagnostic in turn (list, show).
    Stream,
    /// The capped, rustc-style summary with a `--all` hint (lint).
    Summarize { all: bool },
}

/// A built-but-unrendered agents result: the stdout report, the stderr notices (always streamed) and
/// findings (presented per `mode`), and the exit code.
struct Built {
    report: Report,
    notices: Vec<Diagnostic>,
    findings: Vec<Diagnostic>,
    mode: DiagMode,
    exit: u8,
}

/// The agents command result. Holds the report already built for the resolved form plus the stream
/// state, so the pipeline renders to stdout/stderr without re-running any logic.
pub struct AgentsResults {
    built: Built,
    h: Human,
    err_term: Term,
}

impl Results for AgentsResults {
    fn render(&self, out: &mut dyn Write, form: Form) -> io::Result<()> {
        out.write_all(self.built.report.render(form.format(), self.h).as_bytes())
    }

    fn render_diagnostics(&self, err: &mut dyn Write, format: Format) -> io::Result<()> {
        for d in &self.built.notices {
            err.write_all(d.render(format, self.err_term).as_bytes())?;
        }
        match (self.built.mode, format) {
            (_, Format::Json) => {
                for d in &self.built.findings {
                    err.write_all(d.render(Format::Json, self.err_term).as_bytes())?;
                }
            }
            (DiagMode::Stream, Format::Human) => {
                for d in &self.built.findings {
                    err.write_all(d.render(Format::Human, self.err_term).as_bytes())?;
                }
            }
            (DiagMode::Summarize { all }, Format::Human) => {
                if !self.built.findings.is_empty() {
                    let limit = if all { None } else { Some(ISSUE_LIMIT) };
                    let text = summarize(&self.built.findings, limit)
                        .render(self.err_term, Some("the agents lint --all"));
                    err.write_all(text.as_bytes())?;
                }
            }
        }
        Ok(())
    }

    fn diagnostics(&self) -> &[Diagnostic] {
        &self.built.findings
    }

    fn exit_code(&self) -> u8 {
        self.built.exit
    }
}

/// Resolve and scan the roots for `search`, collecting any unusable-input notices (returned to be
/// streamed to stderr, never printed here).
fn catalog(search: &SearchArgs) -> Result<(Vec<AgentDef>, Vec<Diagnostic>), Diagnostic> {
    let env = Env::from_process().map_err(|e| {
        Diagnostic::error("cwd-unavailable", "cannot determine the current directory")
            .cause(e.to_string())
            .help("run `the` from an existing, readable directory")
    })?;
    let mut notices = Vec::new();
    if env.home.is_none() {
        notices.push(
            Diagnostic::warning(
                "home-unset",
                "HOME is not set; skipping user agent directories",
            )
            .help("set HOME, or pass a directory explicitly with `--dir <PATH>`"),
        );
    }

    let mut roots = discover::roots(&env, &search.dirs, search.no_user);
    if let Some(scope) = search.scope {
        roots.retain(|r| r.scope == scope);
    }
    // A `--dir` whose directory and `.claude/agents` subdir both do not exist is surfaced.
    for d in &search.dirs {
        let present = roots
            .iter()
            .any(|r: &Root| r.scope == Scope::Dir && r.exists && r.path.starts_with(d));
        if !present && !d.is_dir() {
            notices.push(
                Diagnostic::warning("dir-not-found", "`--dir` is not a directory")
                    .context(d.display().to_string()),
            );
        }
    }

    let (defs, errors) = discover::discover(&roots);
    for e in errors {
        notices.push(
            Diagnostic::warning("unreadable", "could not read an agents path; skipped it")
                .context(e.path.display().to_string())
                .cause(e.error),
        );
    }
    Ok((defs, notices))
}

/// The "no agent definitions found" note, shared by list and lint.
fn no_agents_note() -> Diagnostic {
    Diagnostic::note("no-agents", "no agent definitions found")
        .cause("no `.claude/agents` directory held any `*.md` definitions in the searched scopes")
        .help("`--dir <PATH>` scans another directory or project for agent definitions")
}

/// A definition's findings visible at the current beta setting (beta-staged rules hidden unless `--beta`).
fn visible(def: &AgentDef, beta: bool) -> impl Iterator<Item = &Finding> {
    def.findings.iter().filter(move |f| beta || !f.is_beta())
}

/// Convert every visible finding across `defs` into a rendered diagnostic (deterministic order:
/// definitions in discovery order, each definition's findings in order).
fn collect_diagnostics(defs: &[&AgentDef], beta: bool) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for def in defs {
        for f in visible(def, beta) {
            out.push(lint::diagnostic(f, &def.name, &def.path));
        }
    }
    out
}

fn by_scope<'a>(defs: &'a [AgentDef], search: &SearchArgs) -> Vec<&'a AgentDef> {
    defs.iter()
        .filter(|d| search.scope.is_none_or(|s| d.scope == s))
        .collect()
}

fn build_list(search: &SearchArgs, h: Human, beta: bool) -> Result<Built, Diagnostic> {
    let (defs, notices) = catalog(search)?;
    let defs = by_scope(&defs, search);
    let mut findings = collect_diagnostics(&defs, beta);
    if defs.is_empty() {
        findings.push(no_agents_note());
    }
    let report = present_list(&defs, h);
    Ok(Built {
        report,
        notices,
        findings,
        mode: DiagMode::Stream,
        exit: 0,
    })
}

fn present_list(defs: &[&AgentDef], _h: Human) -> Report {
    Report::new().table("agents", agents_table(defs))
}

fn agents_table(defs: &[&AgentDef]) -> Table {
    let mut t = Table::new(vec![
        Column::new("name", "NAME"),
        Column::new("scope", "SCOPE"),
        Column::new("runtime", "RUNTIME"),
        Column::new("model", "MODEL"),
        Column::new("tools", "TOOLS"),
        Column::new("description", "DESCRIPTION").max_width(DESCRIPTION_WIDTH),
        Column::json_only("path"),
        Column::json_only("file_stem"),
        Column::json_only("findings"),
    ]);
    for d in defs {
        t.push(vec![
            d.name.clone().into(),
            d.scope.as_str().into(),
            d.runtime.as_str().into(),
            Cell::opt(d.model()),
            d.tools().into(),
            Cell::opt(d.description().map(str::trim)),
            d.path.clone().into(),
            d.file_stem.clone().into(),
            Cell::Json(json!(d.findings)),
        ]);
    }
    t
}

fn build_show(name: &str, search: &SearchArgs, beta: bool) -> Result<Built, Diagnostic> {
    let (defs, notices) = catalog(search)?;
    let defs = by_scope(&defs, search);
    let matches: Vec<&AgentDef> = defs
        .iter()
        .copied()
        .filter(|d| d.name == name || d.file_stem == name)
        .collect();

    let def = match matches.as_slice() {
        [] => return Err(not_found(name, &defs)),
        [one] => *one,
        many => {
            let mut d = Diagnostic::error(
                "agent-ambiguous",
                format!("{} agent definitions are named `{name}`", many.len()),
            )
            .exit(Exit::Usage);
            for a in many {
                d = d.context(format!("{} ({})", a.path.display(), a.scope.as_str()));
            }
            return Err(d.help("narrow it with `--scope <project|user|dir>`"));
        }
    };

    let findings = collect_diagnostics(&[def], beta);
    Ok(Built {
        report: show_report(def),
        notices,
        findings,
        mode: DiagMode::Stream,
        exit: 0,
    })
}

fn not_found(name: &str, defs: &[&AgentDef]) -> Diagnostic {
    let d = Diagnostic::error("agent-not-found", format!("no agent named `{name}`"))
        .exit(Exit::NotFound);
    let lower = name.to_lowercase();
    let mut near: Vec<&str> = defs
        .iter()
        .map(|a| a.name.as_str())
        .filter(|n| n.to_lowercase().contains(&lower) || lower.contains(&n.to_lowercase()))
        .collect();
    near.dedup();
    match near.as_slice() {
        [] => d.help("run `the agents list` to see the available definitions"),
        [one] => d.help(format!("did you mean `{one}`?")),
        many => d.help(format!(
            "did you mean one of: {}?",
            many.iter()
                .map(|n| format!("`{n}`"))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

fn show_report(d: &AgentDef) -> Report {
    let mut rec = Record::new()
        .field("name", d.name.clone())
        .field("description", Cell::opt(d.description().map(str::trim)))
        .field("scope", d.scope.as_str())
        .field("runtime", d.runtime.as_str())
        .field("model", Cell::opt(d.model()))
        .field("path", d.path.clone());
    let tools = d.tools();
    if !tools.is_empty() {
        rec = rec.field("tools", tools);
    }
    // Any other frontmatter keys, in parsed order, surfaced as fields (skip the ones already shown).
    for (k, v) in &d.parsed.frontmatter {
        if matches!(k.as_str(), "name" | "description" | "model" | "tools") {
            continue;
        }
        let v = match v {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        rec = rec.field(k.clone(), v);
    }
    rec = rec.json_only(
        "frontmatter",
        Cell::Json(serde_json::Value::Object(d.parsed.frontmatter.clone())),
    );
    rec = rec.json_only("findings", Cell::Json(json!(d.findings)));

    Report::new()
        .record("agent", rec)
        .text("body", d.parsed.body.clone())
}

fn build_lint(
    target: Option<&str>,
    search: &SearchArgs,
    all: bool,
    beta: bool,
) -> Result<Built, Diagnostic> {
    // A path-like target is a scan root; a bare word is a name filter.
    let (search_owned, name): (SearchArgs, Option<&str>) = match target {
        Some(t) if is_path_like(t) => (with_root(search, PathBuf::from(t)), None),
        other => (search.clone(), other),
    };
    let search = &search_owned;
    let (defs, mut notices) = catalog(search)?;
    let candidates = by_scope(&defs, search);
    let defs: Vec<&AgentDef> = match name {
        Some(target) => {
            let matches: Vec<&AgentDef> = candidates
                .iter()
                .copied()
                .filter(|d| d.name == target || d.file_stem == target)
                .collect();
            if matches.is_empty() {
                return Err(not_found(target, &candidates));
            }
            matches
        }
        None => candidates,
    };

    if defs.is_empty() {
        notices.push(no_agents_note());
    }

    let findings = collect_diagnostics(&defs, beta);
    let report = Report::new()
        .table("agents", lint_table(&defs, beta))
        .json_only("diagnostics", Block::Table(diagnostics_table(&findings)));

    let exit = if findings
        .iter()
        .any(|d| matches!(d.level, Level::Error | Level::Warning))
    {
        Exit::Generic as u8
    } else {
        0
    };
    Ok(Built {
        report,
        notices,
        findings,
        mode: DiagMode::Summarize { all },
        exit,
    })
}

fn lint_table(defs: &[&AgentDef], beta: bool) -> Table {
    let mut t = Table::new(vec![
        Column::new("name", "NAME"),
        Column::new("scope", "SCOPE"),
        Column::new("status", "STATUS"),
        Column::new("issues", "ISSUES"),
        Column::json_only("path"),
        Column::json_only("findings"),
    ]);
    for d in defs {
        let shown: Vec<&Finding> = visible(d, beta).collect();
        let has_error = shown.iter().any(|f| f.is_error());
        let has_warning = shown.iter().any(|f| {
            matches!(
                crate::rules::rule(f.code).map(|r| r.severity),
                Some(crate::rules::Severity::Warn)
            )
        });
        let status = if has_error {
            Cell::toned(Tone::Fail, "error")
        } else if has_warning {
            Cell::toned(Tone::Warn, "warning")
        } else if !shown.is_empty() {
            Cell::toned(Tone::Dim, "note")
        } else {
            Cell::toned(Tone::Ok, "ok")
        };
        let codes: Vec<String> = {
            let mut v: Vec<String> = Vec::new();
            for f in &shown {
                if !v.iter().any(|c| c == f.code) {
                    v.push(f.code.to_owned());
                }
            }
            v
        };
        t.push(vec![
            d.name.clone().into(),
            d.scope.as_str().into(),
            status,
            codes.into(),
            d.path.clone().into(),
            Cell::Json(json!(shown)),
        ]);
    }
    t
}

fn diagnostics_table(diags: &[Diagnostic]) -> Table {
    let mut t = Table::new(vec![
        Column::json_only("level"),
        Column::json_only("code"),
        Column::json_only("title"),
        Column::json_only("cause"),
        Column::json_only("help"),
        Column::json_only("context"),
    ]);
    for d in diags {
        let level = match d.level {
            Level::Error => "error",
            Level::Warning => "warning",
            Level::Note => "note",
        };
        t.push(vec![
            level.into(),
            d.code.into(),
            d.title.clone().into(),
            Cell::opt(d.cause.clone()),
            Cell::opt(d.help.clone()),
            d.context.clone().into(),
        ]);
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use the_output::command::{Registry, StreamState, conformance};

    // The descriptor table must match the clap subcommand tree: a command added to one but not the
    // other is a registration bug. Mirrors the-skills' `descriptors_match_the_clap_tree`.
    #[test]
    fn descriptors_match_the_clap_tree() {
        let root = Agents.augment(Command::new("the"));
        let agents_cmd = root
            .get_subcommands()
            .find(|c| c.get_name() == "agents")
            .expect("agents subcommand folded onto the root");

        let mut real = vec!["agents".to_string()];
        for sub in agents_cmd.get_subcommands().filter(|c| !c.is_hide_set()) {
            if sub.get_name() == "help" {
                continue;
            }
            real.push(format!("agents {}", sub.get_name()));
        }
        real.sort();

        let mut declared: Vec<String> = DESCRIPTORS.iter().map(|d| d.path.to_string()).collect();
        declared.sort();

        assert_eq!(
            declared, real,
            "agents descriptors must match the clap subcommand tree exactly"
        );
    }

    #[test]
    fn descriptor_summaries_follow_the_style_guide() {
        for d in DESCRIPTORS {
            assert!(d.summary.len() <= 60, "`{}` summary too long", d.path);
            assert!(
                !d.summary.ends_with('.'),
                "`{}` summary has a trailing period",
                d.path
            );
            assert_eq!(
                d.summary,
                d.summary.to_lowercase(),
                "`{}` summary should be lowercase",
                d.path
            );
            assert_eq!(d.group, "agents");
        }
    }

    /// A hermetic agents fixture dir with a clean and a terse definition. Returns the dir.
    fn fixture(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("the-agents-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("reviewer.md"),
            "---\nname: reviewer\ndescription: Review changed Rust files for correctness and surface the riskiest diffs\ntools: Read, Grep\nmodel: sonnet\n---\n# Reviewer\nDo the review.\n",
        )
        .unwrap();
        fs::write(
            dir.join("terse.md"),
            "---\nname: terse\ndescription: PDF tools\n---\n# Terse\n",
        )
        .unwrap();
        dir
    }

    fn agents_matches(argv: &[&str]) -> clap::ArgMatches {
        let m = Agents
            .augment(Command::new("the"))
            .try_get_matches_from(argv)
            .unwrap();
        m.subcommand().unwrap().1.clone()
    }

    fn plain_ctx() -> Ctx {
        let plain = StreamState {
            color: false,
            tty: false,
        };
        Ctx::new(Format::Human, false, plain, plain)
    }

    fn run(argv: &[&str]) -> Box<dyn Results> {
        let sub = agents_matches(argv);
        Agents
            .execute(
                &Invocation {
                    target: "agents",
                    matches: sub,
                },
                &plain_ctx(),
            )
            .unwrap()
    }

    fn stdout_of(r: &dyn Results) -> String {
        let mut out = Vec::new();
        r.render(&mut out, Form::Plain).unwrap();
        String::from_utf8(out).unwrap()
    }

    fn stderr_of(r: &dyn Results) -> String {
        let mut err = Vec::new();
        r.render_diagnostics(&mut err, Format::Human).unwrap();
        String::from_utf8(err).unwrap()
    }

    #[test]
    fn execute_lints_a_dir_in_process() {
        let dir = fixture("exec");
        let d = dir.to_str().unwrap();
        let r = run(&["the", "agents", "lint", "--scope", "dir", "--dir", d]);
        let out = stdout_of(r.as_ref());
        assert!(
            out.contains("reviewer"),
            "lint table lists the definition:\n{out}"
        );
        assert!(out.contains("terse"));
        // The terse definition is a note (no error/warning), so exit stays 0 and a finding exists.
        assert!(
            !r.diagnostics().is_empty(),
            "the terse description is a finding"
        );
        assert!(
            stderr_of(r.as_ref()).contains("terse"),
            "the finding names the definition on stderr"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn show_renders_one_definition_and_body() {
        let dir = fixture("show");
        let d = dir.to_str().unwrap();
        let r = run(&[
            "the", "agents", "show", "reviewer", "--scope", "dir", "--dir", d,
        ]);
        let out = stdout_of(r.as_ref());
        assert!(out.contains("reviewer"), "{out}");
        assert!(
            out.contains("Do the review."),
            "the body is rendered:\n{out}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn show_unknown_name_is_not_found() {
        let dir = fixture("notfound");
        let d = dir.to_str().unwrap();
        let sub = agents_matches(&[
            "the", "agents", "show", "ghost", "--scope", "dir", "--dir", d,
        ]);
        let err = Agents
            .execute(
                &Invocation {
                    target: "agents",
                    matches: sub,
                },
                &plain_ctx(),
            )
            .err()
            .expect("an unknown name is an error");
        assert_eq!(err.code, "agent-not-found");
        assert_eq!(err.exit, Exit::NotFound);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_leading_path_is_scanned_not_rejected() {
        let dir = fixture("leadpath");
        let d = dir.to_str().unwrap();
        let r = run(&["the", "agents", "--scope", "dir", d]);
        assert!(stdout_of(r.as_ref()).contains("reviewer"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_non_path_in_the_subcommand_slot_is_a_usage_error() {
        let sub = agents_matches(&["the", "agents", "bogus"]);
        let err = Agents
            .execute(
                &Invocation {
                    target: "agents",
                    matches: sub,
                },
                &plain_ctx(),
            )
            .err()
            .expect("a non-path subcommand is an error");
        assert_eq!(err.code, "unknown-subcommand");
        assert_eq!(err.exit, Exit::Usage);
    }

    // The standard command-coverage test: one argv matrix (subcommands x flags), each line asserted
    // stable across the plain/ansi/json forms by the shared conformance harness.
    #[test]
    fn agents_matrix_is_form_stable() {
        let dir = fixture("matrix");
        let d = dir.to_str().unwrap();
        let reg = Registry::new().register(Box::new(Agents));
        let matrix: &[&[&str]] = &[
            &["the", "agents", "--scope", "dir", "--dir", d],
            &["the", "agents", "--scope", "dir", d], // a leading path in the subcommand slot
            &["the", "agents", "list", "--scope", "dir", "--dir", d],
            &[
                "the",
                "agents",
                "list",
                "--no-header",
                "--scope",
                "dir",
                "--dir",
                d,
            ],
            &["the", "agents", "lint", "--scope", "dir", "--dir", d],
            &[
                "the", "agents", "lint", "--all", "--scope", "dir", "--dir", d,
            ],
            &[
                "the", "agents", "lint", "--beta", "--scope", "dir", "--dir", d,
            ],
            &[
                "the", "agents", "show", "reviewer", "--scope", "dir", "--dir", d,
            ],
        ];
        conformance::assert_matrix(&reg, matrix);
        let _ = fs::remove_dir_all(&dir);
    }

    // Systematic flag-placement coverage: a flag before vs after the subcommand (or a leading path)
    // must produce identical output. The fixture carries a vague description so a swallowed `--beta`
    // would change the output and fail the comparison.
    #[test]
    fn flags_are_placement_independent() {
        let dir = std::env::temp_dir().join(format!("the-agents-placement-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        // A vague description -> the beta `agent-description-vague` rule fires under --beta only.
        fs::write(
            dir.join("vague.md"),
            "---\nname: vague\ndescription: handles various tasks across the whole codebase as required\n---\n# body\n",
        )
        .unwrap();
        let d = dir.to_str().unwrap();
        let reg = Registry::new().register(Box::new(Agents));

        // --beta before vs after a leading path.
        conformance::assert_equivalent(
            &reg,
            &["the", "agents", "--scope", "dir", "--beta", d],
            &["the", "agents", "--scope", "dir", d, "--beta"],
        );
        // --beta before vs after the `lint` subcommand.
        conformance::assert_equivalent(
            &reg,
            &[
                "the", "agents", "lint", "--beta", "--scope", "dir", "--dir", d,
            ],
            &[
                "the", "agents", "--beta", "lint", "--scope", "dir", "--dir", d,
            ],
        );
        // scan flags before vs after the subcommand.
        conformance::assert_equivalent(
            &reg,
            &["the", "agents", "lint", "--scope", "dir", "--dir", d],
            &["the", "agents", "--dir", d, "--scope", "dir", "lint"],
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn render_emits_the_report_as_plain_and_json() {
        let built = Built {
            report: Report::new().text("msg", "hello agents"),
            notices: Vec::new(),
            findings: Vec::new(),
            mode: DiagMode::Stream,
            exit: 0,
        };
        let plain_term = Term {
            color: false,
            tty: false,
        };
        let res = AgentsResults {
            built,
            h: Human {
                term: plain_term,
                header: true,
            },
            err_term: plain_term,
        };
        let mut out = Vec::new();
        res.render(&mut out, Form::Plain).unwrap();
        assert_eq!(String::from_utf8(out).unwrap().trim(), "hello agents");
        let mut j = Vec::new();
        res.render(&mut j, Form::Json).unwrap();
        assert!(String::from_utf8(j).unwrap().contains("hello agents"));
        assert_eq!(res.exit_code(), 0);
    }

    #[test]
    fn is_path_like_guards_the_subcommand_slot() {
        // Documents why a leading `.`/`..`/path routes to a scan and a bare word does not.
        assert!(is_path_like("."));
        assert!(!is_path_like("reviewer"));
        // Sanity: the helper agrees across the Path boundary used in build_agents.
        assert!(is_path_like(&Path::new("../x").to_string_lossy()));
    }

    #[test]
    fn list_streams_per_definition_findings_to_stderr() {
        let dir = fixture("list-stream");
        let d = dir.to_str().unwrap();
        // The bare list (and `list`) stream each finding; the terse definition names itself on stderr.
        let r = run(&["the", "agents", "list", "--scope", "dir", "--dir", d]);
        assert!(stdout_of(r.as_ref()).contains("reviewer"));
        assert!(
            stderr_of(r.as_ref()).contains("terse"),
            "list streams the terse finding to stderr"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_empty_scope_notes_no_definitions() {
        let dir = std::env::temp_dir().join(format!("the-agents-empty-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let d = dir.to_str().unwrap();
        let r = run(&["the", "agents", "list", "--scope", "dir", "--dir", d]);
        assert!(
            stderr_of(r.as_ref()).contains("no agent definitions found"),
            "an empty scope emits the no-agents note"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_dir_is_noticed() {
        let r = run(&[
            "the",
            "agents",
            "lint",
            "--scope",
            "dir",
            "--dir",
            "/no/such/dir/anywhere",
        ]);
        let err = stderr_of(r.as_ref());
        assert!(
            err.contains("not a directory"),
            "a missing --dir is surfaced:\n{err}"
        );
    }

    #[test]
    fn lint_an_unknown_name_is_not_found() {
        let dir = fixture("lint-name");
        let d = dir.to_str().unwrap();
        let sub = agents_matches(&[
            "the", "agents", "lint", "ghost", "--scope", "dir", "--dir", d,
        ]);
        let err = Agents
            .execute(
                &Invocation {
                    target: "agents",
                    matches: sub,
                },
                &plain_ctx(),
            )
            .err()
            .expect("an unknown lint name is an error");
        assert_eq!(err.code, "agent-not-found");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn lint_one_named_definition_narrows_the_scan() {
        let dir = fixture("lint-one");
        let d = dir.to_str().unwrap();
        // Linting `terse` by name lints only it: the table carries terse, not reviewer.
        let r = run(&[
            "the", "agents", "lint", "terse", "--scope", "dir", "--dir", d,
        ]);
        let out = stdout_of(r.as_ref());
        assert!(out.contains("terse"));
        assert!(
            !out.contains("reviewer"),
            "a name filter narrows the table:\n{out}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_key_is_off_by_default_and_surfaces_under_beta() {
        let dir =
            std::env::temp_dir().join(format!("the-agents-unknownkey-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        // A valid definition carrying an extra key (a tool's own field, or a typo). Because a closed
        // key set is assistant/tool-specific, the unknown-key check is off by default (`--beta` only).
        fs::write(
            dir.join("extra.md"),
            "---\nname: extra\ndescription: Review diffs for correctness and surface the riskiest changes\nwat: 1\n---\n# body\n",
        )
        .unwrap();
        let d = dir.to_str().unwrap();

        let def = run(&["the", "agents", "lint", "--scope", "dir", "--dir", d]);
        assert!(
            !stdout_of(def.as_ref()).contains("agent-unknown-key"),
            "the default lint does not flag unknown keys"
        );
        assert!(!stderr_of(def.as_ref()).contains("unknown frontmatter key"));
        assert_eq!(
            def.exit_code(),
            0,
            "an extra key alone does not fail the lint"
        );

        let beta = run(&[
            "the", "agents", "lint", "--beta", "--scope", "dir", "--dir", d,
        ]);
        assert!(
            stderr_of(beta.as_ref()).contains("unknown frontmatter key"),
            "--beta surfaces the unknown-key check"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
