//! `the skills list | show | dirs`. Each builds a [`Report`]; rendering is format-independent.

use std::path::Path;

use serde_json::{Value, json};

use super::locate::{Env, plan};
use super::resolve::{Catalog, RootInfo, Skill, Status, build};
use super::{Agent, bundled_files};
use crate::cli::{SearchArgs, SkillsCmd};
use crate::diag::{Diagnostic, Exit};
use crate::output::{self, Format};
use crate::table::{Block, Cell, Column, Human, Record, Report, Table, Tone};

const DESCRIPTION_WIDTH: usize = 72;

pub fn run(cmd: SkillsCmd, format: Format, h: Human) -> Result<(), Diagnostic> {
    match cmd {
        SkillsCmd::List(search) => list(&search, format, h),
        SkillsCmd::Show { name, search } => show(&name, &search, format, h),
        SkillsCmd::Dirs(search) => dirs(&search, format, h),
    }
}

fn status_cell(s: &Status) -> Cell {
    let tone = match s {
        Status::Loaded => Tone::Ok,
        Status::Shadowed { .. } => Tone::Dim,
        Status::Conflict { .. } | Status::Skipped(_) => Tone::Warn,
    };
    Cell::toned(tone, s.as_str())
}

/// Plan and scan; report unusable inputs on stderr.
fn catalog(search: &SearchArgs, format: Format) -> Result<Catalog, Diagnostic> {
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
    let cat = build(&plan(&env, &search.dirs));
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
            search
                .agent
                .is_none_or(|a| s.view(a).is_some_and(|v| v.status.is_loaded()))
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

/// Warnings for what makes an agent skip or mis-load a skill; notes for format lint.
fn diagnose(skills: &[&Skill], format: Format) {
    let mut conflicts_seen: Vec<&str> = Vec::new();
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
            Diagnostic::warning("skill-skipped", format!("{who} skip skill `{}`", s.name))
                .context(s.path.display().to_string())
                .cause(causes.join("; "))
                .help(unique_fixes(skipped.iter().map(|(_, p)| p.fix)))
                .emit(format);
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
            d.help("rename all but one, or remove the duplicates")
                .emit(format);
        }

        if !s.lint.is_empty() {
            Diagnostic::note(
                "skill-lint",
                format!("skill `{}` does not follow the Agent Skills format", s.name),
            )
            .context(s.path.display().to_string())
            .cause(
                s.lint
                    .iter()
                    .map(|p| p.message.as_str())
                    .collect::<Vec<_>>()
                    .join("; "),
            )
            .help(unique_fixes(s.lint.iter().map(|p| p.fix)))
            .emit(format);
        }
    }
}

fn status_json(s: &Skill) -> Value {
    let mut m = serde_json::Map::new();
    for v in &s.agents {
        let mut o = json!({ "status": v.status.as_str(), "name": v.name });
        match &v.status {
            Status::Shadowed { by } => o["by"] = json!(by),
            Status::Conflict { with } => o["with"] = json!(with),
            Status::Skipped(p) => o["problem"] = json!(p),
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

fn list(search: &SearchArgs, format: Format, h: Human) -> Result<(), Diagnostic> {
    let cat = catalog(search, format)?;
    let skills = selected(&cat, search);
    diagnose(&skills, format);
    if skills.is_empty() {
        Diagnostic::note("no-skills", "no skills found")
            .cause(format!("{} of {} searched directories exist", cat.roots.iter().filter(|r| r.exists).count(), cat.roots.len()))
            .help("`the skills dirs` shows where each agent looks; `--dir <PATH>` searches another directory")
            .emit(format);
    }

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
    for s in &skills {
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
    let report = Report::new()
        .table("skills", t)
        .json_only("searched", Block::Table(roots_table(&cat.roots)));
    output::print(&report.render(format, h))
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
    let cat = catalog(search, format)?;
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
    let cat = catalog(search, format)?;
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
        };
        let problem = match &v.status {
            Status::Skipped(p) => Cell::Json(json!(p)),
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
