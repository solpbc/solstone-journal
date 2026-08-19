// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Host permission bits and a process-local HOME cannot share the lib harness.

#![cfg(unix)]

use std::env;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

#[test]
fn mixed_writability_installs_writable_agent_and_reports_unwritable_agent() {
    let home_guard = tempfile::tempdir().expect("tempdir");
    let claude_target = home_guard.path().join(".claude/skills/solstone");
    let output = Command::new(env::current_exe().unwrap())
        .args(["--exact", "mixed_writability_child", "--nocapture"])
        .env("HOME", home_guard.path())
        .env("SOLSTONE_SKILL_INSTALL_PERMISSIONS", "1")
        .current_dir(home_guard.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "child failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains(&format!("error: install {}", claude_target.display()))
    );
}

#[test]
fn mixed_writability_child() {
    if env::var_os("SOLSTONE_SKILL_INSTALL_PERMISSIONS").is_none() {
        return;
    }
    let home = PathBuf::from(env::var_os("HOME").expect("HOME"));
    let claude_target = home.join(".claude/skills/solstone");
    fs::create_dir_all(&claude_target).expect("claude target");
    fs::write(claude_target.join("marker.txt"), "pre-existing").expect("marker");
    let before = listing(&claude_target);

    fs::set_permissions(&claude_target, fs::Permissions::from_mode(0o500)).expect("chmod 0500");
    if let Ok(()) = fs::write(claude_target.join(".write-probe"), "x") {
        let _ = fs::remove_file(claude_target.join(".write-probe"));
        let _ = fs::set_permissions(&claude_target, fs::Permissions::from_mode(0o700));
        panic!(
            "skill_install_permissions: environment does not enforce directory permission bits (running as root or on an unsupported filesystem) — precondition cannot be established, refusing to report success"
        );
    }

    let codex_skills = home.join(".codex/skills");
    fs::create_dir_all(&codex_skills).expect("codex parent");
    let probe = codex_skills.join(".write-probe");
    if fs::write(&probe, "x").is_err() {
        let _ = fs::set_permissions(&claude_target, fs::Permissions::from_mode(0o700));
        panic!("skill_install_permissions: $HOME/.codex/skills is not writable");
    }
    fs::remove_file(&probe).expect("remove codex probe");

    let exit = solstone_core_sol::run(
        "sol",
        vec![OsString::from("skills"), OsString::from("install")],
    );

    fs::set_permissions(&claude_target, fs::Permissions::from_mode(0o700))
        .expect("restore before parent TempDir drop");

    assert_eq!(exit, ExitCode::from(1));
    assert_eq!(listing(&claude_target), before);
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../core/payload/solstone/talent/solstone");
    assert_eq!(
        fs::read(home.join(".codex/skills/solstone/SKILL.md")).unwrap(),
        fs::read(source.join("SKILL.md")).unwrap()
    );
    assert_eq!(
        fs::read(home.join(".codex/skills/solstone/references/commands.md")).unwrap(),
        fs::read(source.join("references/commands.md")).unwrap()
    );
}

fn listing(dir: &Path) -> Vec<String> {
    let mut names = fs::read_dir(dir)
        .expect("read dir")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect::<Vec<_>>();
    names.sort();
    names
}
