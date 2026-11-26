//! The `language-server` use subject: the thepiad agent-config LSP (WORK-0046 #6, ST-13).
//!
//! `thepia use +language-server` installs the `thepiad` binary onto a stable, PATH-reachable path
//! (`~/.thepia/bin/thepiad`) and records the enable; from then on the VS Code extension
//! (`editors/vscode`) spawns `thepiad --lsp` for every session, so SKILL.md and agent-definition
//! files get live diagnostics. This is the phased "front door": the binary install + enable are the
//! deterministic core done here; the editor extension is installed separately (the activate notes
//! give the exact step). The designed end-state is a resident daemon run as an OS service with a
//! socket transport — a later increment behind the same `thepia use +language-server` command.

use std::path::{Path, PathBuf};

use the_output::diag::{Diagnostic, Exit};

use crate::{Subject, SubjectOutcome, SubjectProbe, UseEnv, which};

/// The binary that hosts the language server (`thepiad --lsp`).
const BINARY: &str = "thepiad";

pub struct LanguageServer;

impl LanguageServer {
    /// The install destination: `~/.thepia/bin/thepiad`.
    fn dest(env: &UseEnv) -> PathBuf {
        env.bin_dir().join(BINARY)
    }

    /// Find a `thepiad` to install, preferring (1) one already installed at the destination, (2) a
    /// sibling of the running `thepia` executable (the two are distributed together), (3) `thepiad`
    /// on `$PATH`. Returns the source path, or `None` if none is found.
    fn locate_source(env: &UseEnv) -> Option<PathBuf> {
        let dest = Self::dest(env);
        if dest.is_file() {
            return Some(dest);
        }
        if let Some(exe) = env.exe()
            && let Some(sib) = exe.parent().map(|d| d.join(BINARY))
            && sib.is_file()
        {
            return Some(sib);
        }
        which(BINARY)
    }
}

impl Subject for LanguageServer {
    fn id(&self) -> &'static str {
        "language-server"
    }

    fn summary(&self) -> &'static str {
        "VS Code language server for SKILL.md and agent configs"
    }

    fn probe(&self, env: &UseEnv) -> SubjectProbe {
        let dest = Self::dest(env);
        if dest.is_file() {
            SubjectProbe {
                installed: true,
                detail: Some(format!("installed at {}", dest.display())),
            }
        } else {
            SubjectProbe {
                installed: false,
                detail: Self::locate_source(env)
                    .map(|s| format!("available to install from {}", s.display())),
            }
        }
    }

    fn activate(&self, env: &UseEnv) -> Result<SubjectOutcome, Diagnostic> {
        let dest = Self::dest(env);

        // 1. Install the binary to ~/.thepia/bin, unless it is already the one there.
        let installed_note = if dest.is_file() {
            Diagnostic::note(
                "ls-installed",
                format!("thepiad already at {}", dest.display()),
            )
        } else {
            let source = Self::locate_source(env).ok_or_else(|| {
                Diagnostic::error(
                    "thepiad-not-found",
                    "cannot find a `thepiad` binary to install",
                )
                .cause("not beside the `thepia` executable, and not on PATH")
                .help(
                    "build it with `just build` (or `cargo build --release -p the-daemon`) and \
                     ensure `thepiad` is on PATH, then re-run `thepia use +language-server`",
                )
                .exit(Exit::NotFound)
            })?;
            std::fs::create_dir_all(env.bin_dir()).map_err(|e| {
                Diagnostic::error("ls-mkdir", "could not create ~/.thepia/bin")
                    .cause(e.to_string())
                    .exit(Exit::Generic)
            })?;
            std::fs::copy(&source, &dest).map_err(|e| {
                Diagnostic::error("ls-install", "could not install thepiad")
                    .cause(format!(
                        "copy {} -> {}: {e}",
                        source.display(),
                        dest.display()
                    ))
                    .exit(Exit::Generic)
            })?;
            Diagnostic::note(
                "ls-installed",
                format!("installed thepiad to {}", dest.display()),
            )
        };

        // 2. Record the enable in ~/.thepia/config.toml.
        env.set_enabled(self.id(), true).map_err(|e| {
            Diagnostic::error(
                "ls-enable",
                "could not record the enable in ~/.thepia/config.toml",
            )
            .cause(e.to_string())
            .exit(Exit::Generic)
        })?;

        let mut outcome = SubjectOutcome::new().note(installed_note);

        // 3. PATH guidance: the editor extension defaults to `thepiad` on PATH, so ~/.thepia/bin
        // must be reachable. Only warn when it is not already there.
        if !dir_on_path(&env.bin_dir()) {
            outcome = outcome.note(
                Diagnostic::warning(
                    "ls-path",
                    format!("{} is not on PATH", env.bin_dir().display()),
                )
                .cause("VS Code's default `thepia.lsp.serverPath` is `thepiad`, resolved via PATH")
                .help(format!(
                    "add it to your shell profile: export PATH=\"{}:$PATH\" \
                     (or set `thepia.lsp.serverPath` to {} in VS Code settings)",
                    env.bin_dir().display(),
                    dest.display()
                )),
            );
        }

        // 4. The editor extension (the piece that actually spawns the server per session). Point at
        // the right next step; tailor it to whether the `code` CLI is present.
        let ext_help = if which("code").is_some() {
            "install the VS Code extension from editors/vscode: \
             `cd editors/vscode && pnpm install && pnpm run compile`, then press F5 to try it, \
             or package it (`vsce package`) and `code --install-extension thepia-agent-config-*.vsix`"
        } else {
            "in VS Code, install the Thepia Agent Config extension (editors/vscode); it spawns \
             `thepiad --lsp` for SKILL.md and .claude/agents/*.md files"
        };
        outcome = outcome.note(
            Diagnostic::note("ls-editor", "enable live diagnostics in your editor").help(ext_help),
        );

        Ok(outcome)
    }

    fn deactivate(&self, env: &UseEnv) -> Result<SubjectOutcome, Diagnostic> {
        env.set_enabled(self.id(), false).map_err(|e| {
            Diagnostic::error("ls-disable", "could not update ~/.thepia/config.toml")
                .cause(e.to_string())
                .exit(Exit::Generic)
        })?;
        Ok(SubjectOutcome::new().note(
            Diagnostic::note(
                "ls-disabled",
                "language-server disabled; the installed binary was left in place",
            )
            .help("remove the Thepia Agent Config extension in VS Code to stop live diagnostics"),
        ))
    }
}

