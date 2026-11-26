//! Version-aware binary install into `~/.thepia/bin`.
//!
//! A use subject installs its binary with [`ensure`], which compares versions and **never overwrites
//! a destination that is newer than or equal to the source**: it installs when absent, upgrades when
//! the source is strictly newer, and keeps the existing binary otherwise. Versions come from running
//! `<binary> --version` and reading the trailing dotted-number token (`thepiad 0.4.0` -> `0.4.0`), so
//! any binary that prints its version participates; one that cannot report a version is treated
//! conservatively (an existing destination is kept, not clobbered).

use std::path::Path;
use std::process::Command;

/// What [`ensure`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The destination was absent; the source was copied in.
    Installed { version: Option<String> },
    /// The source was strictly newer; the destination was overwritten.
    Upgraded {
        from: Option<String>,
        to: Option<String>,
    },
    /// The destination was newer than or equal to the source (or its version could not be read); it
    /// was left untouched.
    Kept {
        dest: Option<String>,
        src: Option<String>,
    },
}

/// Ensure `dest` holds a copy of `src` that is at least as new as `src`, without ever downgrading or
/// clobbering a newer destination. Creates the destination's parent directory as needed.
pub fn ensure(src: &Path, dest: &Path) -> std::io::Result<Outcome> {
    let src_v = binary_version(src);

    if !dest.exists() {
        copy(src, dest)?;
        return Ok(Outcome::Installed { version: src_v });
    }

    let dest_v = binary_version(dest);
    match (parse(src_v.as_deref()), parse(dest_v.as_deref())) {
        // Both versions known: upgrade only when the source is strictly newer.
        (Some(s), Some(d)) if s > d => {
            copy(src, dest)?;
            Ok(Outcome::Upgraded {
                from: dest_v,
                to: src_v,
            })
        }
        (Some(_), Some(_)) => Ok(Outcome::Kept {
            dest: dest_v,
            src: src_v,
        }),
        // A version could not be read on one side: be conservative and keep the existing binary
        // rather than risk overwriting a newer one we cannot evaluate.
        _ => Ok(Outcome::Kept {
            dest: dest_v,
            src: src_v,
        }),
    }
}

/// Copy `src` to `dest`, preserving the source's permission bits (so the executable stays
/// executable). Writes atomically enough for our use: a plain copy over any prior file.
fn copy(src: &Path, dest: &Path) -> std::io::Result<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(src, dest)?;
    Ok(())
}

/// Read a binary's version by running `<path> --version` and taking the trailing dotted-number token.
/// Returns `None` if it cannot be run, exits non-zero, or prints nothing version-like.
fn binary_version(path: &Path) -> Option<String> {
    let out = Command::new(path).arg("--version").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    text.split_whitespace()
        .rev()
        .find(|tok| parse(Some(tok)).is_some())
        .map(|s| s.to_string())
}

/// Parse a version string like `0.4.0` or `1.2` into comparable numeric components. Requires at least
/// two dotted numbers so a bare word is not mistaken for a version; non-digit suffixes on a component
/// (a `-rc1` tail) are dropped. `None` if it does not look like a version.
fn parse(s: Option<&str>) -> Option<Vec<u64>> {
    let s = s?;
    let nums: Vec<u64> = s
        .split('.')
        .map(|p| p.trim_start_matches(|c: char| !c.is_ascii_digit()))
        .map(|p| {
            p.chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
        })
        .filter(|p| !p.is_empty())
        .filter_map(|p| p.parse::<u64>().ok())
        .collect();
    (nums.len() >= 2).then_some(nums)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    fn temp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("the-use-install-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A fake executable that prints `thepiad <version>` for `--version`.
    fn fake_bin(dir: &Path, name: &str, version: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, format!("#!/bin/sh\necho \"thepiad {version}\"\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    #[test]
    fn parses_versions_and_orders_them() {
        assert_eq!(parse(Some("0.4.0")), Some(vec![0, 4, 0]));
        assert_eq!(parse(Some("1.2")), Some(vec![1, 2]));
        assert_eq!(parse(Some("thepiad")), None); // not version-like
        assert!(parse(Some("0.4.0")) < parse(Some("0.10.0")));
        assert!(parse(Some("1.0.0")) > parse(Some("0.9.9")));
    }

    #[test]
    fn installs_when_absent_then_keeps_equal_upgrades_older_keeps_newer() {
        let dir = temp_dir("flow");
        let bindir = dir.join("bin");
        let dest = bindir.join("thepiad");

        // Absent -> Installed.
        let src_040 = fake_bin(&dir, "src040", "0.4.0");
        assert_eq!(
            ensure(&src_040, &dest).unwrap(),
            Outcome::Installed {
                version: Some("0.4.0".into())
            }
        );
        assert!(dest.is_file());

        // Equal version -> Kept (no overwrite).
        assert!(matches!(
            ensure(&src_040, &dest).unwrap(),
            Outcome::Kept { .. }
        ));

        // Strictly newer source -> Upgraded.
        let src_050 = fake_bin(&dir, "src050", "0.5.0");
        assert_eq!(
            ensure(&src_050, &dest).unwrap(),
            Outcome::Upgraded {
                from: Some("0.4.0".into()),
                to: Some("0.5.0".into())
            }
        );

        // Older source against the now-0.5.0 dest -> Kept (never downgrade).
        let src_010 = fake_bin(&dir, "src010", "0.1.0");
        assert!(matches!(
            ensure(&src_010, &dest).unwrap(),
            Outcome::Kept {
                dest: Some(d),
                ..
            } if d == "0.5.0"
        ));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn keeps_existing_when_a_version_is_unreadable() {
        let dir = temp_dir("unknown");
        let dest = dir.join("bin").join("thepiad");
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        // A dest that does not respond to --version (no exec bit / not a program).
        std::fs::write(&dest, b"old-opaque-binary").unwrap();
        let src = fake_bin(&dir, "src", "9.9.9");
        assert!(matches!(ensure(&src, &dest).unwrap(), Outcome::Kept { .. }));
        // Untouched.
        assert_eq!(std::fs::read(&dest).unwrap(), b"old-opaque-binary");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
