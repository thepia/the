//! `thepia use` — install and enable Thepia features ("use subjects").
//!
//! The command is a thin front door over a central **registry of use subjects**: each subject is one
//! installable/enable-able feature (a descriptor row + an impl of [`Subject`]), and the registry is
//! the single source of truth that routing, listing, and the drift check all read, rather than a set
//! of per-feature match arms (see the rust-metadata-tables norm; this mirrors the daemon's
//! `EdgeRegistry` and the command catalog).
//!
//! - `thepia use` — list the subjects and their state.
//! - `thepia use +<subject>` — install the feature and configure it to run from then on (enable).
//! - `thepia use -<subject>` — disable it (stop configuring it to run; the install is left in place).
//!
//! Enabled state is recorded in `~/.thepia/config.toml` under `[use] enabled = [...]`, edited in
//! place so the user's own config and comments survive. A subject's *installed* state is probed from
//! the system (e.g. the presence of the installed binary), so the two are independent: a feature can
//! be installed-but-disabled or enabled-but-not-yet-installed, and the listing shows both.
//!
//! The first subject is [`language_server::LanguageServer`] (the thepiad agent-config LSP, WORK-0046
//! #6). It installs via the phased "front door" model: it places the `thepiad` binary on a stable
//! path and records the enable, so the editor runs it from then on. The resident-daemon /
//! OS-service hosting (launchd/systemd, socket transport) is the designed end-state it grows into.

use std::path::PathBuf;

use the_output::diag::Diagnostic;

pub mod cli;
pub mod cmd;
pub mod language_server;

pub use cmd::Use;

/// What a subject probe found on this machine (the subject-specific half of status). The generic
/// `enabled` half is read from `~/.thepia/config.toml` by the command, not here.
#[derive(Debug, Clone, Default)]
pub struct SubjectProbe {
    /// The feature's artifacts are present (e.g. the installed binary exists).
    pub installed: bool,
    /// A one-line human detail (e.g. the installed path, or why it is not installed).
    pub detail: Option<String>,
}

/// The notes produced by activating/deactivating a subject: note/warning diagnostics rendered on
/// stderr (what happened, PATH guidance, the next manual step). The result itself (the new state)
/// goes to stdout; these are the operational narration, per the channel-discipline rule.
#[derive(Debug, Default)]
pub struct SubjectOutcome {
    pub notes: Vec<Diagnostic>,
}

impl SubjectOutcome {
    pub fn new() -> Self {
        Self::default()
    }
    #[must_use]
    pub fn note(mut self, d: Diagnostic) -> Self {
        self.notes.push(d);
        self
    }
}

/// One installable/enable-able feature. The registry holds these as trait objects and routes by
/// [`id`](Subject::id); adding a feature is one `register`, not a dispatch arm.
pub trait Subject {
    /// Stable routing id, the token after `use` (e.g. `"language-server"`).
    fn id(&self) -> &'static str;

    /// A one-line summary for the listing (lowercase, imperative-ish, no trailing period).
    fn summary(&self) -> &'static str;

    /// Probe the system for the feature's installed state (artifacts present) and a human detail.
    fn probe(&self, env: &UseEnv) -> SubjectProbe;

    /// Install the feature and configure it to run from then on. Idempotent: re-running on an
    /// already-active subject re-verifies and reports, it does not error.
    fn activate(&self, env: &UseEnv) -> Result<SubjectOutcome, Diagnostic>;

    /// Stop configuring the feature to run (disable). The install is left in place.
    fn deactivate(&self, env: &UseEnv) -> Result<SubjectOutcome, Diagnostic>;
}

/// The built-in registry of use subjects. One source of truth for routing, listing, and the drift
/// test; registration order is listing order.
pub struct SubjectRegistry {
    subjects: Vec<Box<dyn Subject>>,
}

impl SubjectRegistry {
    /// The registry with every built-in subject registered.
    pub fn with_builtins() -> Self {
        Self {
            subjects: vec![Box::new(language_server::LanguageServer)],
        }
    }

    /// The subject addressed by `id`, if registered.
    pub fn get(&self, id: &str) -> Option<&dyn Subject> {
        self.subjects
            .iter()
            .find(|s| s.id() == id)
            .map(|s| s.as_ref())
    }

    /// Every registered subject, in registration order.
    pub fn iter(&self) -> impl Iterator<Item = &dyn Subject> {
        self.subjects.iter().map(|s| s.as_ref())
    }

    /// The registered subject ids, in registration order.
    pub fn ids(&self) -> Vec<&'static str> {
        self.subjects.iter().map(|s| s.id()).collect()
    }
}

impl Default for SubjectRegistry {
    fn default() -> Self {
        Self::with_builtins()
    }
}

/// The filesystem environment `thepia use` operates in: the home directory it derives `~/.thepia`
/// from, and the path of the running executable (used to find a sibling binary to install). Carried
/// as a value so tests can point it at a temp home with no real `$HOME` or install side effects.
#[derive(Debug, Clone)]
pub struct UseEnv {
    home: PathBuf,
    exe: Option<PathBuf>,
}

