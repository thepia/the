//! `the skills list | show | dirs | lint`. Each builds a [`Report`]; rendering is format-independent.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use clap::{Args, Command, FromArgMatches};
use serde_json::{Value, json};
use the_output::command::{
    CommandDescriptor, Ctx, Form, Invocation, Results, Target, Tier, is_path_like,
};

use crate::analysis::corpus_findings;
use crate::cli::{ListForm, SearchArgs, SkillsArgs, SkillsCmd};
use crate::fix::{self, Advice, FixClass};
use crate::installed::installed_agents;
use crate::locate::{Env, SearchRoot, plan, plan_for};
use crate::parse::{Problem, ProblemKind};
use crate::resolve::{Catalog, RootInfo, Skill, Status, build, build_for};
use crate::{Agent, Scope, bundled_files};
use the_output::diag::{Diagnostic, Exit, Level, summarize};
use the_output::output::Format;
use the_output::style::{self, Term};
use the_output::table::{Block, Cell, Column, Human, Record, Report, Table, Tone};

const DESCRIPTION_WIDTH: usize = 72;

/// `search` with `root` added as a `--dir` scan root. The positional path is additive, exactly like
/// `--dir`: the agent roots (including the user's own skills) stay in the scan unless the caller
/// narrows them with `--no-user` or `--scope dir`.
fn with_root(search: &SearchArgs, root: PathBuf) -> SearchArgs {
    let mut s = search.clone();
    s.dirs.push(root);
    s
}

/// Fold an optional positional `<path>` (list/dirs) into the search as a scan root.
fn with_positional(search: &SearchArgs, path: Option<PathBuf>) -> SearchArgs {
    match path {
        Some(p) => with_root(search, p),
        None => search.clone(),
    }
}

/// Drop the user-scope roots (`~/.claude/skills` et al.) when `--no-user` is set, so a project/dir scan
/// does not mix in the developer's personal skills. Project, system, and `--dir` roots are untouched.
fn without_user(mut roots: Vec<SearchRoot>, no_user: bool) -> Vec<SearchRoot> {
    if no_user {
        roots.retain(|r| r.scope != Scope::User);
    }
    roots
}

/// The `skills` command group as a routable [`Target`]. Registered into the pipeline's registry; the
/// grammar is contributed by [`Target::augment`] and a routed invocation runs through
/// [`Target::execute`], which returns a [`SkillsResults`] the pipeline renders.
pub struct Skills;

/// The commands this target owns, for help / governance and the registry drift test. Summaries follow
/// the style guide (lowercase, imperative, no trailing period, <= 60 chars).
const DESCRIPTORS: &[CommandDescriptor] = &[
    CommandDescriptor {
        path: "skills",
        group: "skills",
        tier: Tier::Core,
        summary: "inspect and lint local agent skills",
    },
    CommandDescriptor {
        path: "skills list",
        group: "skills",
        tier: Tier::Core,
        summary: "list skills, their agents, and symlinks",
    },
    CommandDescriptor {
        path: "skills show",
        group: "skills",
        tier: Tier::Aux,
        summary: "show one skill's metadata and verdicts",
    },
    CommandDescriptor {
        path: "skills dirs",
        group: "skills",
        tier: Tier::Aux,
        summary: "list the directories each agent searches",
    },
    CommandDescriptor {
        path: "skills lint",
        group: "skills",
        tier: Tier::Core,
        summary: "lint skills for loadability, spec, links, security",
    },
];

impl Target for Skills {
    fn id(&self) -> &'static str {
        "skills"
    }

    fn descriptors(&self) -> &'static [CommandDescriptor] {
        DESCRIPTORS
    }

    fn augment(&self, root: Command) -> Command {
        root.subcommand(
            SkillsArgs::augment_args(Command::new("skills"))
                .about("Agent Skills quality managed for local AI assistants use."),
        )
    }

    fn execute(&self, inv: &Invocation, ctx: &Ctx) -> Result<Box<dyn Results>, Diagnostic> {
        let args = SkillsArgs::from_arg_matches(&inv.matches).map_err(|e| {
            Diagnostic::error("skills-args", "could not parse `skills` arguments")
                .cause(e.to_string())
                .exit(Exit::Usage)
        })?;
        // Build the result with the Human that matches the resolved stdout form, so prose blocks
        // (which bake color at build time) are colored consistently with how they will render. The
        // pipeline renders exactly once, with `ctx.form()`.
        let colored = matches!(ctx.form(), Form::Ansi);
        let h = Human {
            term: Term {
                color: colored,
                tty: ctx.stdout().tty,
            },
            header: !ctx.no_header(),
        };
        let built = build_skills(args, ctx.format(), h)?;
        Ok(Box::new(SkillsResults {
            built,
            h,
            err_term: Term {
                color: ctx.stderr().color,
                tty: ctx.stderr().tty,
            },
        }))
    }
}

/// Route a parsed `skills` invocation to its subcommand builder.
fn build_skills(args: SkillsArgs, format: Format, h: Human) -> Result<Built, Diagnostic> {
    match args.command {
        Some(SkillsCmd::List(list_args)) => {
            let form = list_args.presentation();
            let search = with_positional(&list_args.search, list_args.path);
            build_list(&search, form, format, h)
        }
        Some(SkillsCmd::Show { name, search }) => build_show(&name, &search),
        Some(SkillsCmd::Dirs { path, search }) => {
            let search = with_positional(&search, path);
            build_dirs(&search)
        }
        Some(SkillsCmd::Lint {
            target,
            search,
            all,
            advice,
        }) => build_lint(target.as_deref(), &search, all, advice, args.beta),
        Some(SkillsCmd::Path(tokens)) => {
            // A path (or paths) given instead of a subcommand: scan them like the bare `skills`,
            // additively onto the top-level search. A leading non-path token is a real typo.
            let first = tokens.first().map(String::as_str).unwrap_or_default();
            if !is_path_like(first) {
                return Err(unknown_subcommand(first));
            }
            let mut search = args.search.clone();
            for t in tokens.iter().filter(|t| is_path_like(t)) {
                search.dirs.push(PathBuf::from(t));
            }
            let form = if args.by_scope {
                ListForm::ByScope
            } else {
                ListForm::Prose
            };
            build_list(&search, form, format, h)
        }
        None => {
            let form = if args.by_scope {
                ListForm::ByScope
            } else {
                ListForm::Prose
            };
            build_list(&args.search, form, format, h)
        }
    }
}

