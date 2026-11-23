//! The command pipeline contract for the Thepia command line.
//!
//! A command is parsed into a routable [`Invocation`], routed through a live [`Registry`] to the
//! [`Target`] addressed by its name, executed to a [`Results`] value, and only then rendered to the
//! caller's streams. The same [`Target`] interface is implemented by an in-crate module, a sibling
//! crate, or (later) an out-of-process plugin, so adding a command group is a `register`, not a new
//! dispatch arm.
//!
//! Output is a deliberate second step: [`Target::execute`] returns a [`Results`], and the pipeline
//! renders it with [`Results::render`] (stdout) and [`Results::render_diagnostics`] (stderr). A
//! module owns its own view, using the `the-output` crate as a toolkit rather than relying on a
//! universal renderer. Streams are injected by the caller ([`dispatch`]), so the whole pipeline is
//! unit-testable against buffers.

use std::ffi::OsString;
use std::io::{self, IsTerminal, Write};

use crate::diag::{Diagnostic, Exit};
use crate::output::Format;
use clap::{ArgMatches, Command};

/// The concrete render target a result is written as. Human output is split into `Plain` (no ANSI)
/// and `Ansi` (colored) so a module's render is an explicit choice, not a hidden branch; `Json` is
/// the machine form. Resolved from `--format` and the stdout color state by [`Ctx::form`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Form {
    Plain,
    Ansi,
    Json,
}

impl Form {
    /// The `--format` family this render form belongs to (`Plain`/`Ansi` are both human).
    pub fn format(self) -> Format {
        match self {
            Form::Plain | Form::Ansi => Format::Human,
            Form::Json => Format::Json,
        }
    }
}

/// The resolved color and terminal state of one output stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamState {
    /// Emit ANSI color.
    pub color: bool,
    /// The stream is a terminal: decorate (table rule, truncation).
    pub tty: bool,
}

impl StreamState {
    /// Detect color (honoring `NO_COLOR`/`CLICOLOR*`/`FORCE_COLOR`/`COLOR`) and tty for stdout.
    pub fn stdout() -> Self {
        Self {
            color: crate::style::stdout_color(),
            tty: io::stdout().is_terminal(),
        }
    }

    /// Detect color and tty for stderr.
    pub fn stderr() -> Self {
        Self {
            color: crate::style::stderr_color(),
            tty: io::stderr().is_terminal(),
        }
    }
}

/// Per-invocation context the pipeline threads to every target: the `--format` the user asked for,
/// `--no-header`, and the resolved color/tty of stdout and stderr. It carries no group-specific
/// state (a target builds its own scan/scope context from its typed args), so the core never learns
/// a command's specifics.
#[derive(Debug, Clone, Copy)]
pub struct Ctx {
    format: Format,
    no_header: bool,
    out: StreamState,
    err: StreamState,
}

impl Ctx {
    pub fn new(format: Format, no_header: bool, out: StreamState, err: StreamState) -> Self {
        Self {
            format,
            no_header,
            out,
            err,
        }
    }

    pub fn format(self) -> Format {
        self.format
    }

    pub fn no_header(self) -> bool {
        self.no_header
    }

    pub fn stdout(self) -> StreamState {
        self.out
    }

    pub fn stderr(self) -> StreamState {
        self.err
    }

    /// Resolve the concrete stdout render [`Form`]: JSON stays JSON; human output is `Ansi` when
    /// stdout is colored, else `Plain`.
    pub fn form(self) -> Form {
        match self.format {
            Format::Json => Form::Json,
            Format::Human => {
                if self.out.color {
                    Form::Ansi
                } else {
                    Form::Plain
                }
            }
        }
    }
}

/// Whether a positional token is a filesystem PATH (a scan root) rather than a subcommand or a bare
/// name: it contains `/` or `\`, starts with `~`, or is `.`/`..`. A command uses this to pattern-match
/// a relative/absolute path given in place of (or after) a subcommand, so `<group> ..` scans a
/// directory instead of being rejected as an unknown subcommand. Shared so every command applies the
/// same rule. Pair it with clap's `#[command(external_subcommand)]` to capture the leading token.
pub fn is_path_like(s: &str) -> bool {
    s.contains('/') || s.contains('\\') || s.starts_with('~') || s == "." || s == ".."
}

