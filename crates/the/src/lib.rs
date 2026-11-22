//! `thepia` (`the`): the Thepia command line for nurturing local AI agents.
//!
//! Output contract (same as thepia/cli): stdout carries only the command's result; diagnostics,
//! warnings and notes go to stderr. `--format human|json` is the single switch for both.

mod cli;

use std::io;
use std::process::ExitCode;

use the_output::command::{Registry, StreamState, dispatch};

/// The built-in command registry: every command group is its own crate, registered here as a
/// [`the_output::command::Target`]. Adding a group is a `register` + a crate dep, not a dispatch arm.
fn registry() -> Registry {
    Registry::new().register(Box::new(the_skills::Skills))
}

pub fn run() -> ExitCode {
    ensure_process_name();
    let mut out = io::stdout().lock();
    let mut err = io::stderr().lock();
    let code = dispatch(
        &registry(),
        cli::root_command(),
        std::env::args_os(),
        StreamState::stdout(),
        StreamState::stderr(),
        &mut out,
        &mut err,
    );
    ExitCode::from(code)
}

/// Ensure the running process appears as `thepia` in OS process lists (`ps`, `top`, Activity Monitor)
/// even when invoked via the `the` or `th` command names.
fn ensure_process_name() {
    #[cfg(target_os = "linux")]
    let _ = std::fs::write("/proc/self/comm", "thepia\n");

    #[cfg(unix)]
    {
        let Some(arg0) = std::env::args_os().next() else {
            return;
        };
        let Ok(exe) = std::env::current_exe() else {
            return;
        };
        if let Some(target) = rename_target(std::path::Path::new(&arg0), &exe) {
            use std::os::unix::process::CommandExt;
            let _ = std::process::Command::new(&target)
                .args(std::env::args_os().skip(1))
                .exec();
        }
    }
}

/// Decide whether to re-exec as the sibling `thepia` binary, and to which path. Returns `None` when
/// we were already invoked as `thepia` or no sibling `thepia` file exists next to `exe` (directly or
/// after resolving symlinks). Split from [`ensure_process_name`] so the resolution is testable without
/// the `exec` that replaces the process.
#[cfg(unix)]
fn rename_target(arg0: &std::path::Path, exe: &std::path::Path) -> Option<std::path::PathBuf> {
    use std::path::PathBuf;
    if arg0.file_name().and_then(|s| s.to_str()) == Some("thepia") {
        return None;
    }
    let sibling = |dir: &std::path::Path| -> Option<PathBuf> {
        let p = dir.join("thepia");
        p.is_file().then_some(p)
    };
    exe.parent()
        .and_then(sibling)
        .or_else(|| sibling(exe.canonicalize().ok()?.parent()?))
}

#[cfg(all(test, unix))]
mod rename_target_tests {
    use super::rename_target;
    use std::fs;
    use std::path::{Path, PathBuf};

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("the-rename-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn already_invoked_as_thepia_needs_no_reexec() {
        let dir = tmp("noop");
        fs::write(dir.join("thepia"), b"x").unwrap();
        // Even with a sibling present, arg0 == thepia short-circuits to None.
        assert_eq!(
            rename_target(Path::new("/usr/bin/thepia"), &dir.join("thepia")),
            None
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolves_sibling_thepia_next_to_exe() {
        let dir = tmp("sibling");
        let thepia = dir.join("thepia");
        fs::write(&thepia, b"x").unwrap();
        // Invoked as `the`, exe lives beside a real `thepia` file.
        assert_eq!(
            rename_target(Path::new("the"), &dir.join("the")),
            Some(thepia)
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_sibling_means_no_reexec() {
        let dir = tmp("absent");
        // `the` with no `thepia` beside it (and no resolvable symlink target) -> None.
        assert_eq!(rename_target(Path::new("th"), &dir.join("th")), None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolves_sibling_thepia_after_canonicalizing_a_symlinked_exe() {
        // exe is a symlink in a dir with no `thepia`; its canonical target's dir holds `thepia`. This
        // exercises the canonicalize fallback (the second arm of `or_else`).
        let dir = tmp("canon");
        let real = dir.join("real");
        fs::create_dir_all(&real).unwrap();
        let thepia = real.join("thepia");
        fs::write(&thepia, b"x").unwrap();
        fs::write(real.join("the"), b"x").unwrap();
        let exe = dir.join("the");
        std::os::unix::fs::symlink(real.join("the"), &exe).unwrap();
        assert_eq!(
            rename_target(Path::new("the"), &exe),
            Some(real.canonicalize().unwrap().join("thepia"))
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