/// A token in the subcommand slot that is neither a known subcommand nor a path.
fn unknown_subcommand(name: &str) -> Diagnostic {
    Diagnostic::error(
        "unknown-subcommand",
        format!("unrecognized subcommand `{name}`"),
    )
    .cause("not a known `skills` subcommand, and not a path")
    .help("run `the skills --help`, or pass a path like `the skills .` to scan a directory")
    .exit(Exit::Usage)
}

/// How a subcommand's issue diagnostics are presented on stderr.
#[derive(Clone, Copy)]
enum DiagMode {
    /// Emit each diagnostic in turn (list, show, dirs).
    Stream,
    /// The capped, rustc-style summary with a `--all` hint (lint).
    Summarize { all: bool },
}

/// A built-but-unrendered skills result: the stdout report, the stderr diagnostics (input `notices`
/// that always stream plus the subcommand's `findings` presented per `mode`), and the exit code.
struct Built {
    report: Report,
    /// Input notices (home-unset, unreadable path, no-skills): always streamed to stderr.
    notices: Vec<Diagnostic>,
    /// The subcommand's issues, presented per `mode`.
    findings: Vec<Diagnostic>,
    mode: DiagMode,
    exit: u8,
}

/// The skills command result. Holds the report already built for the resolved form plus the stream
/// state, so the pipeline renders it to stdout/stderr without re-running any logic.
pub struct SkillsResults {
    built: Built,
    /// The `Human` the report was built with (color matches the resolved stdout form).
    h: Human,
    /// Terminal state for stderr diagnostics.
    err_term: Term,
}

impl Results for SkillsResults {
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
                        .render(self.err_term, Some("the skills lint --all"));
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

fn status_cell(s: &Status) -> Cell {
    let tone = match s {
        Status::Loaded => Tone::Ok,
        Status::Shadowed { .. } => Tone::Dim,
        Status::Conflict { .. } | Status::Skipped(_) | Status::Unknown(_) => Tone::Warn,
    };
    Cell::toned(tone, s.as_str())
}

/// Plan and scan; report unusable inputs on stderr.
/// When `only_installed` is true, only assistants installed on the machine (plus any explicitly
/// requested `--agent`) are considered.
/// Plan and scan; collect any unusable-input notices (returned to be streamed to stderr, never
/// printed here). When `only_installed` is true, only assistants installed on the machine (plus any
/// explicitly requested `--agent`) are considered.
fn catalog(
    search: &SearchArgs,
    only_installed: bool,
) -> Result<(Catalog, Vec<Diagnostic>), Diagnostic> {
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
                "HOME is not set; skipping user skill directories",
            )
            .help("set HOME, or pass a directory explicitly with `--dir <PATH>`"),
        );
    }
    let cat = if only_installed {
        let mut agents = installed_agents(&env);
        if let Some(a) = search.agent
            && !agents.contains(&a)
        {
            agents.push(a);
            agents.sort();
        }
        let roots = without_user(plan_for(&env, &agents, &search.dirs), search.no_user);
        build_for(&roots, &agents)
    } else {
        let roots = without_user(plan(&env, &search.dirs), search.no_user);
        build(&roots)
    };
    for r in cat
        .roots
        .iter()
        .filter(|r| r.agents.is_empty() && !r.exists)
    {
        notices.push(
            Diagnostic::warning("dir-not-found", "`--dir` is not a directory")
                .context(r.path.display().to_string()),
        );
    }
    for e in &cat.errors {
        notices.push(
            Diagnostic::warning("unreadable", "could not read a skill path; skipped it")
                .context(e.path.display().to_string())
                .cause(e.error.to_string()),
        );
    }
    Ok((cat, notices))
}

/// The "no skills found" note, shared by list and lint.
fn no_skills_note(cat: &Catalog) -> Diagnostic {
    Diagnostic::note("no-skills", "no skills found")
        .cause(format!(
            "{} of {} searched directories exist",
            cat.roots.iter().filter(|r| r.exists).count(),
            cat.roots.len()
        ))
        .help("`the skills dirs` shows where each agent looks; `--dir <PATH>` searches another directory")
}

fn selected<'a>(cat: &'a Catalog, search: &SearchArgs) -> Vec<&'a Skill> {
    cat.skills
        .iter()
        .filter(|s| search.scope.is_none_or(|sc| s.scope == sc))
        .filter(|s| {
            // Filtering to an agent keeps the skills that agent loads AND the ones whose verdict is
            // unknown for it (Gemini + broken frontmatter): those still need surfacing so they get
            // fixed, rather than vanishing under `--agent gemini`.
            search.agent.is_none_or(|a| {
                s.view(a)
                    .is_some_and(|v| v.status.is_loaded() || matches!(v.status, Status::Unknown(_)))
            })
        })
        .collect()
}

fn agent_list(agents: &[Agent]) -> Vec<String> {
    agents.iter().map(|a| a.as_str().to_owned()).collect()
}

fn unique_fixes<'a>(fixes: impl IntoIterator<Item = &'a str>) -> String {
    let mut v: Vec<&str> = Vec::new();
    for f in fixes {
        if !v.contains(&f) {
            v.push(f);
        }
    }
    v.join("; ")
}

