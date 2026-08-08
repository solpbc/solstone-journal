// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};
use std::{env, fs, path::PathBuf};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_solstone-core")
}

fn temp_path(name: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be available")
        .as_nanos();
    env::temp_dir().join(format!("solstone-core-{name}-{stamp}"))
}

#[test]
fn version_writes_stdout_and_exits_zero() {
    let output = Command::new(bin())
        .arg("--version")
        .output()
        .expect("solstone-core should execute");

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(output.stdout).expect("stdout should be utf-8"),
        format!("solstone-core {}\n", env!("CARGO_PKG_VERSION"))
    );
    assert_eq!(
        String::from_utf8(output.stderr).expect("stderr should be utf-8"),
        ""
    );
}

#[test]
fn usage_error_writes_stderr_and_exits_64() {
    let output = Command::new(bin())
        .arg("--unknown")
        .output()
        .expect("solstone-core should execute");

    assert_eq!(output.status.code(), Some(64));
    assert_eq!(
        String::from_utf8(output.stdout).expect("stdout should be utf-8"),
        ""
    );
    assert_eq!(
        String::from_utf8(output.stderr).expect("stderr should be utf-8"),
        solstone_core_cli::USAGE
    );
}

#[test]
fn journal_path_override_prints_cli_label_without_creating() {
    let target = temp_path("override-no-create");
    let output = Command::new(bin())
        .arg("journal-path")
        .arg("--journal")
        .arg(&target)
        .output()
        .expect("solstone-core should execute");

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(output.stdout).expect("stdout should be utf-8"),
        format!("cli\t{}\n", target.display())
    );
    assert_eq!(
        String::from_utf8(output.stderr).expect("stderr should be utf-8"),
        ""
    );
    assert!(!target.exists());
}

#[test]
fn journal_path_override_create_creates_directory() {
    let target = temp_path("override-create");
    let output = Command::new(bin())
        .arg("journal-path")
        .arg("--journal")
        .arg(&target)
        .arg("--create")
        .output()
        .expect("solstone-core should execute");

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(output.stdout).expect("stdout should be utf-8"),
        format!("cli\t{}\n", target.display())
    );
    assert!(target.is_dir());
    fs::remove_dir_all(target).expect("cleanup created journal");
}

#[test]
fn journal_path_empty_override_prints_but_create_errors() {
    let output = Command::new(bin())
        .arg("journal-path")
        .arg("--journal")
        .arg("")
        .output()
        .expect("solstone-core should execute");
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(output.stdout).expect("stdout should be utf-8"),
        "cli\t\n"
    );

    let create_output = Command::new(bin())
        .arg("journal-path")
        .arg("--journal")
        .arg("")
        .arg("--create")
        .output()
        .expect("solstone-core should execute");
    assert_eq!(create_output.status.code(), Some(75));
    assert_eq!(
        String::from_utf8(create_output.stdout).expect("stdout should be utf-8"),
        ""
    );
    assert!(
        String::from_utf8(create_output.stderr)
            .expect("stderr should be utf-8")
            .starts_with("could not create journal directory (cli): ")
    );
}

#[test]
fn journal_path_env_spaces_are_unstripped() {
    let output = Command::new(bin())
        .arg("journal-path")
        .env("SOLSTONE_JOURNAL", "   ")
        .output()
        .expect("solstone-core should execute");

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(output.stdout).expect("stdout should be utf-8"),
        "env\t   \n"
    );
}

#[test]
fn journal_path_config_tilde_is_literal() {
    let home = temp_path("config-tilde-home");
    let config_dir = home.join(".config").join("solstone");
    fs::create_dir_all(&config_dir).expect("create config dir");
    fs::write(config_dir.join("config.toml"), "journal = \"~/journal\"\n").expect("write config");

    let output = Command::new(bin())
        .arg("journal-path")
        .env_remove("SOLSTONE_JOURNAL")
        .env("HOME", &home)
        .output()
        .expect("solstone-core should execute");

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(output.stdout).expect("stdout should be utf-8"),
        "config\t~/journal\n"
    );
    fs::remove_dir_all(home).expect("cleanup config home");
}
