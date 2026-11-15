//! `the skills list | show | dirs | lint`. Each builds a [`Report`]; rendering is format-independent.

use std::io::Write;
use std::path::Path;
use std::process::ExitCode;

use serde_json::{Value, json};

use super::fix::{self, Advice, FixClass};
use super::installed::installed_agents;
use super::locate::{Env, plan, plan_for};
use super::parse::{Problem, ProblemKind};
use super::resolve::{Catalog, RootInfo, Skill, Status, build, build_for};
use super::{Agent, Scope, bundled_files};
use crate::cli::{ListForm, SearchArgs, SkillsArgs, SkillsCmd};
use crate::diag::{Diagnostic, Exit, Level};
use crate::output::{self, Format};
use crate::style;
use crate::table::{Block, Cell, Column, Human, Record, Report, Table, Tone};

const DESCRIPTION_WIDTH: usize = 72;

pub fn run(args: SkillsArgs, format: Format, h: Human) -> Result<ExitCode, Diagnostic> {
    match args.command {
        Some(SkillsCmd::List(list_args)) => {
            list(&list_args.search, list_args.presentation(), format, h)?;
            Ok(ExitCode::SUCCESS)
        }
        Some(SkillsCmd::Show { name, search }) => {
            show(&name, &search, format, h)?;
            Ok(ExitCode::SUCCESS)
        }
        Some(SkillsCmd::Dirs(search)) => {
            dirs(&search, format, h)?;
            Ok(ExitCode::SUCCESS)
        }
        Some(SkillsCmd::Lint {
            name,
            search,
            all,
            advice,
        }) => lint(name.as_deref(), &search, all, advice, format, h),
        None => {
            let form = if args.by_scope {
                ListForm::ByScope
            } else {
                ListForm::Prose
            };
            list(&args.search, form, format, h)?;
            Ok(ExitCode::SUCCESS)
        }
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
fn catalog(
    search: &SearchArgs,
    format: Format,
    only_installed: bool,
) -> Result<Catalog, Diagnostic> {
    let env = Env::from_process().map_err(|e| {
        Diagnostic::error("cwd-unavailable", "cannot determine the current directory")
            .cause(e.to_string())
            .help("run `the` from an existing, readable directory")
    })?;
    if env.home.is_none() {
        Diagnostic::warning(
            "home-unset",
            "HOME is not set; skipping user skill directories",
        )
        .help("set HOME, or pass a directory explicitly with `--dir <PATH>`")
        .emit(format);
    }
    let cat = if only_installed {
        let mut agents = installed_agents(&env);
        if let Some(a) = search.agent
            && !agents.contains(&a)
        {
            agents.push(a);
            agents.sort();
        }
        build_for(&plan_for(&env, &agents, &search.dirs), &agents)
    } else {
        build(&plan(&env, &search.dirs))
    };
    for r in cat
        .roots
        .iter()
        .filter(|r| r.agents.is_empty() && !r.exists)
    {
        Diagnostic::warning("dir-not-found", "`--dir` is not a directory")
            .context(r.path.display().to_string())
            .emit(format);
    }
    for e in &cat.errors {
        Diagnostic::warning("unreadable", "could not read a skill path; skipped it")
            .context(e.path.display().to_string())
            .cause(e.error.to_string())
            .emit(format);
    }
    Ok(cat)
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
            && let Some(alias) = super::parse::claude_alias(&s.parsed)
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
            out.push(
                d.context(s.path.display().to_string())
                    .cause(p.message.clone())
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
                        .map(|p| p.message.as_str())
                        .collect::<Vec<_>>()
                        .join("; "),
                )
                .help(unique_fixes(spec_lint.iter().map(|p| p.fix))),
            );
        }
    }
    out
}

