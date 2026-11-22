//! Build the catalog: one entry per distinct SKILL.md (by canonical path, so a skill shared between
//! agents through symlinks is one entry with several locations), with each agent's verdict.
//!
//! Duplicate names, per agent: Claude Code ranks managed > personal > project (its roots are planned
//! in that order) and Goose keeps the first found, so for both the first in search order wins and
//! the rest are shadowed. OpenCode lets the last parsed win, but parses concurrently, so which of
//! several same-named skills it keeps is unpredictable: all of them are reported as a conflict.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::links::check_links;
use crate::locate::SearchRoot;
use crate::orphans::check_orphans;
use crate::parse::{Load, Parsed, Problem, claude_alias, display_name, lint, loads, parse};
use crate::scan::{Link, link_in, scan};
use crate::smoke::check_smoke;
use crate::{Agent, Scope};

/// One way a skill is reached.
#[derive(Debug, Clone)]
pub struct Location {
    /// `None` for a `--dir` root.
    pub agent: Option<Agent>,
    pub scope: Scope,
    /// The SKILL.md path as reached (symlinks not resolved).
    pub path: PathBuf,
    pub link: Option<Link>,
    rank: (usize, usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Loaded,
    /// A same-named skill earlier in this agent's precedence wins.
    Shadowed {
        by: PathBuf,
    },
    /// OpenCode: several skills share this name; it keeps one of them unpredictably.
    Conflict {
        with: Vec<PathBuf>,
    },
    Skipped(Problem),
    /// `the` cannot predict this agent's verdict (Gemini with a broken-but-present
    /// frontmatter block its unmodeled lenient parser may recover). The `Problem`
    /// is the malformed frontmatter, still reported so it gets fixed.
    Unknown(Problem),
}

impl Status {
    pub fn as_str(&self) -> &'static str {
        match self {
            Status::Loaded => "loaded",
            Status::Shadowed { .. } => "shadowed",
            Status::Conflict { .. } => "conflict",
            Status::Skipped(_) => "skipped",
            Status::Unknown(_) => "unknown",
        }
    }

    /// The agent may load this skill (a conflict loads one of the contenders).
    /// Unknown is not loaded: `the` makes no claim either way.
    pub fn is_loaded(&self) -> bool {
        matches!(self, Status::Loaded | Status::Conflict { .. })
    }
}

#[derive(Debug, Clone)]
pub struct AgentView {
    pub agent: Agent,
    /// The name the agent knows it by (or would, if it loaded it).
    pub name: String,
    pub status: Status,
}

#[derive(Debug, Clone)]
pub struct Skill {
    /// Frontmatter `name`, else the directory name.
    pub name: String,
    pub scope: Scope,
    /// Preferred path: the first location not reached through a symlink.
    pub path: PathBuf,
    /// The resolved SKILL.md.
    pub canonical: PathBuf,
    pub parsed: Parsed,
    pub locations: Vec<Location>,
    pub agents: Vec<AgentView>,
    pub lint: Vec<Problem>,
    /// Other skills loaded by Claude Code whose frontmatter `name` claims the same `/` command; only
    /// one of them gets it (the rest stay reachable by directory name).
    pub claude_alias_with: Vec<PathBuf>,
}

impl Skill {
    pub fn description(&self) -> Option<&str> {
        self.parsed.str_field("description")
    }

    /// The resolved skill directory.
    pub fn dir(&self) -> &Path {
        self.canonical.parent().unwrap_or(&self.canonical)
    }

    pub fn view(&self, agent: Agent) -> Option<&AgentView> {
        self.agents.iter().find(|v| v.agent == agent)
    }

    pub fn loaded_by(&self) -> Vec<Agent> {
        self.agents
            .iter()
            .filter(|v| v.status.is_loaded())
            .map(|v| v.agent)
            .collect()
    }

    /// Reached through a symlink from at least one location.
    pub fn linked(&self) -> bool {
        self.locations.iter().any(|l| l.link.is_some())
    }

    /// Distinct reached paths (a root searched by several agents gives one path for all of them),
    /// with the agents reaching each.
    pub fn paths(&self) -> Vec<(&Location, Vec<Agent>)> {
        let mut out: Vec<(&Location, Vec<Agent>)> = Vec::new();
        for l in &self.locations {
            match out.iter_mut().find(|(o, _)| o.path == l.path) {
                Some((_, agents)) => agents.extend(l.agent.filter(|a| !agents.contains(a))),
                None => out.push((l, l.agent.into_iter().collect())),
            }
        }
        out
    }
}

/// A searched root, merged across the agents that search it.
#[derive(Debug, Clone)]
pub struct RootInfo {
    pub path: PathBuf,
    pub scope: Scope,
    pub agents: Vec<Agent>,
    pub exists: bool,
    pub link: Option<Link>,
}

#[derive(Debug)]
pub struct ScanError {
    pub path: PathBuf,
    pub error: io::Error,
}

#[derive(Debug)]
pub struct Catalog {
    pub agents: Vec<Agent>,
    pub roots: Vec<RootInfo>,
    pub skills: Vec<Skill>,
    pub errors: Vec<ScanError>,
}

