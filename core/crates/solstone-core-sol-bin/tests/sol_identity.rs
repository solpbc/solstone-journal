// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

const CLI_BOUNDARY_JSON: &str = include_str!("../../../fixtures/native-sol/cli-boundary-v1.json");

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(name: &str) -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be available")
            .as_nanos();
        let path = env::temp_dir().join(format!(
            "solstone-core-{name}-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("create temporary test directory");
        Self { path }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_solstone-core-sol")
}

fn write_forbidden_shim(path: &Path, sentinel: &Path) {
    let script = format!(
        "#!/bin/sh\nprintf '%s %s\\n' \"$0\" \"$*\" >> '{}'\nexit 97\n",
        sentinel.display()
    );
    fs::write(path, script).expect("write forbidden interpreter shim");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
        .expect("make forbidden interpreter shim executable");
}

fn poison_path(temp: &TempDir) -> (PathBuf, PathBuf) {
    let shims = temp.path.join("shims");
    let sentinel = temp.path.join("sentinel.log");
    fs::create_dir(&shims).expect("create shim directory");
    for name in ["python", "python3"] {
        write_forbidden_shim(&shims.join(name), &sentinel);
    }
    (shims, sentinel)
}

fn assert_sentinel_untouched(sentinel: &Path) {
    assert!(
        !sentinel.exists()
            || fs::read_to_string(sentinel)
                .expect("read sentinel")
                .is_empty(),
        "sol identity invoked a forbidden interpreter: {}",
        fs::read_to_string(sentinel).unwrap_or_default(),
    );
}

fn run_sol(args: &[&str], path: &Path, journal: Option<&Path>) -> Output {
    let mut command = Command::new(bin());
    command
        .args(args)
        .env("PATH", path)
        .env_remove("HOME")
        .env_remove("SOLSTONE_JOURNAL");
    if let Some(journal) = journal {
        command.env("SOLSTONE_JOURNAL", journal);
    }
    command.output().expect("solstone-core should execute")
}

fn fixture() -> Value {
    serde_json::from_str(CLI_BOUNDARY_JSON).expect("parse CLI boundary fixture")
}

fn strings<'a>(value: &'a Value, field: &str) -> Vec<&'a str> {
    value[field]
        .as_array()
        .expect("fixture field must be an array")
        .iter()
        .map(|entry| entry.as_str().expect("fixture entry must be a string"))
        .collect()
}

fn assert_unique(values: &[&str], field: &str) {
    let unique = values.iter().copied().collect::<BTreeSet<_>>();
    assert_eq!(unique.len(), values.len(), "{field} contains duplicates");
}

#[test]
fn sol_binary_owns_only_the_sol_identity() {
    let temp = TempDir::new("sol-process-identity");
    let (path, sentinel) = poison_path(&temp);

    let version = run_sol(&["--version"], &path, None);
    assert_eq!(version.status.code(), Some(0));
    assert!(
        String::from_utf8(version.stdout)
            .expect("version stdout should be utf-8")
            .starts_with("sol (solstone) ")
    );

    let journal_marker = run_sol(&["__solstone_identity=journal", "--version"], &path, None);
    assert_ne!(journal_marker.status.code(), Some(0));
    assert!(!String::from_utf8_lossy(&journal_marker.stdout).starts_with("journal (solstone) "));
    assert_sentinel_untouched(&sentinel);
}

#[test]
fn cli_boundary_declares_the_complete_sol_partition() {
    let fixture = fixture();
    let sol = &fixture["identities"]["sol"];
    let api = strings(sol, "api_commands");
    let device = strings(sol, "invoking_device_commands");
    let journal_leaves = strings(sol, "http_paths");

    assert_eq!(api.len(), 4);
    assert_eq!(device.len(), 3);
    assert_eq!(journal_leaves.len(), 17);
    assert_unique(&api, "api_commands");
    assert_unique(&device, "invoking_device_commands");
    assert_unique(&journal_leaves, "http_paths");
    assert_eq!(
        api.iter().copied().collect::<BTreeSet<_>>(),
        BTreeSet::from(["call", "chat", "import", "status"])
    );
    assert_eq!(
        device.iter().copied().collect::<BTreeSet<_>>(),
        BTreeSet::from(["link", "root", "skills"])
    );
    assert!(api.iter().all(|command| !device.contains(command)));
    assert!(
        journal_leaves
            .iter()
            .all(|path| path.starts_with("call journal "))
    );

    let temp = TempDir::new("sol-api-help");
    let (path, sentinel) = poison_path(&temp);
    for command in api {
        let output = run_sol(&[command, "--help"], &path, None);
        assert_eq!(output.status.code(), Some(0), "sol {command} --help");
        let stdout = String::from_utf8(output.stdout).expect("help stdout should be utf-8");
        assert!(
            stdout.starts_with(&format!("Usage: sol {command}")),
            "sol {command} --help did not reach generated help: {stdout}"
        );
        assert!(
            !stdout.contains("Unsupported native sol command."),
            "sol {command} --help fell through to unsupported output"
        );
        assert_eq!(output.stderr, b"", "sol {command} --help");
    }
    assert_sentinel_untouched(&sentinel);
}

#[test]
fn fixture_retired_sol_invocations_are_unsupported_without_spawning() {
    let fixture = fixture();
    let retired = strings(&fixture["identities"]["sol"], "retired_invocations");
    assert_eq!(retired.len(), 11);
    assert_unique(&retired, "retired_invocations");

    let temp = TempDir::new("sol-retired");
    let (path, sentinel) = poison_path(&temp);
    for invocation in retired {
        let args = invocation.split_ascii_whitespace().collect::<Vec<_>>();
        let output = run_sol(&args, &path, None);
        assert!(
            !output.status.success(),
            "retired sol invocation succeeded: {invocation}"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !stderr.contains("panicked at") && !stderr.contains("thread 'main' panicked"),
            "retired sol invocation crashed: {invocation}: {stderr}"
        );
    }
    assert_sentinel_untouched(&sentinel);
}

#[test]
fn status_never_reads_the_journal_local_port_file() {
    let temp = TempDir::new("sol-status");
    let (path, sentinel) = poison_path(&temp);
    let journal = temp.path.join("journal");
    fs::create_dir_all(journal.join("health")).expect("create journal health directory");
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind journal-local decoy port");
    listener
        .set_nonblocking(true)
        .expect("make decoy listener nonblocking");
    let port = listener.local_addr().expect("read reserved port").port();
    fs::write(journal.join("health/convey.port"), port.to_string())
        .expect("write decoy convey port");

    let _output = run_sol(&["status"], &path, Some(&journal));
    assert_eq!(
        listener
            .accept()
            .expect_err("sol must not dial journal-local decoy")
            .kind(),
        std::io::ErrorKind::WouldBlock
    );
    assert_sentinel_untouched(&sentinel);
}

#[test]
fn bare_help_never_resolves_a_journal() {
    let temp = TempDir::new("sol-bare-help");
    let (path, sentinel) = poison_path(&temp);

    for args in [Vec::new(), vec!["--help"]] {
        let output = run_sol(&args, &path, None);
        assert_eq!(output.status.code(), Some(0), "sol {args:?}");
        let stdout = String::from_utf8(output.stdout).expect("help stdout should be utf-8");
        assert!(!stdout.contains("Journal:"), "sol {args:?}");
        assert!(!stdout.contains("Days:"), "sol {args:?}");
        assert_eq!(output.stderr, b"", "sol {args:?}");
    }
    assert_sentinel_untouched(&sentinel);
}
