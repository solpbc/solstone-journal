// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! End-to-end proof that a V2 install can upgrade itself.
//!
//! `install.sh`'s documented respin/upgrade route drops a newer build into a
//! fresh sibling `versions/<ver>-<digest>` directory and reflips `current`
//! onto it, leaving the old version directory in place. `journal setup`
//! must then re-admit the existing installation and repoint the managed
//! wrapper at the new build -- not refuse admission, and not silently leave
//! the wrapper pointed at the old one.
//!
//! This deliberately never sets `SETUP_FIXTURE_BIN_DIR`, unlike every other
//! setup-process test in this crate. That override is exactly what hides
//! this defect: it substitutes a literal, caller-supplied path for what a
//! real `journal` binary computes from `std::env::current_exe()`. On Linux
//! that reads `/proc/self/exe`, which the kernel keeps fully dereferenced,
//! so a binary launched through `current/bin/journal` never actually sees
//! `"current"` in its own executable directory -- it sees whatever
//! directory `current` resolves to. Letting the fixture resolve its own
//! path, exactly like the shipped binary does, is what makes this test see
//! what the founder's machine saw.

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::Command;

fn locate_workspace_binary(package: &str, binary: &str) -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_manifest = manifest_dir
        .parent()
        .expect("crates dir")
        .parent()
        .expect("core dir")
        .join("Cargo.toml");
    let status = Command::new(env!("CARGO"))
        .args(["build", "--manifest-path"])
        .arg(&workspace_manifest)
        .args(["-p", package, "--bin", binary])
        .status()
        .expect("run cargo build");
    assert!(
        status.success(),
        "cargo build -p {package} --bin {binary} failed"
    );
    workspace_manifest
        .parent()
        .expect("core dir")
        .join("target/debug")
        .join(binary)
}

fn write_layout_anchors(share: &Path) {
    for relative in [
        "solstone/talent/journal/contract/bundle.json",
        "solstone/think/contract/layout.json",
        "solstone/think/templates/segment_preamble.md",
    ] {
        let path = share.join(relative);
        fs::create_dir_all(path.parent().expect("anchor parent")).expect("create anchor dir");
        fs::write(&path, relative).expect("write layout anchor");
    }
}

/// Builds `<prefix>/versions/<version_dir>/{bin,share}` the way `install.sh`
/// lays out a real release tree, using the setup-fixture binary as a stand-in
/// for both `journal` and `solstone` so no real subprocess (models, skills,
/// service) is ever invoked for real.
fn make_version(prefix: &Path, fixture: &Path, version_dir: &str) -> PathBuf {
    let dir = prefix.join("versions").join(version_dir);
    fs::create_dir_all(dir.join("bin")).expect("create version bin dir");
    for binary in ["journal", "solstone"] {
        let dest = dir.join("bin").join(binary);
        fs::copy(fixture, &dest).expect("copy fixture binary");
        fs::set_permissions(&dest, fs::Permissions::from_mode(0o755)).expect("make executable");
    }
    write_layout_anchors(&dir.join("share"));
    dir
}

fn run_setup(
    journal_bin: &Path,
    home: &Path,
    journal_path: &Path,
    extra_args: &[&str],
) -> std::process::Output {
    let empty_path = home.parent().expect("home parent").join("empty-path");
    fs::create_dir_all(&empty_path).expect("create empty PATH dir");
    Command::new(journal_bin)
        .arg("setup")
        .args(extra_args)
        .arg("--yes")
        .env("HOME", home)
        .env("SOLSTONE_JOURNAL", journal_path)
        .env("PATH", &empty_path)
        .output()
        .expect("run journal setup")
}

#[test]
fn v2_self_upgrade_repoints_wrapper_after_a_same_version_respin() {
    let fixture = locate_workspace_binary("solstone-core-journal-bin", "setup-fixture-journal");
    let root =
        std::env::temp_dir().join(format!("solstone-v2-self-upgrade-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("create scratch root");
    let prefix = root.join("prefix");
    let home = root.join("home");
    let journal_path = home.join("journal");
    fs::create_dir_all(&prefix).expect("create prefix");

    let first = make_version(&prefix, &fixture, "2.0.0-aaaaaaaaaaaa");
    symlink("versions/2.0.0-aaaaaaaaaaaa", prefix.join("current")).expect("link current first");

    let output = run_setup(
        &prefix.join("current/bin/journal"),
        &home,
        &journal_path,
        &[],
    );
    assert!(
        output.status.success(),
        "first (fresh) setup failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // The documented respin/upgrade route: a new sibling version directory,
    // `current` reflipped onto it, the old directory left on disk.
    let second = make_version(&prefix, &fixture, "2.0.0-bbbbbbbbbbbb");
    fs::remove_file(prefix.join("current")).expect("remove current first");
    symlink("versions/2.0.0-bbbbbbbbbbbb", prefix.join("current")).expect("link current second");

    let output = run_setup(
        &prefix.join("current/bin/journal"),
        &home,
        &journal_path,
        &["--accept-existing-journal"],
    );
    assert!(
        output.status.success(),
        "second setup, after a same-version respin, refused or failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let wrapper = fs::read_to_string(home.join(".local/bin/journal")).expect("read wrapper");
    let sol_bin_line = wrapper
        .lines()
        .find(|line| line.starts_with("SOL_BIN="))
        .expect("wrapper carries SOL_BIN");
    assert!(
        sol_bin_line.contains(second.join("bin/journal").to_str().expect("utf8 path")),
        "wrapper was not repointed at the newly installed build: {sol_bin_line}"
    );
    assert!(
        !sol_bin_line.contains(first.join("bin/journal").to_str().expect("utf8 path")),
        "wrapper still names the old build: {sol_bin_line}"
    );

    let _ = fs::remove_dir_all(&root);
}