fn dir_name(skill_md: &Path) -> String {
    skill_md
        .parent()
        .and_then(Path::file_name)
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

pub fn build(plan: &[SearchRoot]) -> Catalog {
    build_for(plan, &Agent::ALL)
}

pub fn build_for(plan: &[SearchRoot], agents: &[Agent]) -> Catalog {
    let mut roots: Vec<RootInfo> = Vec::new();
    for r in plan {
        match roots.iter_mut().find(|i| i.path == r.path) {
            Some(i) => i.agents.extend(r.agent.filter(|a| !i.agents.contains(a))),
            None => roots.push(RootInfo {
                path: r.path.clone(),
                scope: r.scope,
                agents: r.agent.into_iter().collect(),
                exists: r.path.is_dir(),
                link: link_in(&r.path, 2),
            }),
        }
    }

    let mut errors = Vec::new();
    let mut by_canonical: HashMap<PathBuf, usize> = HashMap::new();
    let mut found: Vec<(PathBuf, String, Parsed, Vec<Location>)> = Vec::new();
    for (ri, r) in plan.iter().enumerate() {
        let hits = match scan(r) {
            Ok(h) => h,
            Err(error) => {
                if !errors.iter().any(|e: &ScanError| e.path == r.path) {
                    errors.push(ScanError {
                        path: r.path.clone(),
                        error,
                    });
                }
                continue;
            }
        };
        for (hi, hit) in hits.into_iter().enumerate() {
            let canonical = fs::canonicalize(&hit.path).unwrap_or_else(|_| hit.path.clone());
            let loc = Location {
                agent: r.agent,
                scope: r.scope,
                path: hit.path,
                link: hit.link,
                rank: (ri, hi),
            };
            if let Some(&i) = by_canonical.get(&canonical) {
                found[i].3.push(loc);
                continue;
            }
            match fs::read_to_string(&canonical) {
                Ok(text) => {
                    let parsed = parse(&text);
                    by_canonical.insert(canonical.clone(), found.len());
                    found.push((canonical, text, parsed, vec![loc]));
                }
                Err(error) => {
                    if !errors.iter().any(|e: &ScanError| e.path == loc.path) {
                        errors.push(ScanError {
                            path: loc.path,
                            error,
                        });
                    }
                }
            }
        }
    }

    let mut skills: Vec<Skill> = found
        .into_iter()
        .map(|(canonical, text, parsed, locations)| {
            let primary = locations
                .iter()
                .find(|l| l.link.is_none())
                .unwrap_or(&locations[0]);
            let (path, scope) = (primary.path.clone(), primary.scope);
            let dir = dir_name(&path);
            let name = display_name(&parsed, &dir);
            let skill_dir = canonical.parent().unwrap_or(&canonical);
            let mut lint = lint(&parsed, &dir);
            lint.extend(check_links(&parsed, skill_dir));
            lint.extend(check_smoke(&text, &parsed, skill_dir));
            lint.extend(check_orphans(skill_dir));
            Skill {
                name,
                scope,
                path,
                canonical,
                parsed,
                locations,
                agents: Vec::new(),
                lint,
                claude_alias_with: Vec::new(),
            }
        })
        .collect();

    for &agent in agents {
        // (rank, skill index, name as loaded, path as reached by this agent)
        let mut loaded: Vec<((usize, usize), usize, String, PathBuf)> = Vec::new();
        for (i, s) in skills.iter_mut().enumerate() {
            let Some(best) = s
                .locations
                .iter()
                .filter(|l| l.agent == Some(agent))
                .min_by_key(|l| l.rank)
            else {
                continue;
            };
            match loads(agent, &s.parsed, &dir_name(&best.path)) {
                Load::Loaded(name) => loaded.push((best.rank, i, name, best.path.clone())),
                Load::Skipped(p) => s.agents.push(AgentView {
                    agent,
                    name: s.name.clone(),
                    status: Status::Skipped(p),
                }),
                Load::Unknown(p) => s.agents.push(AgentView {
                    agent,
                    name: s.name.clone(),
                    status: Status::Unknown(p),
                }),
            }
        }
        loaded.sort_by_key(|l| l.0);
        let mut groups: Vec<(String, Vec<(usize, PathBuf)>)> = Vec::new();
        for (_, i, name, path) in loaded {
            match groups.iter_mut().find(|(n, _)| *n == name) {
                Some((_, g)) => g.push((i, path)),
                None => groups.push((name, vec![(i, path)])),
            }
        }
        for (name, group) in groups {
            for (k, (i, _)) in group.iter().enumerate() {
                let status = if group.len() == 1 {
                    Status::Loaded
                } else if agent == Agent::Opencode {
                    let with = group
                        .iter()
                        .filter(|(j, _)| j != i)
                        .map(|(_, p)| p.clone())
                        .collect();
                    Status::Conflict { with }
                } else if k == 0 {
                    Status::Loaded
                } else {
                    Status::Shadowed {
                        by: group[0].1.clone(),
                    }
                };
                skills[*i].agents.push(AgentView {
                    agent,
                    name: name.clone(),
                    status,
                });
            }
        }
    }
    // Claude Code: skills it loads whose frontmatter `name` claims the same `/` command.
    let claimants: Vec<(usize, String, PathBuf)> = skills
        .iter()
        .enumerate()
        .filter(|(_, s)| {
            s.view(Agent::Claude)
                .is_some_and(|v| v.status == Status::Loaded)
        })
        .filter_map(|(i, s)| claude_alias(&s.parsed).map(|a| (i, a.to_owned(), s.path.clone())))
        .collect();
    for (i, alias, _) in &claimants {
        skills[*i].claude_alias_with = claimants
            .iter()
            .filter(|(j, a, _)| a == alias && j != i)
            .map(|(_, _, p)| p.clone())
            .collect();
    }
    for s in &mut skills {
        s.agents.sort_by_key(|v| v.agent);
    }
    skills.sort_by(|a, b| {
        a.name
            .cmp(&b.name)
            .then(a.scope.cmp(&b.scope))
            .then(a.path.cmp(&b.path))
    });
    Catalog {
        agents: agents.to_vec(),
        roots,
        skills,
        errors,
    }
}