/// A parsed, routable command: the target it is addressed to and the parsed arguments for it. This
/// is the in-process form; a target that forwards execution to a daemon lowers it to the transport
/// envelope (`thepia-proto`, newline-JSON locally / irpc over iroh) at that boundary. Kept minimal
/// so it reads as a command / intent / actor message.
pub struct Invocation {
    /// The target's routing id (its top-level subcommand name).
    pub target: &'static str,
    /// The parsed arguments for the routed subcommand; a target recovers its typed args from these.
    pub matches: ArgMatches,
}

/// What a command produced, rendered on demand. Execution and output are two steps: a target returns
/// this, and the pipeline renders it, so a routed or long-running command produces its result once
/// and the CLI renders it. The render methods own the view, using `the-output` as a toolkit.
pub trait Results {
    /// Write the result (stdout) in `form`. Only the result goes here; diagnostics are rendered by
    /// [`render_diagnostics`](Results::render_diagnostics).
    fn render(&self, out: &mut dyn Write, form: Form) -> io::Result<()>;

    /// Write diagnostics (stderr) in `format`. The default streams each diagnostic plainly; a target
    /// with a richer presentation (e.g. a capped summary) overrides this.
    fn render_diagnostics(&self, err: &mut dyn Write, format: Format) -> io::Result<()> {
        for d in self.diagnostics() {
            err.write_all(d.render(format, crate::style::Term::stderr()).as_bytes())?;
        }
        Ok(())
    }

    /// The structured diagnostics (for the default stderr rendering and for inspection in tests).
    fn diagnostics(&self) -> &[Diagnostic] {
        &[]
    }

    /// Process exit code (0 = success).
    fn exit_code(&self) -> u8 {
        0
    }

    /// Convenience render targets (the `plain()/ansi()/json()` surface), each delegating to
    /// [`render`](Results::render).
    fn plain(&self, out: &mut dyn Write) -> io::Result<()> {
        self.render(out, Form::Plain)
    }
    fn ansi(&self, out: &mut dyn Write) -> io::Result<()> {
        self.render(out, Form::Ansi)
    }
    fn json(&self, out: &mut dyn Write) -> io::Result<()> {
        self.render(out, Form::Json)
    }
}

/// The command-economy tier of a command (help grouping / surface governance).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// A daily-driver command, surfaced prominently.
    Core,
    /// A de-emphasized / auxiliary command.
    Aux,
}

/// Help and governance metadata for one command, a plain data literal. The registry's descriptors
/// drive the bare-surface listing and grouped help, and a drift test asserts the set matches the
/// clap tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandDescriptor {
    /// Space-joined command path, e.g. `"skills"` or `"skills lint"`.
    pub path: &'static str,
    /// The owning target id, e.g. `"skills"`.
    pub group: &'static str,
    pub tier: Tier,
    /// An imperative one-liner (style guide: lowercase, no trailing period, <= 60 chars).
    pub summary: &'static str,
}

/// One command group. Implemented identically by an in-crate module, a sibling crate, or a future
/// out-of-process plugin, so the [`Registry`] routes to any of them the same way.
pub trait Target {
    /// Stable routing id and top-level subcommand name (e.g. `"skills"`).
    fn id(&self) -> &'static str;

    /// This target's commands, for help / governance and the drift check.
    fn descriptors(&self) -> &'static [CommandDescriptor];

    /// Contribute this target's grammar to the root parser. Called once when the parser is built.
    fn augment(&self, root: Command) -> Command;

    /// Run a routed invocation, producing a result. Output is a separate step (see [`Results`]).
    fn execute(&self, inv: &Invocation, ctx: &Ctx) -> Result<Box<dyn Results>, Diagnostic>;
}

