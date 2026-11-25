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
use crate::rules::Severity;
use crate::{AgentDef, Scope, instructions};

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
    CommandDescriptor {
        path: "agents rules",
        group: "agents",
        tier: Tier::Aux,
        summary: "list the lint rules and what each checks",
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
        Some(AgentsCmd::Rules) => build_rules(),
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
        Column::new("assistant", "ASSISTANT"),
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
            d.assistant.id.into(),
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
        .field("assistant", d.assistant.id)
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

/// `the agents rules`: a static listing of the agent lint-rule registry. No scan, no discovery.
fn build_rules() -> Result<Built, Diagnostic> {
    Ok(Built {
        report: Report::new().table("rules", rules_table()),
        notices: Vec::new(),
        findings: Vec::new(),
        mode: DiagMode::Stream,
        exit: 0,
    })
}

/// One row per [`crate::rules::RULES`] entry. The JSON form is the machine-readable surface a
/// conformance check reconciles against the spec register (every shipped `code` with its severity,
/// gate, and kind), so it reads the one metadata table rather than restating the set anywhere. Mirrors
/// the-skills' `rules_table`.
fn rules_table() -> Table {
    use crate::rules::{Kind, Severity, Stability};
    let mut t = Table::new(vec![
        Column::new("code", "CODE"),
        Column::new("severity", "SEVERITY"),
        Column::new("gate", "GATE"),
        Column::new("what", "WHAT"),
        Column::json_only("kind"),
        Column::json_only("why"),
        Column::json_only("fix"),
    ]);
    for r in crate::rules::RULES {
        let (sev, tone) = match r.severity {
            Severity::Error => ("error", Tone::Fail),
            Severity::Warn => ("warning", Tone::Warn),
            Severity::Note => ("note", Tone::Dim),
        };
        let gate = match r.stability {
            Stability::Default => "default",
            Stability::Beta => "beta",
        };
        let kind = match r.kind {
            Kind::Breaking => "breaking",
            Kind::Spec => "spec",
            Kind::Security => "security",
        };
        t.push(vec![
            r.code.into(),
            Cell::toned(tone, sev),
            gate.into(),
            r.label.into(),
            kind.into(),
            r.why.into(),
            r.fix.into(),
        ]);
    }
    t
}

/// Discover and lint the instruction files (`CLAUDE.md` / `CLAUDE.local.md` / `AGENTS.md`) across the
/// project chain, user scope, and any `--dir`, honoring `--scope`. Part of `the agents lint` (there is
/// one lint command per group): a full-scope lint covers both agent definitions and instruction files.
fn gather_instruction_files(search: &SearchArgs) -> Vec<instructions::InstructionFile> {
    let Ok(env) = Env::from_process() else {
        return Vec::new();
    };
    let mut files = instructions::discover(&env, &search.dirs, search.no_user);
    if let Some(scope) = search.scope {
        files.retain(|f| f.scope == scope);
    }
    files
}

fn instructions_table(files: &[instructions::InstructionFile]) -> Table {
    let mut t = Table::new(vec![
        Column::new("file", "FILE"),
        Column::new("scope", "SCOPE"),
        Column::new("status", "STATUS"),
        Column::new("issues", "ISSUES"),
        Column::json_only("path"),
        Column::json_only("kind"),
        Column::json_only("findings"),
    ]);
    for f in files {
        let has_error = f.findings.iter().any(|x| {
            matches!(
                instructions::rule(x.code).map(|r| r.severity),
                Some(Severity::Error)
            )
        });
        let has_warning = f.findings.iter().any(|x| {
            matches!(
                instructions::rule(x.code).map(|r| r.severity),
                Some(Severity::Warn)
            )
        });
        let status = if has_error {
            Cell::toned(Tone::Fail, "error")
        } else if has_warning {
            Cell::toned(Tone::Warn, "warning")
        } else if !f.findings.is_empty() {
            Cell::toned(Tone::Dim, "note")
        } else {
            Cell::toned(Tone::Ok, "ok")
        };
        let mut codes: Vec<String> = Vec::new();
        for x in &f.findings {
            if !codes.iter().any(|c| c == x.code) {
                codes.push(x.code.to_owned());
            }
        }
        t.push(vec![
            f.kind.as_str().into(),
            f.scope.as_str().into(),
            status,
            codes.into(),
            f.path.clone().into(),
            f.kind.as_str().into(),
            Cell::Json(json!(f.findings)),
        ]);
    }
    t
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

    let mut findings = collect_diagnostics(&defs, beta);
    // Cross-cutting reference check: resolve each agent's `skills:` against the real skill catalog.
    // Needs the catalog (not available per-definition at discovery), so it runs here, like the-skills'
    // corpus findings. Only built when some definition actually declares skills (no scan otherwise).
    let skillref = skills_reference_diagnostics(&defs, search);
    // The definition paths carrying a skills-reference warning, so the table STATUS reflects it.
    let skillref_paths: std::collections::HashSet<String> = skillref
        .iter()
        .flat_map(|d| d.context.iter().cloned())
        .collect();
    findings.extend(skillref);

    // One lint command per group: a full-scope `the agents lint` also lints the instruction files
    // (CLAUDE.md / AGENTS.md) in the same scopes. When narrowed to a single named agent, it does not
    // (the instruction files are not that agent's), keeping a named lint focused.
    let instr_files = if name.is_none() {
        gather_instruction_files(search)
    } else {
        Vec::new()
    };
    for file in &instr_files {
        for f in &file.findings {
            findings.push(instructions::diagnostic(f, file));
        }
    }

    let mut report = Report::new().table("agents", lint_table(&defs, beta, &skillref_paths));
    if !instr_files.is_empty() {
        report = report.table("instructions", instructions_table(&instr_files));
    }
    report = report.json_only("diagnostics", Block::Table(diagnostics_table(&findings)));

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

/// Resolve each Claude agent's `skills:` entries against the real skill catalog, emitting
/// `agent-skill-not-found` for any that do not exist. `skills:` is a Claude subagent field, so this is
/// scoped to Claude definitions. The skill catalog is built only when some definition declares skills
/// (no scan otherwise), so the common case stays cheap.
fn skills_reference_diagnostics(defs: &[&AgentDef], search: &SearchArgs) -> Vec<Diagnostic> {
    let any_declares = defs
        .iter()
        .any(|d| d.assistant.id == "claude" && !d.skills().is_empty());
    if !any_declares {
        return Vec::new();
    }
    let known = known_skill_names(search);
    let mut out = Vec::new();
    for def in defs {
        if def.assistant.id != "claude" {
            continue;
        }
        for skill in def.skills() {
            if !known.contains(&skill) {
                out.push(lint::diagnostic(
                    &Finding {
                        code: "agent-skill-not-found",
                        message: format!(
                            "references skill `{skill}`, not found in the skill catalog"
                        ),
                        line: None,
                        col: None,
                    },
                    &def.name,
                    &def.path,
                ));
            }
        }
    }
    out
}

/// The set of available skill names, discovered via the-skills over the same scopes as the agent scan
/// (project + user unless `--no-user`, plus `--dir`). Empty if the environment is unavailable.
fn known_skill_names(search: &SearchArgs) -> std::collections::HashSet<String> {
    let Ok(env) = Env::from_process() else {
        return std::collections::HashSet::new();
    };
    let mut roots = the_skills::locate::plan(&env, &search.dirs);
    if search.no_user {
        roots.retain(|r| r.scope != the_skills::Scope::User);
    }
    the_skills::resolve::build(&roots)
        .skills
        .into_iter()
        .map(|s| s.name)
        .collect()
}

/// Build the lint result table. `skillref_paths` is the set of definition paths that carry a
/// command-level `agent-skill-not-found` warning (computed from the skill catalog, not in the
/// per-definition findings), so the per-row STATUS/ISSUES reflect it and never read `ok` while the
/// summary reports a warning.
fn lint_table(
    defs: &[&AgentDef],
    beta: bool,
    skillref_paths: &std::collections::HashSet<String>,
) -> Table {
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
        let has_skillref = skillref_paths.contains(&d.path.display().to_string());
        let has_error = shown.iter().any(|f| f.is_error());
        let has_warning = has_skillref
            || shown.iter().any(|f| {
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
            if has_skillref {
                v.push("agent-skill-not-found".to_owned());
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
            &["the", "agents", "rules"],
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
    fn skills_reference_resolves_against_the_skill_catalog() {
        // An agent's `skills:` is resolved against the real skill catalog: a present skill is fine, a
        // missing one fires agent-skill-not-found. Hermetic: a `--dir` holding one skill + two agents.
        let dir = std::env::temp_dir().join(format!("the-agents-skillref-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        // A real skill `present-skill` discoverable under the --dir.
        let skill = dir.join("present-skill");
        fs::create_dir_all(&skill).unwrap();
        fs::write(
            skill.join("SKILL.md"),
            "---\nname: present-skill\ndescription: Work with structured documents end to end\n---\n# body\n",
        )
        .unwrap();
        // One agent references the present skill (ok), one references a missing skill (fires).
        fs::write(
            dir.join("good.md"),
            "---\nname: good\ndescription: Reviews code with a real skill reference\nskills: present-skill\n---\n# body\n",
        )
        .unwrap();
        fs::write(
            dir.join("bad.md"),
            "---\nname: bad\ndescription: Reviews code with a dangling skill reference\nskills: no-such-skill\n---\n# body\n",
        )
        .unwrap();
        let d = dir.to_str().unwrap();

        let r = run(&["the", "agents", "lint", "--scope", "dir", "--dir", d]);
        let diags = r.diagnostics();
        let missing: Vec<_> = diags
            .iter()
            .filter(|x| x.code == "agent-skill-not-found")
            .collect();
        assert_eq!(
            missing.len(),
            1,
            "exactly one dangling skill reference: {diags:?}"
        );
        assert!(
            missing[0].title.contains("bad"),
            "the finding names the referencing agent"
        );
        assert!(
            missing[0]
                .cause
                .as_deref()
                .unwrap_or("")
                .contains("no-such-skill"),
            "the finding names the missing skill"
        );
        // The table STATUS reflects the command-level finding, not a false `ok`.
        assert!(
            stdout_of(r.as_ref()).contains("agent-skill-not-found"),
            "the lint table surfaces the skills-reference issue, not just the summary"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn lint_also_covers_instruction_files_in_the_same_scope() {
        // One `the agents lint` covers both agent definitions and the instruction files (CLAUDE.md /
        // AGENTS.md) in scope: a single lint command per group, not a separate one.
        // A realistic project root: the agent lives in `.claude/agents`, the instruction file at the root.
        let dir =
            std::env::temp_dir().join(format!("the-agents-instr-lint-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join(".claude/agents")).unwrap();
        fs::create_dir_all(dir.join("docs")).unwrap();
        fs::write(dir.join("docs/guide.md"), "hi").unwrap();
        fs::write(
            dir.join(".claude/agents/reviewer.md"),
            "---\nname: reviewer\ndescription: Review changed Rust files for correctness and risk\n---\n# body\n",
        )
        .unwrap();
        fs::write(
            dir.join("CLAUDE.md"),
            "# Project\n\nSee @docs/guide.md and @docs/missing.md.\n",
        )
        .unwrap();
        let d = dir.to_str().unwrap();

        let r = run(&["the", "agents", "lint", "--scope", "dir", "--dir", d]);
        let out = stdout_of(r.as_ref());
        assert!(
            out.contains("reviewer"),
            "the agent definition is listed:\n{out}"
        );
        assert!(
            out.contains("CLAUDE.md") && out.contains("instruction-broken-import"),
            "the one lint command also surfaces the instruction file finding:\n{out}"
        );
        assert!(
            !out.contains("agent-no-frontmatter"),
            "CLAUDE.md is an instruction file, not an empty agent definition:\n{out}"
        );
        assert!(
            stderr_of(r.as_ref()).contains("docs/missing.md"),
            "the broken import is reported"
        );
        assert_eq!(r.exit_code(), Exit::Generic as u8);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn lint_of_a_named_agent_does_not_pull_in_instruction_files() {
        let dir =
            std::env::temp_dir().join(format!("the-agents-instr-named-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join(".claude/agents")).unwrap();
        fs::write(
            dir.join(".claude/agents/reviewer.md"),
            "---\nname: reviewer\ndescription: Review changed Rust files for correctness and risk\n---\n# body\n",
        )
        .unwrap();
        fs::write(dir.join("CLAUDE.md"), "See @docs/missing.md.\n").unwrap();
        let d = dir.to_str().unwrap();

        let r = run(&[
            "the", "agents", "lint", "reviewer", "--scope", "dir", "--dir", d,
        ]);
        assert!(
            !stdout_of(r.as_ref()).contains("instruction-broken-import"),
            "a named-agent lint does not lint instruction files"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rules_lists_the_whole_registry_with_severity_and_gate() {
        let results = run(&["the", "agents", "rules"]);

        let plain = stdout_of(results.as_ref());
        // Every rule in the registry is listed (no silent subset); beta rules included.
        let listed = crate::rules::RULES
            .iter()
            .filter(|r| plain.contains(r.code))
            .count();
        assert_eq!(
            listed,
            crate::rules::RULES.len(),
            "every registry rule is listed:\n{plain}"
        );
        assert!(plain.contains("agent-description-vague"), "beta rule shown");

        // JSON carries the machine-readable fields a conformance reconciliation needs.
        let mut j = Vec::new();
        results.render(&mut j, Form::Json).unwrap();
        let json = String::from_utf8(j).unwrap();
        for field in ["\"code\"", "\"severity\"", "\"gate\"", "\"kind\""] {
            assert!(json.contains(field), "json exposes {field}:\n{json}");
        }
        assert!(json.contains("\"agent-ignored-field\""));
        // rules is static: no finding, exit 0.
        assert!(results.diagnostics().is_empty());
        assert_eq!(results.exit_code(), 0);
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