/// Collect structured diagnostics for `skills`:
/// - `Diagnostic::error` for `symlink-escape`, `trojan-source`, `exposed-secret`
/// - `Diagnostic::warning` for `skill-skipped`, `skill-conflict`, `broken-link`, `link-escapes-skill`, `broken-symlink`, `unrestricted-bash`
/// - `Diagnostic::note` for `skill-lint` spec conformance issues
fn collect_diagnostics(skills: &[&Skill]) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let mut conflicts_seen: Vec<&str> = Vec::new();
    let mut aliases_seen: Vec<&str> = Vec::new();
    for s in skills {
        let skipped: Vec<_> = s
            .agents
            .iter()
            .filter_map(|v| match &v.status {
                Status::Skipped(p) => Some((v.agent, p)),
                _ => None,
            })
            .collect();
        if !skipped.is_empty() {
            let who = skipped
                .iter()
                .map(|(a, _)| a.as_str())
                .collect::<Vec<_>>()
                .join(" and ");
            let mut causes: Vec<String> = Vec::new();
            for (_, p) in &skipped {
                if !causes.contains(&p.message) {
                    causes.push(p.message.clone());
                }
            }
            out.push(
                Diagnostic::warning("skill-skipped", format!("{who} skip skill `{}`", s.name))
                    .context(s.path.display().to_string())
                    .cause(causes.join("; "))
                    .help(unique_fixes(skipped.iter().map(|(_, p)| p.fix))),
            );
        }

        // Gemini verdicts `the` cannot predict (a broken-but-present frontmatter block its lenient
        // parser may recover). Surface the malformed frontmatter so it gets fixed even when no
        // strict agent flagged it; the verdict itself is honestly left undetermined.
        let unknown: Vec<_> = s
            .agents
            .iter()
            .filter_map(|v| match &v.status {
                Status::Unknown(p) => Some((v.agent, p)),
                _ => None,
            })
            .collect();
        if !unknown.is_empty() {
            let who = unknown
                .iter()
                .map(|(a, _)| a.as_str())
                .collect::<Vec<_>>()
                .join(" and ");
            let mut causes: Vec<String> = Vec::new();
            for (_, p) in &unknown {
                if !causes.contains(&p.message) {
                    causes.push(p.message.clone());
                }
            }
            out.push(
                Diagnostic::warning(
                    "skill-unknown",
                    format!("cannot tell whether {who} loads skill `{}`", s.name),
                )
                .context(s.path.display().to_string())
                .cause(causes.join("; "))
                .help(unique_fixes(unknown.iter().map(|(_, p)| p.fix))),
            );
        }

        if let Some(Status::Conflict { with }) = s.view(Agent::Opencode).map(|v| &v.status)
            && !conflicts_seen.contains(&s.name.as_str())
        {
            conflicts_seen.push(&s.name);
            let mut d = Diagnostic::warning(
                "skill-conflict",
                format!(
                    "{} skills are named `{}`; OpenCode loads one of them unpredictably",
                    with.len() + 1,
                    s.name
                ),
            )
            .context(s.path.display().to_string());
            for p in with {
                d = d.context(p.display().to_string());
            }
            out.push(d.help("rename all but one, or remove the duplicates"));
        }

        if !s.claude_alias_with.is_empty()
            && let Some(alias) = crate::parse::claude_alias(&s.parsed)
            && !aliases_seen.contains(&alias)
        {
            aliases_seen.push(alias);
            let mut d = Diagnostic::warning(
                "claude-alias-collision",
                format!(
                    "{} skills claim the Claude Code command `/{alias}`; it invokes only one of them",
                    s.claude_alias_with.len() + 1
                ),
            )
            .context(s.path.display().to_string());
            for p in &s.claude_alias_with {
                d = d.context(p.display().to_string());
            }
            out.push(
                d.cause("each is still listed and invocable by its directory name")
                    .help("give each skill a unique `name:` (ideally its directory name), or remove `name:`"),
            );
        }

        for p in &s.lint {
            if p.kind() == ProblemKind::Spec {
                continue;
            }
            let title = format!("skill `{}`: {}", s.name, issue_label(p.code));
            let d = if p.is_error() {
                Diagnostic::error(p.code, title)
            } else {
                Diagnostic::warning(p.code, title)
            };
            // Point at the exact SKILL.md location when the problem has one (links, assets,
            // security), so the context is a clickable `path:line` (or `path:line:col` when the
            // column is precisely known), which an editor opens with the cursor there.
            let path = s.path.display();
            let location = match (p.line, p.col) {
                (Some(line), Some(col)) => format!("{path}:{line}:{col}"),
                (Some(line), None) => format!("{path}:{line}"),
                _ => path.to_string(),
            };
            // what = title (label), why = the impact, fix = help. The specific message leads the
            // cause, the registry's `why` states why it matters.
            out.push(
                d.context(location)
                    .cause(format!("{}; {}", p.message, p.why))
                    .help(p.fix),
            );
        }

        let spec_lint: Vec<&Problem> = s
            .lint
            .iter()
            .filter(|p| p.kind() == ProblemKind::Spec)
            .collect();
        if !spec_lint.is_empty() {
            out.push(
                Diagnostic::note(
                    "skill-lint",
                    format!("skill `{}` does not follow the Agent Skills format", s.name),
                )
                .context(s.path.display().to_string())
                .cause(
                    spec_lint
                        .iter()
                        .map(|p| format!("{}; {}", p.message, p.why))
                        .collect::<Vec<_>>()
                        .join(" | "),
                )
                .help(unique_fixes(spec_lint.iter().map(|p| p.fix))),
            );
        }
    }
    out
}

fn status_json(s: &Skill) -> Value {
    let mut m = serde_json::Map::new();
    for v in &s.agents {
        let mut o = json!({ "status": v.status.as_str(), "name": v.name });
        match &v.status {
            Status::Shadowed { by } => o["by"] = json!(by),
            Status::Conflict { with } => o["with"] = json!(with),
            Status::Skipped(p) | Status::Unknown(p) => o["problem"] = json!(p),
            Status::Loaded => {}
        }
        m.insert(v.agent.as_str().to_owned(), o);
    }
    Value::Object(m)
}

fn locations_json(s: &Skill) -> Value {
    Value::Array(
        s.paths()
            .into_iter()
            .map(|(l, agents)| json!({ "path": l.path, "scope": l.scope, "agents": agents, "link": l.link }))
            .collect(),
    )
}

fn build_list(
    search: &SearchArgs,
    form: ListForm,
    format: Format,
    h: Human,
) -> Result<Built, Diagnostic> {
    let (cat, notices) = catalog(search, true)?;
    let skills = selected(&cat, search);
    // The tabular and JSON forms stream the per-skill diagnostics; the prose forms carry their own
    // inline issue summary instead, so per-skill diagnostics are suppressed there (unchanged).
    let mut findings = Vec::new();
    if matches!(form, ListForm::Tabular) || matches!(format, Format::Json) {
        findings = collect_diagnostics(&skills);
    }
    if skills.is_empty() {
        findings.push(no_skills_note(&cat));
    }
    let report = present_list(&skills, &cat.agents, &cat.roots, form, h);
    Ok(Built {
        report,
        notices,
        findings,
        mode: DiagMode::Stream,
        exit: 0,
    })
}

/// Build the [`Report`] for `the skills` / `the skills list` in the requested [`ListForm`].
fn present_list(
    skills: &[&Skill],
    agents: &[Agent],
    roots: &[RootInfo],
    form: ListForm,
    h: Human,
) -> Report {
    match form {
        ListForm::Tabular => present_tabular(skills, roots),
        ListForm::Prose => present_prose_single(skills, agents, roots, h),
        ListForm::ByScope => present_prose_by_scope(skills, roots, h),
    }
}