/// A live set of registered targets. Routing (`get`), parser construction (`augment_root`), and the
/// governance surface (`descriptors`) all read this one structure; adding a command group is a
/// `register`, not a new dispatch arm. Modeled on cli's compile-time `PluginRegistry`.
#[derive(Default)]
pub struct Registry {
    targets: Vec<Box<dyn Target>>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a target. Chainable: `Registry::new().register(a).register(b)`.
    #[must_use]
    pub fn register(mut self, target: Box<dyn Target>) -> Self {
        self.targets.push(target);
        self
    }

    /// The target addressed by `id`, if registered.
    pub fn get(&self, id: &str) -> Option<&dyn Target> {
        self.targets
            .iter()
            .find(|t| t.id() == id)
            .map(|t| t.as_ref())
    }

    /// Fold every registered target's grammar onto `root`.
    pub fn augment_root(&self, root: Command) -> Command {
        self.targets.iter().fold(root, |c, t| t.augment(c))
    }

    /// Every registered command descriptor, across all targets.
    pub fn descriptors(&self) -> Vec<&'static CommandDescriptor> {
        self.targets
            .iter()
            .flat_map(|t| t.descriptors().iter())
            .collect()
    }

    /// The registered target ids, in registration order.
    pub fn ids(&self) -> Vec<&'static str> {
        self.targets.iter().map(|t| t.id()).collect()
    }
}