impl UseEnv {
    /// Detect the real environment: `$HOME` (falling back to the current dir) and the running exe.
    pub fn detect() -> Self {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        Self {
            home,
            exe: std::env::current_exe().ok(),
        }
    }

    /// A test/override environment rooted at `home`, with no known executable.
    pub fn at(home: impl Into<PathBuf>) -> Self {
        Self {
            home: home.into(),
            exe: None,
        }
    }

    /// Override the executable path (for tests that exercise sibling-binary discovery).
    #[must_use]
    pub fn with_exe(mut self, exe: impl Into<PathBuf>) -> Self {
        self.exe = Some(exe.into());
        self
    }

    pub fn home(&self) -> &std::path::Path {
        &self.home
    }
    pub fn exe(&self) -> Option<&std::path::Path> {
        self.exe.as_deref()
    }

    /// `~/.thepia`, the Thepia per-user base directory.
    pub fn thepia_dir(&self) -> PathBuf {
        self.home.join(".thepia")
    }
    /// `~/.thepia/bin`, where `thepia use` installs feature binaries (PATH-reachable).
    pub fn bin_dir(&self) -> PathBuf {
        self.thepia_dir().join("bin")
    }
    /// `~/.thepia/config.toml`, the user-editable config recording enabled subjects and more.
    pub fn config_path(&self) -> PathBuf {
        self.thepia_dir().join("config.toml")
    }

    /// The subject ids currently enabled per `~/.thepia/config.toml` (`[use] enabled = [...]`).
    /// A missing or malformed file reads as "none enabled" rather than an error, so the listing and
    /// status always work; a write is what surfaces a real config problem.
    pub fn enabled_subjects(&self) -> Vec<String> {
        let Ok(text) = std::fs::read_to_string(self.config_path()) else {
            return Vec::new();
        };
        let Ok(doc) = text.parse::<toml_edit::DocumentMut>() else {
            return Vec::new();
        };
        doc.get("use")
            .and_then(|u| u.get("enabled"))
            .and_then(|e| e.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Record `id` as enabled (`on = true`) or not (`on = false`) in `~/.thepia/config.toml`,
    /// editing the file in place so any other config and the user's comments are preserved. Creates
    /// `~/.thepia` and the file if absent. The `[use] enabled` array is kept sorted and deduplicated.
    pub fn set_enabled(&self, id: &str, on: bool) -> std::io::Result<()> {
        use toml_edit::{Array, DocumentMut, Item, Value, value};

        std::fs::create_dir_all(self.thepia_dir())?;
        let path = self.config_path();
        let mut doc = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| t.parse::<DocumentMut>().ok())
            .unwrap_or_default();

        // Gather the current set, apply the change, re-sort/dedup.
        let mut set: Vec<String> = doc
            .get("use")
            .and_then(|u| u.get("enabled"))
            .and_then(|e| e.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        set.retain(|s| s != id);
        if on {
            set.push(id.to_string());
        }
        set.sort();
        set.dedup();

        let mut arr = Array::new();
        for s in set {
            arr.push(Value::from(s));
        }
        // Ensure a `[use]` table exists, then set its `enabled` array.
        if !doc.contains_key("use") {
            doc["use"] = Item::Table(toml_edit::Table::new());
        }
        doc["use"]["enabled"] = value(arr);

        std::fs::write(&path, doc.to_string())
    }
}

/// Locate an executable named `name` on `$PATH`, returning the first match. A small built-in `which`
/// so a subject can find a sibling/installed binary without a dependency.
pub(crate) fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|p| p.is_file())
}

#[cfg(test)]
mod config_tests {
    use super::*;

    fn temp_home(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("the-use-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn enabled_round_trips_and_preserves_other_config() {
        let home = temp_home("cfg");
        let env = UseEnv::at(&home);
        assert!(env.enabled_subjects().is_empty());

        // Seed the config with unrelated content + a comment to prove it survives.
        std::fs::create_dir_all(env.thepia_dir()).unwrap();
        std::fs::write(
            env.config_path(),
            "# my thepia config\n[mesh]\nname = \"laptop\"\n",
        )
        .unwrap();

        env.set_enabled("language-server", true).unwrap();
        assert_eq!(env.enabled_subjects(), vec!["language-server".to_string()]);

        let text = std::fs::read_to_string(env.config_path()).unwrap();
        assert!(text.contains("# my thepia config"), "comment preserved");
        assert!(text.contains("name = \"laptop\""), "other table preserved");

        // Enabling again is idempotent (no duplicate).
        env.set_enabled("language-server", true).unwrap();
        assert_eq!(env.enabled_subjects(), vec!["language-server".to_string()]);

        // Disabling removes it, leaving an empty array.
        env.set_enabled("language-server", false).unwrap();
        assert!(env.enabled_subjects().is_empty());

        let _ = std::fs::remove_dir_all(&home);
    }
}