fn present_tabular(skills: &[&Skill], roots: &[RootInfo]) -> Report {
    Report::new()
        .table("skills", skills_table(skills))
        .json_only("searched", Block::Table(roots_table(roots)))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum IssueTone {
    Warn,
    Error,
}

fn skill_issue_tone(s: &Skill) -> Option<IssueTone> {
    if s.agents
        .iter()
        .any(|v| matches!(v.status, Status::Skipped(_)))
        || s.lint.iter().any(Problem::is_error)
    {
        Some(IssueTone::Error)
    } else if s
        .agents
        .iter()
        .any(|v| matches!(v.status, Status::Conflict { .. } | Status::Unknown(_)))
        || !s.lint.is_empty()
    {
        Some(IssueTone::Warn)
    } else {
        None
    }
}

/// Render a list of skills as a comma-separated paragraph, deduplicating same-named skills within
/// the paragraph and prefixing a warning/error colored `▲` (`style::GLYPH_ISSUE`) to skills with issues.
fn format_skill_paragraph(skills: &[&Skill], h: Human) -> String {
    let mut entries: Vec<(&str, Option<IssueTone>)> = Vec::new();
    for s in skills {
        let tone = skill_issue_tone(s);
        match entries
            .iter_mut()
            .find(|(name, _)| *name == s.name.as_str())
        {
            Some((_, existing)) => {
                if tone > *existing {
                    *existing = tone;
                }
            }
            None => entries.push((s.name.as_str(), tone)),
        }
    }
    entries
        .into_iter()
        .map(|(name, tone)| match tone {
            Some(IssueTone::Error) => {
                format!(
                    "{}{name}",
                    h.term.paint(style::FAIL.bold(), style::GLYPH_ISSUE)
                )
            }
            Some(IssueTone::Warn) => {
                format!(
                    "{}{name}",
                    h.term.paint(style::WARN.bold(), style::GLYPH_ISSUE)
                )
            }
            None => name.to_owned(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn issue_label(code: &str) -> &'static str {
    // Single source of truth: the rule registry (the-skills `rules`).
    crate::rules::label(code)
}

/// Build a concise summary of totals and issue counts across `skills` for prose output.
fn format_issue_summary(skills: &[&Skill], h: Human) -> Option<String> {
    const ISSUE_ORDER: &[(&str, IssueTone)] = &[
        ("no-frontmatter", IssueTone::Error),
        ("frontmatter-unterminated", IssueTone::Error),
        ("frontmatter-invalid", IssueTone::Error),
        ("name-missing", IssueTone::Error),
        ("name-has-slash", IssueTone::Error),
        ("description-not-string", IssueTone::Error),
        ("symlink-escape", IssueTone::Error),
        ("trojan-source", IssueTone::Error),
        ("exposed-secret", IssueTone::Error),
        ("skill-conflict", IssueTone::Warn),
        ("broken-link", IssueTone::Warn),
        ("link-escapes-skill", IssueTone::Warn),
        ("broken-symlink", IssueTone::Warn),
        ("unrestricted-bash", IssueTone::Warn),
        ("name-format", IssueTone::Warn),
        ("name-mismatch", IssueTone::Warn),
        ("description-missing", IssueTone::Warn),
        ("description-too-long", IssueTone::Warn),
        ("compatibility-invalid", IssueTone::Warn),
        ("license-not-string", IssueTone::Warn),
        ("metadata-not-map", IssueTone::Warn),
        ("allowed-tools-invalid", IssueTone::Warn),
        ("unknown-frontmatter-key", IssueTone::Warn),
    ];

    let mut distinct_names: Vec<&str> = Vec::new();
    let mut issue_names: Vec<&str> = Vec::new();
    for s in skills {
        if !distinct_names.contains(&s.name.as_str()) {
            distinct_names.push(&s.name);
        }
        if skill_issue_tone(s).is_some() && !issue_names.contains(&s.name.as_str()) {
            issue_names.push(&s.name);
        }
    }
    if issue_names.is_empty() {
        return None;
    }

    let mut lines = Vec::new();
    let total = distinct_names.len();
    let affected = issue_names.len();
    let skill_word = if total == 1 { "skill" } else { "skills" };
    lines.push(h.term.paint(
        style::HEAD,
        &format!("{total} {skill_word} ({affected} with issues):"),
    ));

    for &(code, tone) in ISSUE_ORDER {
        let mut count_names: Vec<&str> = Vec::new();
        for s in skills {
            let has_code = match code {
                "skill-conflict" => s
                    .agents
                    .iter()
                    .any(|v| matches!(v.status, Status::Conflict { .. })),
                _ => {
                    s.agents.iter().any(|v| {
                        matches!(&v.status, Status::Skipped(p) | Status::Unknown(p) if p.code == code)
                    }) || s.lint.iter().any(|p| p.code == code)
                }
            };
            if has_code && !count_names.contains(&s.name.as_str()) {
                count_names.push(&s.name);
            }
        }
        if !count_names.is_empty() {
            let glyph_style = match tone {
                IssueTone::Error => style::FAIL.bold(),
                IssueTone::Warn => style::WARN.bold(),
            };
            let glyph = h.term.paint(glyph_style, style::GLYPH_ISSUE);
            lines.push(format!(
                "  {glyph} {} {}",
                count_names.len(),
                issue_label(code)
            ));
        }
    }

    Some(lines.join("\n"))
}

fn format_assistants_paragraph(agents: &[Agent], h: Human) -> Option<String> {
    if agents.is_empty() {
        return None;
    }
    let heading = h.term.paint(style::brand(), "[assistants]");
    Some(format!("{heading}\n{}", agent_list(agents).join(", ")))
}

fn present_prose_single(
    skills: &[&Skill],
    agents: &[Agent],
    roots: &[RootInfo],
    h: Human,
) -> Report {
    let mut report = Report::new();
    if let Some(assistants) = format_assistants_paragraph(agents, h) {
        report = report.human_only("assistants", Block::Text(assistants));
    }
    if !skills.is_empty() {
        let heading = h.term.paint(style::brand(), "[all skills]");
        let paragraph = format!("{heading}\n{}", format_skill_paragraph(skills, h));
        report = report.human_only("prose", Block::Text(paragraph));
    }
    if let Some(summary) = format_issue_summary(skills, h) {
        report = report.human_only("issues", Block::Text(summary));
    }
    report
        .json_only("skills", Block::Table(skills_table(skills)))
        .json_only("searched", Block::Table(roots_table(roots)))
}

fn present_prose_by_scope(skills: &[&Skill], roots: &[RootInfo], h: Human) -> Report {
    let mut report = Report::new();
    for scope in [Scope::Project, Scope::User, Scope::System, Scope::Dir] {
        let group: Vec<&Skill> = skills
            .iter()
            .copied()
            .filter(|s| s.scope == scope)
            .collect();
        if !group.is_empty() {
            let heading = h
                .term
                .paint(style::brand(), &format!("{}:", scope.as_str()));
            let paragraph = format!("{heading}\n{}", format_skill_paragraph(&group, h));
            report = report.human_only(scope.as_str(), Block::Text(paragraph));
        }
    }
    if let Some(summary) = format_issue_summary(skills, h) {
        report = report.human_only("issues", Block::Text(summary));
    }
    report
        .json_only("skills", Block::Table(skills_table(skills)))
        .json_only("searched", Block::Table(roots_table(roots)))
}

fn skills_table(skills: &[&Skill]) -> Table {
    let mut t = Table::new(vec![
        Column::new("name", "NAME"),
        Column::new("scope", "SCOPE"),
        Column::new("agents", "AGENTS"),
        Column::new("linked", "LINKED"),
        Column::new("description", "DESCRIPTION").max_width(DESCRIPTION_WIDTH),
        Column::json_only("path"),
        Column::json_only("dir"),
        Column::json_only("status"),
        Column::json_only("locations"),
        Column::json_only("lint"),
    ]);
    for s in skills {
        t.push(vec![
            s.name.clone().into(),
            s.scope.as_str().into(),
            agent_list(&s.loaded_by()).into(),
            Cell::Bool(s.linked()),
            Cell::opt(s.description()),
            s.path.clone().into(),
            s.dir().to_path_buf().into(),
            Cell::Json(status_json(s)),
            Cell::Json(locations_json(s)),
            Cell::Json(json!(s.lint)),
        ]);
    }
    t
}

fn roots_table(roots: &[RootInfo]) -> Table {
    let mut t = Table::new(vec![
        Column::new("path", "PATH"),
        Column::new("scope", "SCOPE"),
        Column::new("agents", "AGENTS"),
        Column::new("exists", "EXISTS"),
        Column::new("links_to", "LINKS TO"),
        Column::json_only("link"),
    ]);
    for r in roots {
        t.push(vec![
            r.path.clone().into(),
            r.scope.as_str().into(),
            agent_list(&r.agents).into(),
            Cell::Bool(r.exists),
            r.link
                .as_ref()
                .map_or(Cell::Empty, |l| l.target.clone().into()),
            r.link
                .as_ref()
                .map_or(Cell::Empty, |l| Cell::Json(json!(l))),
        ]);
    }
    t
}

fn build_dirs(search: &SearchArgs) -> Result<Built, Diagnostic> {
    let (cat, notices) = catalog(search, false)?;
    let roots: Vec<RootInfo> = cat
        .roots
        .into_iter()
        .filter(|r| search.scope.is_none_or(|s| r.scope == s))
        .filter(|r| search.agent.is_none_or(|a| r.agents.contains(&a)))
        .collect();
    Ok(Built {
        report: Report::new().table("dirs", roots_table(&roots)),
        notices,
        findings: Vec::new(),
        mode: DiagMode::Stream,
        exit: 0,
    })
}

fn dir_name(skill_md: &Path) -> Option<String> {
    skill_md
        .parent()?
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
}

fn build_show(name: &str, search: &SearchArgs) -> Result<Built, Diagnostic> {
    let (cat, notices) = catalog(search, true)?;
    let candidates = selected(&cat, search);
    let matches: Vec<&Skill> = candidates
        .iter()
        .copied()
        .filter(|s| {
            s.name == name
                || s.agents.iter().any(|v| v.name == name)
                || s.locations
                    .iter()
                    .any(|l| dir_name(&l.path).as_deref() == Some(name))
        })
        .collect();

    let skill = match matches.as_slice() {
        [] => return Err(not_found(name, &candidates)),
        [one] => *one,
        many => {
            let mut d = Diagnostic::error(
                "skill-ambiguous",
                format!("{} skills are named `{name}`", many.len()),
            )
            .exit(Exit::Usage);
            for s in many {
                let by = agent_list(&s.loaded_by());
                let by = if by.is_empty() {
                    "no agent".to_owned()
                } else {
                    by.join(", ")
                };
                d = d.context(format!(
                    "{} ({}; loaded by {by})",
                    s.path.display(),
                    s.scope.as_str()
                ));
            }
            return Err(d.help("narrow it with `--agent <claude|opencode|goose>` (that agent's pick) or `--scope <project|user|system|dir>`"));
        }
    };

    let findings = collect_diagnostics(&[skill]);
    Ok(Built {
        report: show_report(skill),
        notices,
        findings,
        mode: DiagMode::Stream,
        exit: 0,
    })
}

fn not_found(name: &str, skills: &[&Skill]) -> Diagnostic {
    let d = Diagnostic::error("skill-not-found", format!("no skill named `{name}`"))
        .exit(Exit::NotFound);
    let lower = name.to_lowercase();
    let mut near: Vec<&str> = skills
        .iter()
        .map(|s| s.name.as_str())
        .filter(|n| n.to_lowercase().contains(&lower) || lower.contains(&n.to_lowercase()))
        .collect();
    near.dedup();
    match near.as_slice() {
        [] => d.help("run `the skills list` to see the available skills"),
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

fn show_report(s: &Skill) -> Report {
    let mut rec = Record::new()
        .field("name", s.name.clone())
        .field("description", Cell::opt(s.description().map(str::trim)))
        .field("scope", s.scope.as_str())
        .field("path", s.path.clone());
    if s.path != s.canonical {
        rec = rec.field("resolved", s.canonical.clone());
    }
    for (k, v) in &s.parsed.frontmatter {
        if k != "name" && k != "description" {
            let v = match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            rec = rec.field(k.clone(), v);
        }
    }
    let files = bundled_files(s.dir());
    if !files.is_empty() {
        rec = rec.field("files", files);
    }
    rec = rec
        .json_only(
            "frontmatter",
            Cell::Json(Value::Object(s.parsed.frontmatter.clone())),
        )
        .json_only("lint", Cell::Json(json!(s.lint)));

    let mut agents = Table::new(vec![
        Column::new("agent", "AGENT"),
        Column::new("status", "STATUS"),
        Column::new("detail", "DETAIL"),
        Column::json_only("name"),
        Column::json_only("problem"),
    ]);
    for v in &s.agents {
        let detail = match &v.status {
            Status::Loaded if v.name != s.name => format!("as `{}`", v.name),
            Status::Loaded => String::new(),
            Status::Shadowed { by } => format!("by {}", by.display()),
            Status::Conflict { with } => {
                format!(
                    "with {}",
                    with.iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
            Status::Skipped(p) => p.message.clone(),
            Status::Unknown(p) => format!("unknown: {}", p.message),
        };
        let problem = match &v.status {
            Status::Skipped(p) | Status::Unknown(p) => Cell::Json(json!(p)),
            _ => Cell::Empty,
        };
        agents.push(vec![
            v.agent.as_str().into(),
            status_cell(&v.status),
            if detail.is_empty() {
                Cell::Empty
            } else {
                detail.into()
            },
            v.name.clone().into(),
            problem,
        ]);
    }

    let mut locations = Table::new(vec![
        Column::new("path", "LOCATION"),
        Column::new("agents", "AGENTS"),
        Column::new("links_to", "LINKS TO"),
        Column::json_only("scope"),
        Column::json_only("link"),
    ]);
    for (l, by) in s.paths() {
        locations.push(vec![
            l.path.clone().into(),
            agent_list(&by).into(),
            l.link
                .as_ref()
                .map_or(Cell::Empty, |k| k.target.clone().into()),
            l.scope.as_str().into(),
            l.link
                .as_ref()
                .map_or(Cell::Empty, |k| Cell::Json(json!(k))),
        ]);
    }

    Report::new()
        .record("skill", rec)
        .table("agents", agents)
        .table("locations", locations)
        .text("body", s.parsed.body.clone())
}

fn build_lint(
    target: Option<&str>,
    search: &SearchArgs,
    all: bool,
    advice: bool,
    beta: bool,
) -> Result<Built, Diagnostic> {
    // A path-like target (`.`, `./x`, `../p`, `/abs`) is a scan root (lint that directory/project); a
    // bare word is a skill-name filter (lint that one skill).
    let (search_owned, name): (SearchArgs, Option<&str>) = match target {
        Some(t) if is_path_like(t) => (with_root(search, PathBuf::from(t)), None),
        other => (search.clone(), other),
    };
    let search = &search_owned;
    let (cat, mut notices) = catalog(search, true)?;
    let candidates = selected(&cat, search);
    let skills: Vec<&Skill> = match name {
        Some(target) => {
            let matches: Vec<&Skill> = candidates
                .iter()
                .copied()
                .filter(|s| {
                    s.name == target
                        || s.agents.iter().any(|v| v.name == target)
                        || s.locations
                            .iter()
                            .any(|l| dir_name(&l.path).as_deref() == Some(target))
                })
                .collect();
            if matches.is_empty() {
                return Err(not_found(target, &candidates));
            }
            matches
        }
        None => candidates,
    };

    if skills.is_empty() {
        notices.push(no_skills_note(&cat));
    }

    let mut findings = collect_diagnostics(&skills);
    // Corpus-level beta rules (`redundant-with`, `superseded-by`): computed always so a graduated rule
    // works by default, then gated on each rule's `stability` in the registry — shown only with
    // `--beta` while still staged. Graduation is flipping one field in `the-skills::rules`.
    for (skill_name, path, p) in corpus_findings(&skills) {
        findings.push(
            Diagnostic::warning(
                p.code,
                format!("skill `{skill_name}`: {}", issue_label(p.code)),
            )
            .context(path.display().to_string())
            .cause(p.message)
            .help(p.fix),
        );
    }
    findings.retain(|d| beta || !crate::rules::is_beta(d.code));

    // Result first: the table on stdout so a reader sees the overview before the issues. `--advice`
    // adds a fix-advice table (also a stdout result: the requested fix plan, per SPEC-0001 OUT)
    // classing each issue auto / assisted / manual, writing nothing.
    let mut report = Report::new().table("skills", lint_table(&skills, beta));
    if advice {
        report = report.table("advice", advice_table(&fix::plan_advice(&skills)));
    }
    let report = report
        .json_only("diagnostics", Block::Table(diagnostics_table(&findings)))
        .json_only("searched", Block::Table(roots_table(&cat.roots)));

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

/// Cap on urgent issues shown after the table before the summary suggests `--all`.
const ISSUE_LIMIT: usize = 5;

fn lint_table(skills: &[&Skill], beta: bool) -> Table {
    let mut t = Table::new(vec![
        Column::new("name", "NAME"),
        Column::new("scope", "SCOPE"),
        Column::new("status", "STATUS"),
        Column::new("agents", "AGENTS"),
        Column::new("issues", "ISSUES"),
        Column::json_only("path"),
        Column::json_only("dir"),
        Column::json_only("lint"),
    ]);

    for s in skills {
        // Beta rules (staged behind `--beta`) are hidden from the result table by default, matching
        // how the diagnostics stream is gated; without this a per-skill beta rule would leak here.
        let lint: Vec<&Problem> = s
            .lint
            .iter()
            .filter(|p| beta || !crate::rules::is_beta(p.code))
            .collect();
        let mut codes: Vec<String> = Vec::new();
        for v in &s.agents {
            match &v.status {
                Status::Skipped(p) | Status::Unknown(p) => {
                    if !codes.contains(&p.code.to_owned()) {
                        codes.push(p.code.to_owned());
                    }
                }
                Status::Conflict { .. } => {
                    if !codes.iter().any(|c| c == "skill-conflict") {
                        codes.push("skill-conflict".to_owned());
                    }
                }
                Status::Loaded | Status::Shadowed { .. } => {}
            }
        }
        for p in &lint {
            if !codes.iter().any(|c| c == p.code) {
                codes.push(p.code.to_owned());
            }
        }
        // A frontmatter parse error (e.g. duplicate-frontmatter-key, frontmatter-invalid) otherwise
        // surfaces only when an installed agent skips the skill; include it directly so linting a
        // directory still flags it. Deduped: a real agent-root skill already has it from the skip above.
        let fe = s.parsed.frontmatter_error.as_ref();
        if let Some(p) = fe
            && !codes.iter().any(|c| c == p.code)
        {
            codes.push(p.code.to_owned());
        }

        let has_error = lint.iter().any(|p| p.is_error()) || fe.is_some_and(Problem::is_error);
        let has_warning = fe.is_some_and(|p| !p.is_error())
            || s.agents.iter().any(|v| {
                matches!(
                    v.status,
                    Status::Skipped(_) | Status::Conflict { .. } | Status::Unknown(_)
                )
            })
            || lint
                .iter()
                .any(|p| !p.is_error() && p.kind() != ProblemKind::Spec);
        let status = if has_error {
            Cell::toned(Tone::Fail, "error")
        } else if has_warning {
            Cell::toned(Tone::Warn, "warning")
        } else if !lint.is_empty() {
            Cell::toned(Tone::Dim, "note")
        } else {
            Cell::toned(Tone::Ok, "ok")
        };

        t.push(vec![
            s.name.clone().into(),
            s.scope.as_str().into(),
            status,
            agent_list(&s.loaded_by()).into(),
            codes.into(),
            s.path.clone().into(),
            s.dir().to_path_buf().into(),
            Cell::Json(json!(lint)),
        ]);
    }
    t
}

/// The `--advice` fix plan: one row per issue with its repair class and recommended approach. The
/// rows are sorted safest-first (auto, then assisted, then manual) so a reader scans the mechanical
/// wins before the judgement calls; `safe` and the (currently always-null) `command` are JSON-only so
/// an agent can branch on them. The advice is computed by [`fix::plan_advice`]; this only renders it.
fn advice_table(advice: &[Advice]) -> Table {
    fn class_rank(c: FixClass) -> u8 {
        match c {
            FixClass::Auto => 0,
            FixClass::Assisted => 1,
            FixClass::Manual => 2,
        }
    }
    let mut rows: Vec<&Advice> = advice.iter().collect();
    rows.sort_by_key(|a| class_rank(a.class));

    let mut t = Table::new(vec![
        Column::new("name", "NAME"),
        Column::new("class", "CLASS"),
        Column::new("code", "CODE"),
        Column::new("strategy", "FIX"),
        Column::json_only("path"),
        Column::json_only("safe"),
        Column::json_only("command"),
    ]);
    for a in rows {
        let tone = match a.class {
            FixClass::Auto => Tone::Ok,
            FixClass::Assisted => Tone::Warn,
            FixClass::Manual => Tone::Dim,
        };
        t.push(vec![
            a.skill.clone().into(),
            Cell::toned(tone, a.class.as_str()),
            a.code.into(),
            a.strategy.clone().into(),
            a.path.clone().into(),
            Cell::Bool(a.safe()),
            // No apply path exists yet (Phase 2 `--fix`), so there is honestly no command to name
            // here rather than a fabricated one (the no-false-hint norm).
            Cell::Empty,
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

    fn plain_results(built: Built) -> SkillsResults {
        SkillsResults {
            built,
            h: Human {
                term: Term {
                    color: false,
                    tty: false,
                },
                header: true,
            },
            err_term: Term {
                color: false,
                tty: false,
            },
        }
    }

    fn render_to_string(res: &SkillsResults, form: Form) -> String {
        let mut out = Vec::new();
        res.render(&mut out, form).unwrap();
        String::from_utf8(out).unwrap()
    }

    fn diags_to_string(res: &SkillsResults, format: Format) -> String {
        let mut err = Vec::new();
        res.render_diagnostics(&mut err, format).unwrap();
        String::from_utf8(err).unwrap()
    }

    // The descriptor table must match the clap subcommand tree: a command added to one but not the
    // other is a registration bug. Mirrors cli's `catalog_matches_the_clap_tree`.
    #[test]
    fn descriptors_match_the_clap_tree() {
        let root = Skills.augment(Command::new("the"));
        let skills_cmd = root
            .get_subcommands()
            .find(|c| c.get_name() == "skills")
            .expect("skills subcommand folded onto the root");

        let mut real = vec!["skills".to_string()];
        for sub in skills_cmd.get_subcommands().filter(|c| !c.is_hide_set()) {
            if sub.get_name() == "help" {
                continue;
            }
            real.push(format!("skills {}", sub.get_name()));
        }
        real.sort();

        let mut declared: Vec<String> = DESCRIPTORS.iter().map(|d| d.path.to_string()).collect();
        declared.sort();

        assert_eq!(
            declared, real,
            "skills descriptors must match the clap subcommand tree exactly"
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
            assert_eq!(d.group, "skills");
        }
    }

    #[test]
    fn render_emits_the_report_as_plain_and_json() {
        let built = Built {
            report: Report::new().text("msg", "hello world"),
            notices: Vec::new(),
            findings: Vec::new(),
            mode: DiagMode::Stream,
            exit: 0,
        };
        let res = plain_results(built);
        assert_eq!(render_to_string(&res, Form::Plain).trim(), "hello world");
        // JSON renders the same block as a typed document (the one section collapses to its value).
        let json = render_to_string(&res, Form::Json);
        assert!(json.contains("hello world"));
        assert!(json.trim_start().starts_with('{') || json.trim_start().starts_with('"'));
        assert_eq!(res.exit_code(), 0);
    }

    #[test]
    fn stream_mode_emits_each_notice_and_finding() {
        let built = Built {
            report: Report::new(),
            notices: vec![Diagnostic::warning("home-unset", "HOME is not set")],
            findings: vec![Diagnostic::warning("broken-link", "a link is broken")],
            mode: DiagMode::Stream,
            exit: 0,
        };
        let res = plain_results(built);
        let err = diags_to_string(&res, Format::Human);
        assert!(err.contains("HOME is not set"), "notice streamed");
        assert!(err.contains("a link is broken"), "finding streamed");
    }

    #[test]
    fn summarize_mode_caps_findings_and_points_to_all() {
        // Six findings, summarize mode with all=false caps the shown list and names `--all`.
        let findings: Vec<Diagnostic> = (0..6)
            .map(|i| Diagnostic::warning("broken-link", format!("issue {i}")))
            .collect();
        let built = Built {
            report: Report::new(),
            notices: Vec::new(),
            findings,
            mode: DiagMode::Summarize { all: false },
            exit: Exit::Generic as u8,
        };
        let res = plain_results(built);
        let err = diags_to_string(&res, Format::Human);
        assert!(
            err.contains("the skills lint --all"),
            "names the --all hint"
        );
        assert_eq!(res.exit_code(), Exit::Generic as u8);
    }

    #[test]
    fn json_findings_stream_regardless_of_mode() {
        let built = Built {
            report: Report::new(),
            notices: Vec::new(),
            findings: vec![Diagnostic::warning("broken-link", "a link is broken")],
            mode: DiagMode::Summarize { all: false },
            exit: Exit::Generic as u8,
        };
        let res = plain_results(built);
        let err = diags_to_string(&res, Format::Json);
        assert!(err.contains("broken-link"), "JSON streams each finding");
        assert!(err.contains('{'), "JSON object per line");
    }

    // An in-process run of the real pipeline: parse → execute → render to buffers, against a
    // hermetic temp skill dir. This exercises execute/build_lint/catalog/SkillsResults in-process
    // (the CLI integration tests exercise the same path in a subprocess, which coverage can't see).
    #[test]
    fn execute_lints_a_dir_in_process() {
        use std::fs;
        let dir = std::env::temp_dir().join(format!("the-cmd-exec-{}", std::process::id()));
        let skill = dir.join("myskill");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&skill).unwrap();
        fs::write(
            skill.join("SKILL.md"),
            "---\nname: myskill\ndescription: PDF tools\n---\n# Body\n",
        )
        .unwrap();

        let root = Skills.augment(Command::new("the"));
        let m = root
            .try_get_matches_from([
                "the",
                "skills",
                "lint",
                "--scope",
                "dir",
                "--dir",
                dir.to_str().unwrap(),
            ])
            .unwrap();
        let (_id, sub) = m.subcommand().unwrap();
        let plain = the_output::command::StreamState {
            color: false,
            tty: false,
        };
        let ctx = Ctx::new(Format::Human, false, plain, plain);
        let inv = Invocation {
            target: "skills",
            matches: sub.clone(),
        };
        let results = Skills.execute(&inv, &ctx).unwrap();

        let mut out = Vec::new();
        results.render(&mut out, Form::Plain).unwrap();
        assert!(
            String::from_utf8(out).unwrap().contains("myskill"),
            "the lint table lists the scanned skill"
        );
        // A terse description ("PDF tools") is flagged as a spec note, so there is a finding and a
        // stderr diagnostic naming the skill (notes do not raise the exit code).
        assert!(
            !results.diagnostics().is_empty(),
            "the terse description is a finding"
        );
        let mut err = Vec::new();
        results.render_diagnostics(&mut err, Format::Human).unwrap();
        assert!(
            String::from_utf8(err).unwrap().contains("myskill"),
            "the finding names the skill on stderr"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    // The standard command-coverage test: one argv matrix (subcommands x flags), each line asserted
    // stable across the plain/ansi/json forms by the shared conformance harness. This is how a command
    // crate proves flexibility cheaply; add a line here when a new subcommand or flag lands.
    #[test]
    fn skills_matrix_is_form_stable() {
        use std::fs;
        use the_output::command::{Registry, conformance};

        let dir = std::env::temp_dir().join(format!("the-skills-matrix-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        for (name, desc) in [
            (
                "alpha-skill",
                "Work with alpha structured documents end to end",
            ),
            ("beta", "PDF tools"),
        ] {
            let s = dir.join(name);
            fs::create_dir_all(&s).unwrap();
            fs::write(
                s.join("SKILL.md"),
                format!("---\nname: {name}\ndescription: {desc}\n---\n# Body\n"),
            )
            .unwrap();
        }
        let d = dir.to_str().unwrap();
        let reg = Registry::new().register(Box::new(Skills));

        let matrix: &[&[&str]] = &[
            &["the", "skills", "--scope", "dir", "--dir", d],
            &["the", "skills", "--scope", "dir", d], // a leading path in the subcommand slot
            &["the", "skills", "list", "--scope", "dir", "--dir", d],
            &[
                "the", "skills", "list", "--prose", "--scope", "dir", "--dir", d,
            ],
            &[
                "the",
                "skills",
                "list",
                "--by-scope",
                "--scope",
                "dir",
                "--dir",
                d,
            ],
            &[
                "the",
                "skills",
                "list",
                "--no-header",
                "--scope",
                "dir",
                "--dir",
                d,
            ],
            &["the", "skills", "dirs", "--scope", "dir", "--dir", d],
            &["the", "skills", "lint", "--scope", "dir", "--dir", d],
            &[
                "the", "skills", "lint", "--all", "--scope", "dir", "--dir", d,
            ],
            &[
                "the", "skills", "lint", "--advice", "--scope", "dir", "--dir", d,
            ],
            &[
                "the", "skills", "lint", "--beta", "--scope", "dir", "--dir", d,
            ],
            &[
                "the", "skills", "show", "beta", "--scope", "dir", "--dir", d,
            ],
        ];
        conformance::assert_matrix(&reg, matrix);
        let _ = fs::remove_dir_all(&dir);
    }

    // Parse an argv against the real skills grammar and return the "skills" submatches for execute().
    #[cfg(test)]
    fn skills_matches(argv: &[&str]) -> clap::ArgMatches {
        let m = Skills
            .augment(Command::new("the"))
            .try_get_matches_from(argv)
            .unwrap();
        m.subcommand().unwrap().1.clone()
    }

    #[cfg(test)]
    fn plain_ctx() -> Ctx {
        let plain = the_output::command::StreamState {
            color: false,
            tty: false,
        };
        Ctx::new(Format::Human, false, plain, plain)
    }

    #[test]
    fn a_leading_path_is_scanned_not_rejected() {
        // Regression: `the skills <path>` with no subcommand used to error `unrecognized subcommand`.
        use std::fs;
        let dir = std::env::temp_dir().join(format!("the-skills-leadpath-{}", std::process::id()));
        let skill = dir.join("myskill");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&skill).unwrap();
        fs::write(
            skill.join("SKILL.md"),
            "---\nname: myskill\ndescription: Work with structured documents end to end\n---\n# Body\n",
        )
        .unwrap();

        // `--scope dir` (a top-level flag, parsed before the external path) keeps the scan hermetic.
        let sub = skills_matches(&["the", "skills", "--scope", "dir", dir.to_str().unwrap()]);
        let results = Skills
            .execute(
                &Invocation {
                    target: "skills",
                    matches: sub,
                },
                &plain_ctx(),
            )
            .unwrap();
        let mut out = Vec::new();
        results.render(&mut out, Form::Plain).unwrap();
        assert!(
            String::from_utf8(out).unwrap().contains("myskill"),
            "a leading path scans that directory"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_non_path_in_the_subcommand_slot_is_a_usage_error() {
        let sub = skills_matches(&["the", "skills", "bogus"]);
        let err = Skills
            .execute(
                &Invocation {
                    target: "skills",
                    matches: sub,
                },
                &plain_ctx(),
            )
            .err()
            .expect("a non-path subcommand is an error");
        assert_eq!(err.code, "unknown-subcommand");
        assert_eq!(err.exit, Exit::Usage);
    }

    #[test]
    fn is_path_like_distinguishes_paths_from_skill_names() {
        assert!(is_path_like("."));
        assert!(is_path_like(".."));
        assert!(is_path_like("./x"));
        assert!(is_path_like("../p"));
        assert!(is_path_like("/abs/dir"));
        assert!(is_path_like("~/skills"));
        assert!(!is_path_like("my-skill"));
        assert!(!is_path_like("pdf"));
    }
}