/// Parse `argv` against the registry's grammar, route to the addressed target, render its result to
/// `out` and its diagnostics to `err`, and return the process exit code. Streams and argv are
/// injected so the whole pipeline is unit-testable against buffers.
///
/// `root` is the base clap command (name, version, and the global `--format` / `--no-header` args);
/// the registry folds each target's subcommand onto it. `out_state` / `err_state` are the resolved
/// color + tty of the real streams (injected so a test can force them).
pub fn dispatch<I, S>(
    reg: &Registry,
    root: Command,
    argv: I,
    out_state: StreamState,
    err_state: StreamState,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> u8
where
    I: IntoIterator<Item = S>,
    S: Into<OsString> + Clone,
{
    let root = reg.augment_root(root);
    let matches = match root.try_get_matches_from(argv) {
        Ok(m) => m,
        // clap renders `--help` / `--version` / usage errors itself; route its text to the right
        // stream (help/version to stdout, errors to stderr) and map its exit code.
        Err(e) => {
            let text = e.render().to_string();
            if e.use_stderr() {
                let _ = err.write_all(text.as_bytes());
            } else {
                let _ = out.write_all(text.as_bytes());
            }
            return e.exit_code().clamp(0, 255) as u8;
        }
    };

    // Read the globals defensively: a root without them (e.g. a minimal parser) is valid and
    // falls back to the defaults rather than panicking on an unknown arg id.
    let format = matches
        .try_get_one::<Format>("format")
        .ok()
        .flatten()
        .copied()
        .unwrap_or(Format::Human);
    let no_header = matches
        .try_get_one::<bool>("no-header")
        .ok()
        .flatten()
        .copied()
        .unwrap_or(false);
    let ctx = Ctx::new(format, no_header, out_state, err_state);

    let Some((id, sub)) = matches.subcommand() else {
        // With `subcommand_required` + `arg_required_else_help`, clap errors above before reaching
        // here; this is only a defensive fallback.
        return 0;
    };

    let Some(target) = reg.get(id) else {
        // A parsed subcommand with no registered target is a registration bug the drift test guards.
        let _ = writeln!(err, "internal error: no target registered for `{id}`");
        return Exit::Generic as u8;
    };

    let inv = Invocation {
        target: target.id(),
        matches: sub.clone(),
    };
    match target.execute(&inv, &ctx) {
        Ok(results) => {
            let _ = results.render(out, ctx.form());
            let _ = results.render_diagnostics(err, ctx.format());
            results.exit_code()
        }
        Err(d) => {
            let _ = err.write_all(
                d.render(ctx.format(), crate::style::Term::stderr())
                    .as_bytes(),
            );
            d.exit as u8
        }
    }
}

/// Test-support for command crates: drive a command through the full render-form matrix and assert
/// the cross-form invariants, so one short argv list yields wide-combination coverage. This is the
/// standard way a command crate proves it is flexible across `--format` and color. Not used in
/// production (pure test helpers; covered by the command crates' tests).
pub mod conformance {
    use super::*;
    use clap::{Arg, ArgAction, value_parser};

    /// One captured run of the pipeline against injected plain buffers.
    #[derive(Debug)]
    pub struct Captured {
        pub out: String,
        pub err: String,
        pub code: u8,
    }

    /// A root carrying the global `--format` / `--no-header`, as the binary builds it, so a command
    /// crate can run the matrix without depending on the binary.
    pub fn root() -> Command {
        Command::new("the")
            .subcommand_required(true)
            .arg_required_else_help(true)
            .arg(
                Arg::new("format")
                    .long("format")
                    .global(true)
                    .value_parser(value_parser!(Format))
                    .default_value("human"),
            )
            .arg(
                Arg::new("no-header")
                    .long("no-header")
                    .global(true)
                    .action(ArgAction::SetTrue),
            )
    }

    /// Run one `argv` through the pipeline with the given stdout color; stderr is plain.
    pub fn run(reg: &Registry, argv: &[&str], color: bool) -> Captured {
        let out_state = StreamState { color, tty: false };
        let err_state = StreamState {
            color: false,
            tty: false,
        };
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = dispatch(
            reg,
            root(),
            argv.iter().copied(),
            out_state,
            err_state,
            &mut out,
            &mut err,
        );
        Captured {
            out: String::from_utf8_lossy(&out).into_owned(),
            err: String::from_utf8_lossy(&err).into_owned(),
            code,
        }
    }

    /// Strip CSI/SGR escape sequences so colored output can be compared to plain.
    fn strip_ansi(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        let mut chars = s.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\u{1b}' {
                for n in chars.by_ref() {
                    if n.is_ascii_alphabetic() {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    /// Run `argv` as plain, ansi, and json and assert the cross-form invariants every command must
    /// hold, returning the plain capture for any command-specific extra checks:
    /// - plain stdout carries no ANSI escapes (color is opt-in, never leaked);
    /// - ansi stdout stripped of color equals plain stdout (color is the only difference);
    /// - json stdout, when non-empty, parses as JSON (the machine form is always valid);
    /// - the exit code is identical across all three forms (the form never changes the outcome).
    pub fn assert_forms(reg: &Registry, argv: &[&str]) -> Captured {
        assert!(!argv.is_empty(), "argv needs at least the program name");
        let plain = run(reg, argv, false);
        let ansi = run(reg, argv, true);
        let mut jargv = vec![argv[0], "--format", "json"];
        jargv.extend_from_slice(&argv[1..]);
        let json = run(reg, &jargv, false);

        assert!(
            !plain.out.contains('\u{1b}'),
            "plain stdout must carry no ANSI escapes for {argv:?}:\n{}",
            plain.out
        );
        assert_eq!(
            strip_ansi(&ansi.out),
            plain.out,
            "ansi and plain stdout must differ only by color for {argv:?}"
        );
        if !json.out.trim().is_empty() {
            serde_json::from_str::<serde_json::Value>(&json.out).unwrap_or_else(|e| {
                panic!("json stdout must parse for {argv:?}: {e}\n{}", json.out)
            });
        }
        assert_eq!(
            plain.code, ansi.code,
            "exit code must not change with color for {argv:?}"
        );
        assert_eq!(
            plain.code, json.code,
            "exit code must not change with --format for {argv:?}"
        );
        plain
    }

    /// Assert the invariants across a whole matrix of argv lines (subcommands x flags x ...).
    pub fn assert_matrix(reg: &Registry, argvs: &[&[&str]]) {
        for argv in argvs {
            assert_forms(reg, argv);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn state(color: bool, tty: bool) -> StreamState {
        StreamState { color, tty }
    }

    #[test]
    fn form_resolves_from_format_and_stdout_color() {
        let plain = Ctx::new(
            Format::Human,
            false,
            state(false, false),
            state(false, false),
        );
        assert_eq!(plain.form(), Form::Plain);
        let ansi = Ctx::new(Format::Human, false, state(true, true), state(true, true));
        assert_eq!(ansi.form(), Form::Ansi);
        // JSON ignores color entirely.
        let json_colored = Ctx::new(Format::Json, false, state(true, true), state(true, true));
        assert_eq!(json_colored.form(), Form::Json);
        let json_plain = Ctx::new(
            Format::Json,
            false,
            state(false, false),
            state(false, false),
        );
        assert_eq!(json_plain.form(), Form::Json);
    }

    #[test]
    fn form_maps_back_to_format_family() {
        assert_eq!(Form::Plain.format(), Format::Human);
        assert_eq!(Form::Ansi.format(), Format::Human);
        assert_eq!(Form::Json.format(), Format::Json);
    }

    // A minimal fake target proves a command drops in by registration alone: no edit to `dispatch`.
    struct FakeResults {
        word: String,
    }
    impl Results for FakeResults {
        fn render(&self, out: &mut dyn Write, form: Form) -> io::Result<()> {
            match form {
                Form::Json => writeln!(out, "{{\"word\":\"{}\"}}", self.word),
                Form::Plain | Form::Ansi => writeln!(out, "{}", self.word),
            }
        }
        fn exit_code(&self) -> u8 {
            0
        }
    }

    struct FakeTarget;
    const FAKE_DESCRIPTORS: &[CommandDescriptor] = &[CommandDescriptor {
        path: "echo",
        group: "echo",
        tier: Tier::Aux,
        summary: "echo a word back",
    }];
    impl Target for FakeTarget {
        fn id(&self) -> &'static str {
            "echo"
        }
        fn descriptors(&self) -> &'static [CommandDescriptor] {
            FAKE_DESCRIPTORS
        }
        fn augment(&self, root: Command) -> Command {
            root.subcommand(Command::new("echo").arg(clap::Arg::new("word").required(true)))
        }
        fn execute(&self, inv: &Invocation, _ctx: &Ctx) -> Result<Box<dyn Results>, Diagnostic> {
            let word = inv
                .matches
                .get_one::<String>("word")
                .cloned()
                .unwrap_or_default();
            Ok(Box::new(FakeResults { word }))
        }
    }

    fn test_root() -> Command {
        Command::new("the")
            .subcommand_required(true)
            .arg_required_else_help(true)
    }

    /// A root carrying the `--format` global, as the real binary builds it, so the JSON path and the
    /// global-arg read in `dispatch` are exercised.
    fn test_root_with_globals() -> Command {
        test_root().arg(
            clap::Arg::new("format")
                .long("format")
                .global(true)
                .value_parser(clap::value_parser!(Format))
                .default_value("human"),
        )
    }

    fn reg() -> Registry {
        Registry::new().register(Box::new(FakeTarget))
    }

    #[test]
    fn registry_routes_to_the_registered_target() {
        assert!(reg().get("echo").is_some());
        assert!(reg().get("nope").is_none());
        assert_eq!(reg().ids(), vec!["echo"]);
        assert_eq!(reg().descriptors().len(), 1);
    }

    #[test]
    fn dispatch_routes_executes_and_renders_plain() {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = dispatch(
            &reg(),
            test_root(),
            ["the", "echo", "hello"],
            state(false, false),
            state(false, false),
            &mut out,
            &mut err,
        );
        assert_eq!(code, 0);
        assert_eq!(String::from_utf8(out).unwrap(), "hello\n");
        assert!(err.is_empty());
    }

    #[test]
    fn dispatch_renders_json_when_format_json() {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = dispatch(
            &reg(),
            test_root_with_globals(),
            ["the", "--format", "json", "echo", "hi"],
            // stdout is a color tty, but `--format json` must still render JSON (color ignored).
            state(true, true),
            state(false, false),
            &mut out,
            &mut err,
        );
        assert_eq!(code, 0);
        assert_eq!(String::from_utf8(out).unwrap(), "{\"word\":\"hi\"}\n");
    }

    #[test]
    fn dispatch_renders_ansi_when_stdout_is_a_color_tty() {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = dispatch(
            &reg(),
            test_root_with_globals(),
            ["the", "echo", "hi"],
            state(true, true),
            state(false, false),
            &mut out,
            &mut err,
        );
        assert_eq!(code, 0);
        // The fake renders Plain/Ansi identically; this asserts the human (non-JSON) path is taken.
        assert_eq!(String::from_utf8(out).unwrap(), "hi\n");
    }

    #[test]
    fn dispatch_reports_help_to_stdout_with_zero_exit() {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = dispatch(
            &reg(),
            test_root(),
            ["the", "--help"],
            state(false, false),
            state(false, false),
            &mut out,
            &mut err,
        );
        assert_eq!(code, 0);
        assert!(String::from_utf8(out).unwrap().contains("echo"));
    }

    // A results type relying on the default trait methods (diagnostics/exit/plain/ansi/json).
    struct DefaultResults;
    impl Results for DefaultResults {
        fn render(&self, out: &mut dyn Write, form: Form) -> io::Result<()> {
            writeln!(out, "form={form:?}")
        }
    }

    fn rendered(
        r: &DefaultResults,
        f: impl FnOnce(&DefaultResults, &mut Vec<u8>) -> io::Result<()>,
    ) -> String {
        let mut b = Vec::new();
        f(r, &mut b).unwrap();
        String::from_utf8(b).unwrap()
    }

    #[test]
    fn default_results_methods_delegate_and_default() {
        let r = DefaultResults;
        assert_eq!(r.exit_code(), 0);
        assert!(r.diagnostics().is_empty());
        assert_eq!(rendered(&r, |r, b| r.plain(b)), "form=Plain\n");
        assert_eq!(rendered(&r, |r, b| r.ansi(b)), "form=Ansi\n");
        assert_eq!(rendered(&r, |r, b| r.json(b)), "form=Json\n");
        // Default render_diagnostics with no diagnostics writes nothing.
        let mut e = Vec::new();
        r.render_diagnostics(&mut e, Format::Human).unwrap();
        assert!(e.is_empty());
    }

    struct DiagResults(Vec<Diagnostic>);
    impl Results for DiagResults {
        fn render(&self, _out: &mut dyn Write, _form: Form) -> io::Result<()> {
            Ok(())
        }
        fn diagnostics(&self) -> &[Diagnostic] {
            &self.0
        }
    }

    #[test]
    fn default_render_diagnostics_streams_each() {
        let r = DiagResults(vec![Diagnostic::warning("w", "boom")]);
        let mut e = Vec::new();
        r.render_diagnostics(&mut e, Format::Human).unwrap();
        assert!(String::from_utf8(e).unwrap().contains("boom"));
    }

    #[test]
    fn dispatch_returns_zero_when_no_subcommand_given() {
        // A root that does not require a subcommand, with none supplied, falls through to the
        // defensive `return 0`.
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = dispatch(
            &reg(),
            Command::new("the"),
            ["the"],
            state(false, false),
            state(false, false),
            &mut out,
            &mut err,
        );
        assert_eq!(code, 0);
        assert!(out.is_empty());
    }

    #[test]
    fn dispatch_reports_internal_error_for_an_unregistered_subcommand() {
        // A ghost subcommand in the grammar with no matching target exercises the drift-guard branch.
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = dispatch(
            &Registry::new(),
            Command::new("the").subcommand(Command::new("ghost")),
            ["the", "ghost"],
            state(false, false),
            state(false, false),
            &mut out,
            &mut err,
        );
        assert_eq!(code, Exit::Generic as u8);
        assert!(
            String::from_utf8(err)
                .unwrap()
                .contains("no target registered")
        );
    }

    #[test]
    fn dispatch_reports_unknown_subcommand_to_stderr_nonzero() {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = dispatch(
            &reg(),
            test_root(),
            ["the", "bogus"],
            state(false, false),
            state(false, false),
            &mut out,
            &mut err,
        );
        assert_ne!(code, 0);
        assert!(out.is_empty());
        assert!(!err.is_empty());
    }
}
