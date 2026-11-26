//! `the use [+SUBJECT | -SUBJECT]` as a routable [`Target`]. A bare `use` lists the subjects and
//! their state; `+SUBJECT` installs and enables a feature, `-SUBJECT` disables it. The result (the
//! resulting state, as a table) goes to stdout; the operational notes (what was installed, PATH and
//! editor next steps) go to stderr, per the channel-discipline rule.

use std::io::{self, Write};

use clap::{Args, Command, FromArgMatches};
use the_output::command::{CommandDescriptor, Ctx, Form, Invocation, Results, Target, Tier};
use the_output::diag::{Diagnostic, Exit};
use the_output::output::Format;
use the_output::style::Term;
use the_output::table::{Cell, Column, Human, Report, Table, Tone};

use crate::cli::{Action, UseArgs};
use crate::{Subject, SubjectRegistry, UseEnv};

/// The `use` command group as a routable [`Target`].
pub struct Use;

const DESCRIPTORS: &[CommandDescriptor] = &[CommandDescriptor {
    path: "use",
    group: "use",
    tier: Tier::Core,
    summary: "install and enable thepia features (use subjects)",
}];

impl Target for Use {
    fn id(&self) -> &'static str {
        "use"
    }

    fn descriptors(&self) -> &'static [CommandDescriptor] {
        DESCRIPTORS
    }

    fn augment(&self, root: Command) -> Command {
        root.subcommand(
            UseArgs::augment_args(Command::new("use")).about(
                "Install and enable Thepia features (use subjects), e.g. `+language-server`.",
            ),
        )
    }

    fn execute(&self, inv: &Invocation, ctx: &Ctx) -> Result<Box<dyn Results>, Diagnostic> {
        let args = UseArgs::from_arg_matches(&inv.matches).map_err(|e| {
            Diagnostic::error("use-args", "could not parse `use` arguments")
                .cause(e.to_string())
                .exit(Exit::Usage)
        })?;
        let action = Action::parse(args.subject.as_deref()).map_err(|bad| {
            Diagnostic::error(
                "use-subject",
                format!("not a usable subject token: `{bad}`"),
            )
            .cause("a subject is a name, optionally prefixed `+` (enable) or `-` (disable)")
            .help("e.g. `the use +language-server`; run `the use` to list subjects")
            .exit(Exit::Usage)
        })?;

        let colored = matches!(ctx.form(), Form::Ansi);
        let h = Human {
            term: Term {
                color: colored,
                tty: ctx.stdout().tty,
            },
            header: !ctx.no_header(),
        };
        let err_term = Term {
            color: ctx.stderr().color,
            tty: ctx.stderr().tty,
        };

        let reg = SubjectRegistry::with_builtins();
        let env = UseEnv::detect();
        let built = run_action(&reg, &env, action, h)?;
        Ok(Box::new(UseResults {
            report: built.report,
            notes: built.notes,
            exit: built.exit,
            h,
            err_term,
        }))
    }
}

/// A built-but-unrendered `use` result.
pub(crate) struct Built {
    report: Report,
    notes: Vec<Diagnostic>,
    exit: u8,
}

/// Run a parsed [`Action`] against the subject registry and environment, producing the report (the
/// resulting state as a table) and the stderr notes. Split out so the action -> result mapping is
/// unit-testable against a temp environment, without the real `~/.thepia` or process detection.
pub(crate) fn run_action(
    reg: &SubjectRegistry,
    env: &UseEnv,
    action: Action,
    _h: Human,
) -> Result<Built, Diagnostic> {
    match action {
        Action::List => Ok(Built {
            report: Report::new().table("subjects", status_table(reg, env, None)),
            notes: Vec::new(),
            exit: 0,
        }),
        Action::Enable(id) => {
            let subject = lookup(reg, &id)?;
            let outcome = subject.activate(env)?;
            Ok(Built {
                report: Report::new().table("subjects", status_table(reg, env, Some(subject.id()))),
                notes: outcome.notes,
                exit: 0,
            })
        }
        Action::Disable(id) => {
            let subject = lookup(reg, &id)?;
            let outcome = subject.deactivate(env)?;
            Ok(Built {
                report: Report::new().table("subjects", status_table(reg, env, Some(subject.id()))),
                notes: outcome.notes,
                exit: 0,
            })
        }
    }
}

/// Resolve a subject id, or a `NotFound` diagnostic that lists the valid ones.
fn lookup<'a>(reg: &'a SubjectRegistry, id: &str) -> Result<&'a dyn Subject, Diagnostic> {
    reg.get(id).ok_or_else(|| {
        Diagnostic::error("unknown-subject", format!("no use subject named `{id}`"))
            .cause("not a registered use subject")
            .help(format!("valid subjects: {}", reg.ids().join(", ")))
            .exit(Exit::NotFound)
    })
}