/// Emit diagnostics for `skills` to stderr.
fn diagnose(skills: &[&Skill], format: Format) {
    for d in collect_diagnostics(skills) {
        d.emit(format);
    }
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

fn list(search: &SearchArgs, form: ListForm, format: Format, h: Human) -> Result<(), Diagnostic> {
    let cat = catalog(search, format, true)?;
    let skills = selected(&cat, search);
    if matches!(form, ListForm::Tabular) || matches!(format, Format::Json) {
        diagnose(&skills, format);
    }
    if skills.is_empty() {
        Diagnostic::note("no-skills", "no skills found")
            .cause(format!("{} of {} searched directories exist", cat.roots.iter().filter(|r| r.exists).count(), cat.roots.len()))
            .help("`the skills dirs` shows where each agent looks; `--dir <PATH>` searches another directory")
            .emit(format);
    }

    let report = present_list(&skills, &cat.agents, &cat.roots, form, h);
    output::print(&report.render(format, h))
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
    match code {
        "no-frontmatter" => "missing YAML frontmatter",
        "frontmatter-unterminated" => "unterminated YAML frontmatter",
        "frontmatter-invalid" => "invalid YAML frontmatter",
        "name-missing" => "missing skill name",
        "name-has-slash" => "skill name contains '/'",
        "description-not-string" => "non-string description",
        "symlink-escape" => "symlink escapes skill directory",
        "trojan-source" => "hidden control or bidirectional character",
        "exposed-secret" => "exposed secret or credential literal",
        "skill-conflict" => "duplicate skill name conflict",
        "broken-link" => "broken relative link in SKILL.md",
        "link-escapes-skill" => "relative link escapes skill directory",
        "broken-symlink" => "dangling symlink in skill directory",
        "unrestricted-bash" => "unrestricted Bash tool permission",
        "name-format" => "invalid skill name format",
        "name-mismatch" => "skill name differs from directory",
        "description-missing" => "missing description",
        "description-too-long" => "description exceeds 1024 characters",
        "compatibility-invalid" => "invalid or overlong compatibility field",
        "license-not-string" => "non-string license field",
        "metadata-not-map" => "metadata is not a flat scalar map",
        "allowed-tools-invalid" => "invalid allowed-tools field",
        "unknown-frontmatter-key" => "unknown frontmatter key",
        _ => "skill format issue",
    }
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

fn dirs(search: &SearchArgs, format: Format, h: Human) -> Result<(), Diagnostic> {
    let cat = catalog(search, format, false)?;
    let roots: Vec<RootInfo> = cat
        .roots
        .into_iter()
        .filter(|r| search.scope.is_none_or(|s| r.scope == s))
        .filter(|r| search.agent.is_none_or(|a| r.agents.contains(&a)))
        .collect();
    output::print(
        &Report::new()
            .table("dirs", roots_table(&roots))
            .render(format, h),
    )
}

fn dir_name(skill_md: &Path) -> Option<String> {
    skill_md
        .parent()?
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
}

fn show(name: &str, search: &SearchArgs, format: Format, h: Human) -> Result<(), Diagnostic> {
    let cat = catalog(search, format, true)?;
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

    diagnose(&[skill], format);
    output::print(&show_report(skill).render(format, h))
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

fn lint(
    name: Option<&str>,
    search: &SearchArgs,
    all: bool,
    advice: bool,
    format: Format,
    h: Human,
) -> Result<ExitCode, Diagnostic> {
    let cat = catalog(search, format, true)?;
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
        Diagnostic::note("no-skills", "no skills found")
            .cause(format!(
                "{} of {} searched directories exist",
                cat.roots.iter().filter(|r| r.exists).count(),
                cat.roots.len()
            ))
            .help("`the skills dirs` shows where each agent looks; `--dir <PATH>` searches another directory")
            .emit(format);
    }

    let diags = collect_diagnostics(&skills);

    // Result first: print the table on stdout so a reader sees the overview before the issues.
    // `--advice` adds a fix-advice table (also a stdout result: it is the requested fix plan, per
    // SPEC-0001 OUT) classing each issue auto / assisted / manual, writing nothing.
    let mut report = Report::new().table("skills", lint_table(&skills));
    if advice {
        report = report.table("advice", advice_table(&fix::plan_advice(&skills)));
    }
    let report = report
        .json_only("diagnostics", Block::Table(diagnostics_table(&diags)))
        .json_only("searched", Block::Table(roots_table(&cat.roots)));
    output::print(&report.render(format, h))?;

    // Then the diagnostics on stderr: JSON streams all of them; human shows the most urgent few
    // (each with its file link and fix) plus a status line, directing to `--all` for the rest.
    match format {
        Format::Json => {
            for d in &diags {
                d.emit(format);
            }
        }
        Format::Human if !diags.is_empty() => {
            // Shared renderer (the-output): the most urgent few, then a `= status:` count line.
            let limit = if all { None } else { Some(ISSUE_LIMIT) };
            let text = crate::diag::summarize(&diags, limit)
                .render(style::Term::stderr(), Some("the skills lint --all"));
            let _ = std::io::stderr().lock().write_all(text.as_bytes());
        }
        Format::Human => {}
    }

    let has_error_or_warning = diags
        .iter()
        .any(|d| matches!(d.level, Level::Error | Level::Warning));
    if has_error_or_warning {
        Ok(ExitCode::from(Exit::Generic as u8))
    } else {
        Ok(ExitCode::SUCCESS)
    }
}

/// Cap on urgent issues shown after the table before the summary suggests `--all`.
const ISSUE_LIMIT: usize = 5;

fn lint_table(skills: &[&Skill]) -> Table {
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
        for p in &s.lint {
            if !codes.iter().any(|c| c == p.code) {
                codes.push(p.code.to_owned());
            }
        }

        let has_error = s.lint.iter().any(Problem::is_error);
        let has_warning = s.agents.iter().any(|v| {
            matches!(
                v.status,
                Status::Skipped(_) | Status::Conflict { .. } | Status::Unknown(_)
            )
        }) || s
            .lint
            .iter()
            .any(|p| !p.is_error() && p.kind() != ProblemKind::Spec);
        let status = if has_error {
            Cell::toned(Tone::Fail, "error")
        } else if has_warning {
            Cell::toned(Tone::Warn, "warning")
        } else if !s.lint.is_empty() {
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
            Cell::Json(json!(s.lint)),
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