/// Whether `dir` is one of the entries on `$PATH` (compared by canonicalized path when possible).
fn dir_on_path(dir: &Path) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    let want = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    std::env::split_paths(&path).any(|p| {
        let p = p.canonicalize().unwrap_or(p);
        p == want
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_home(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("the-use-ls-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// activate installs the located binary, records the enable, and is reflected by probe; a second
    /// activate is idempotent; deactivate clears the enable but keeps the binary.
    #[test]
    fn activate_installs_and_enables_then_deactivate_keeps_binary() {
        let home = temp_home("activate");

        // A fake `thepiad` beside a fake `thepia` exe, so locate_source finds a sibling.
        let fake_bin = home.join("dist");
        std::fs::create_dir_all(&fake_bin).unwrap();
        let fake_thepiad = fake_bin.join("thepiad");
        std::fs::write(&fake_thepiad, b"#!/bin/sh\n").unwrap();
        let env = UseEnv::at(&home).with_exe(fake_bin.join("thepia"));

        // Not installed, but discoverable.
        let p = LanguageServer.probe(&env);
        assert!(!p.installed);
        assert!(p.detail.unwrap().contains("available to install"));

        let out = LanguageServer.activate(&env).unwrap();
        assert!(LanguageServer::dest(&env).is_file(), "binary installed");
        assert_eq!(env.enabled_subjects(), vec!["language-server".to_string()]);
        assert!(out.notes.iter().any(|n| n.code == "ls-installed"));
        assert!(out.notes.iter().any(|n| n.code == "ls-editor"));

        // Idempotent: re-activate reports already-installed, stays enabled.
        let out2 = LanguageServer.activate(&env).unwrap();
        assert!(
            out2.notes
                .iter()
                .any(|n| n.title.contains("already at") || n.code == "ls-installed")
        );
        assert_eq!(env.enabled_subjects(), vec!["language-server".to_string()]);
        assert!(LanguageServer.probe(&env).installed);

        // Deactivate clears the enable but leaves the installed binary.
        LanguageServer.deactivate(&env).unwrap();
        assert!(env.enabled_subjects().is_empty());
        assert!(LanguageServer::dest(&env).is_file(), "binary kept");
        assert!(LanguageServer.probe(&env).installed);

        let _ = std::fs::remove_dir_all(&home);
    }

    /// With no sibling and nothing on PATH, activate fails with a precise, fixable diagnostic.
    #[test]
    fn activate_without_a_binary_errors_clearly() {
        let home = temp_home("nobin");
        // An exe path whose parent has no `thepiad`; rely on PATH not having one either in CI.
        let env = UseEnv::at(&home).with_exe(home.join("nowhere").join("thepia"));
        if which(BINARY).is_some() {
            // A real thepiad on PATH would make this succeed; skip the negative assertion then.
            let _ = std::fs::remove_dir_all(&home);
            return;
        }
        let err = LanguageServer.activate(&env).unwrap_err();
        assert_eq!(err.code, "thepiad-not-found");
        assert_eq!(err.exit, Exit::NotFound);
        let _ = std::fs::remove_dir_all(&home);
    }
}
