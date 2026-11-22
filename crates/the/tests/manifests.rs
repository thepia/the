//! Workspace-manifest tripwires. These run inside `cargo test --workspace` (the CI gate), so a new
//! crate split cannot silently reintroduce the packaging regressions that the the-output / the-skills
//! split did (2026-11): an internal path dependency with no `version`, or a crate directory left out
//! of the workspace. Deterministic, offline (`cargo metadata --no-deps` reads only local manifests).

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

use serde_json::Value;

/// `cargo metadata --no-deps` for this workspace (members only, no registry resolution).
fn metadata() -> Value {
    let out = Command::new(env!("CARGO"))
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .output()
        .expect("run cargo metadata");
    assert!(
        out.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("parse cargo metadata json")
}

#[test]
fn internal_path_deps_declare_a_version() {
    // A path dependency without a `version` makes `cargo publish` fail with "all dependencies must
    // have a version requirement specified when publishing". cargo reports such a dep with `path`
    // set and a `req` of "*". Require every local dependency to also carry a version requirement.
    let meta = metadata();
    let mut offenders = Vec::new();
    for pkg in meta["packages"].as_array().unwrap() {
        let pkg_name = pkg["name"].as_str().unwrap();
        for dep in pkg["dependencies"].as_array().unwrap() {
            let is_local = dep["path"].is_string();
            let no_version = dep["req"].as_str() == Some("*");
            if is_local && no_version {
                let dep_name = dep["name"].as_str().unwrap();
                offenders.push(format!(
                    "{pkg_name} -> {dep_name}: path dependency with no `version`"
                ));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "internal path dependencies must also specify a `version` (else `cargo publish` fails); \
         add `version = \"<workspace version>\"`:\n  {}",
        offenders.join("\n  ")
    );
}

#[test]
fn every_crate_directory_is_a_workspace_member() {
    // A new crate under crates/ that is left out of `[workspace] members` is silently not built,
    // tested, or linted. Compare the filesystem against the members cargo actually sees.
    let meta = metadata();
    let root = meta["workspace_root"].as_str().unwrap();
    let member_dirs: BTreeSet<String> = meta["packages"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|p| p["manifest_path"].as_str())
        .filter_map(|m| {
            Path::new(m)
                .parent()
                .map(|d| d.to_string_lossy().into_owned())
        })
        .collect();

    let crates_dir = Path::new(root).join("crates");
    let mut missing = Vec::new();
    for entry in std::fs::read_dir(&crates_dir).expect("read crates/") {
        let dir = entry.unwrap().path();
        if dir.join("Cargo.toml").is_file()
            && !member_dirs.contains(&dir.to_string_lossy().into_owned())
        {
            missing.push(dir.file_name().unwrap().to_string_lossy().into_owned());
        }
    }
    assert!(
        missing.is_empty(),
        "crate directories not in the workspace `members`: {missing:?}; add them to the root Cargo.toml"
    );
}