/// A table of subjects and their state. `only` limits it to one subject (the action view); `None`
/// lists them all (the bare `use` view).
fn status_table(reg: &SubjectRegistry, env: &UseEnv, only: Option<&str>) -> Table {
    let enabled = env.enabled_subjects();
    let mut t = Table::new(vec![
        Column::new("subject", "SUBJECT"),
        Column::new("installed", "INSTALLED"),
        Column::new("enabled", "ENABLED"),
        Column::new("summary", "SUMMARY").max_width(56),
    ]);
    for s in reg.iter() {
        if let Some(id) = only
            && s.id() != id
        {
            continue;
        }
        let installed = s.probe(env).installed;
        let on = enabled.iter().any(|e| e == s.id());
        t.push(vec![
            Cell::from(s.id()),
            yes_no(installed),
            yes_no(on),
            Cell::from(s.summary()),
        ]);
    }
    t
}

fn yes_no(b: bool) -> Cell {
    if b {
        Cell::toned(Tone::Ok, "yes")
    } else {
        Cell::toned(Tone::Dim, "no")
    }
}

/// The `use` command result: the state table (stdout) and the action notes (stderr).
pub struct UseResults {
    report: Report,
    notes: Vec<Diagnostic>,
    exit: u8,
    h: Human,
    err_term: Term,
}

impl Results for UseResults {
    fn render(&self, out: &mut dyn Write, form: Form) -> io::Result<()> {
        out.write_all(self.report.render(form.format(), self.h).as_bytes())
    }

    fn render_diagnostics(&self, err: &mut dyn Write, format: Format) -> io::Result<()> {
        for d in &self.notes {
            err.write_all(d.render(format, self.err_term).as_bytes())?;
        }
        Ok(())
    }

    fn diagnostics(&self) -> &[Diagnostic] {
        &self.notes
    }

    fn exit_code(&self) -> u8 {
        self.exit
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn plain_human() -> Human {
        Human {
            term: Term {
                color: false,
                tty: false,
            },
            header: true,
        }
    }

    fn temp_home(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("the-use-cmd-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn descriptors_match_the_clap_tree() {
        let root = Use.augment(Command::new("the"));
        let use_cmd = root
            .get_subcommands()
            .find(|c| c.get_name() == "use")
            .expect("use subcommand folded onto the root");

        let mut real = vec!["use".to_string()];
        for sub in use_cmd.get_subcommands().filter(|c| !c.is_hide_set()) {
            if sub.get_name() == "help" {
                continue;
            }
            real.push(format!("use {}", sub.get_name()));
        }
        real.sort();

        let mut declared: Vec<String> = DESCRIPTORS.iter().map(|d| d.path.to_string()).collect();
        declared.sort();

        assert_eq!(declared, real, "use descriptors must match the clap tree");
    }

    #[test]
    fn descriptor_summaries_follow_the_style_guide() {
        for d in DESCRIPTORS {
            assert!(d.summary.len() <= 60, "`{}` summary too long", d.path);
            assert!(!d.summary.ends_with('.'), "`{}` trailing period", d.path);
            assert_eq!(
                d.summary,
                d.summary.to_lowercase(),
                "`{}` lowercase",
                d.path
            );
            assert_eq!(d.group, "use");
        }
    }

    #[test]
    fn list_renders_the_registered_subjects() {
        let home = temp_home("list");
        let env = UseEnv::at(&home);
        let reg = SubjectRegistry::with_builtins();
        let built = run_action(&reg, &env, Action::List, plain_human()).unwrap();
        let mut buf = Vec::new();
        UseResults {
            report: built.report,
            notes: built.notes,
            exit: built.exit,
            h: plain_human(),
            err_term: Term {
                color: false,
                tty: false,
            },
        }
        .render(&mut buf, Form::Plain)
        .unwrap();
        let text = String::from_utf8(buf).unwrap();
        assert!(
            text.contains("language-server"),
            "lists the subject: {text}"
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn enable_then_list_reflects_enabled_state() {
        let home = temp_home("enable");
        // Sibling fake thepiad so activate can install without a real build.
        let dist = home.join("dist");
        std::fs::create_dir_all(&dist).unwrap();
        std::fs::write(dist.join("thepiad"), b"#!/bin/sh\n").unwrap();
        let env = UseEnv::at(&home).with_exe(dist.join("thepia"));
        let reg = SubjectRegistry::with_builtins();

        let built = run_action(
            &reg,
            &env,
            Action::Enable("language-server".to_string()),
            plain_human(),
        )
        .unwrap();
        assert_eq!(built.exit, 0);
        assert!(!built.notes.is_empty(), "activate produced notes");
        assert_eq!(env.enabled_subjects(), vec!["language-server".to_string()]);

        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn unknown_subject_is_a_not_found_diagnostic() {
        let home = temp_home("unknown");
        let env = UseEnv::at(&home);
        let reg = SubjectRegistry::with_builtins();
        let err = run_action(
            &reg,
            &env,
            Action::Enable("nope".to_string()),
            plain_human(),
        )
        .err()
        .expect("unknown subject is an error");
        assert_eq!(err.code, "unknown-subject");
        assert_eq!(err.exit, Exit::NotFound);
        assert!(err.help.unwrap().contains("language-server"));
        let _ = std::fs::remove_dir_all(&home);
    }
}
