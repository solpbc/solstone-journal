// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::env;
use std::fs;
use std::io::ErrorKind;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::thread::sleep;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const ROOT_COMMANDS: &[&str] = &[
    "--path", "path", "status", "root", "doctor", "check", "contract", "notify",
];
const SERVICE_COMMANDS: &[&str] = &[
    "backfill-processing-records",
    "backup",
    "brain",
    "config",
    "convey",
    "cortex",
    "depict",
    "describe",
    "down",
    "engage",
    "export",
    "facet-candidates",
    "grab",
    "health",
    "heartbeat",
    "identity",
    "importer",
    "indexer",
    "install-models",
    "install-provider",
    "journal-stats",
    "maint",
    "maintenance",
    "navigate",
    "observer",
    "reprocess",
    "restart-convey",
    "schedule",
    "segment",
    "sense",
    "service",
    "settings",
    "setup",
    "spl",
    "start",
    "streams",
    "supervisor",
    "talent",
    "think",
    "top",
    "transcribe",
    "transfer",
    "up",
    "warm",
];

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
    env!("CARGO_BIN_EXE_solstone-core")
}

fn identity_arg(public_argv0: &str) -> String {
    format!("__solstone_identity={public_argv0}")
}

fn output_with_retry(args: &[String], path: Option<&Path>) -> Output {
    for _ in 0..100 {
        let mut command = Command::new(bin());
        command.args(args);
        if let Some(path) = path {
            command.env("PATH", path);
        }
        match command.output() {
            Ok(output) => return output,
            Err(error) if error.kind() == ErrorKind::ExecutableFileBusy => {
                sleep(Duration::from_millis(20));
            }
            Err(error) => panic!("solstone-core should execute: {error:?}"),
        }
    }
    panic!("solstone-core stayed busy after retries")
}

fn run_journal(args: &[&str], path: Option<&Path>) -> Output {
    let mut full = vec![identity_arg("journal")];
    full.extend(args.iter().map(|arg| (*arg).to_owned()));
    output_with_retry(&full, path)
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
    let path = format!(
        "{}:{}",
        shims.display(),
        env::var("PATH").expect("PATH must be set")
    );
    (PathBuf::from(path), sentinel)
}

fn assert_sentinel_untouched(sentinel: &Path) {
    assert!(
        !sentinel.exists()
            || fs::read_to_string(sentinel)
                .expect("read sentinel")
                .is_empty(),
        "journal identity invoked a forbidden interpreter: {}",
        fs::read_to_string(sentinel).unwrap_or_default(),
    );
}

#[test]
fn journal_identity_is_distinct_from_sol_identity() {
    let sol = output_with_retry(&[identity_arg("sol"), "--version".to_owned()], None);
    let journal = run_journal(&["--version"], None);

    assert_eq!(sol.status.code(), Some(0));
    assert_eq!(journal.status.code(), Some(0));
    let sol_stdout = String::from_utf8(sol.stdout).expect("sol stdout should be utf-8");
    let journal_stdout = String::from_utf8(journal.stdout).expect("journal stdout should be utf-8");
    assert!(sol_stdout.starts_with("sol (solstone) "));
    assert!(journal_stdout.starts_with("journal (solstone) "));
    assert_ne!(sol_stdout, journal_stdout);

    let sol_think = output_with_retry(&[identity_arg("sol"), "think".to_owned()], None);
    let journal_think = run_journal(&["think"], None);
    assert_eq!(sol_think.status.code(), Some(2));
    assert!(
        String::from_utf8(sol_think.stderr)
            .expect("sol stderr should be utf-8")
            .contains("'think' moved to 'journal think' — run that instead.")
    );
    assert_eq!(journal_think.status.code(), Some(69));
    assert!(
        String::from_utf8(journal_think.stderr)
            .expect("journal stderr should be utf-8")
            .contains("journal_command_unavailable")
    );
}

#[test]
fn journal_identity_marks_all_known_tokens_unavailable_without_spawning() {
    let temp = TempDir::new("journal-known-tokens");
    let (path, sentinel) = poison_path(&temp);

    for token in ROOT_COMMANDS.iter().chain(SERVICE_COMMANDS.iter()) {
        let output = run_journal(&[token], Some(&path));
        assert_eq!(output.status.code(), Some(69), "{token}");
        assert_eq!(output.stdout, b"", "{token}");
        let stderr = String::from_utf8(output.stderr).expect("stderr should be utf-8");
        assert!(stderr.contains(token), "{token}: {stderr}");
        assert!(
            stderr.contains("journal_command_unavailable"),
            "{token}: {stderr}"
        );
    }
    assert_eq!(ROOT_COMMANDS.len() + SERVICE_COMMANDS.len(), 52);
    assert_sentinel_untouched(&sentinel);
}

#[test]
fn journal_identity_rejects_dotted_modules_and_unknown_without_spawning() {
    let temp = TempDir::new("journal-rejected-tokens");
    let (path, sentinel) = poison_path(&temp);

    for token in [
        "solstone.think.supervisor",
        "a.b",
        "totally-unknown-command",
    ] {
        let output = run_journal(&[token], Some(&path));
        assert_eq!(output.status.code(), Some(64), "{token}");
        assert_eq!(output.stdout, b"", "{token}");
        assert_eq!(
            String::from_utf8(output.stderr).expect("stderr should be utf-8"),
            solstone_core_journal_cli::JOURNAL_USAGE,
            "{token}"
        );
    }
    assert_sentinel_untouched(&sentinel);
}

#[test]
fn journal_identity_marker_is_exact_and_first_only() {
    let malformed = output_with_retry(&["__solstone_identity=journal-typo".to_owned()], None);
    assert_eq!(malformed.status.code(), Some(64));
    assert_eq!(malformed.stdout, b"");
    assert_eq!(
        String::from_utf8(malformed.stderr).expect("stderr should be utf-8"),
        solstone_core_cli::USAGE
    );

    let later_marker = output_with_retry(
        &[
            "journal-path".to_owned(),
            "__solstone_identity=journal".to_owned(),
        ],
        None,
    );
    assert_eq!(later_marker.status.code(), Some(64));
    assert_eq!(later_marker.stdout, b"");
    assert_eq!(
        String::from_utf8(later_marker.stderr).expect("stderr should be utf-8"),
        solstone_core_cli::USAGE
    );

    let journal_path = env::temp_dir().join(format!(
        "solstone-core-journal-identity-path-{}",
        std::process::id()
    ));
    let raw_journal_path = output_with_retry(
        &[
            "journal-path".to_owned(),
            "--journal".to_owned(),
            journal_path.display().to_string(),
        ],
        None,
    );
    assert_eq!(raw_journal_path.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(raw_journal_path.stdout).expect("stdout should be utf-8"),
        format!("cli\t{}\n", journal_path.display())
    );
    assert_eq!(raw_journal_path.stderr, b"");
    assert!(!journal_path.exists());
}

fn repo_root() -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .expect("workspace checkout root")
        .to_path_buf();
    assert!(
        root.join("Makefile").is_file(),
        "repo root must contain Makefile"
    );
    root
}

fn is_binary_target(target: &serde_json::Value) -> bool {
    target["kind"]
        .as_array()
        .is_some_and(|kinds| kinds.iter().any(|kind| kind.as_str() == Some("bin")))
}

#[test]
fn cargo_metadata_confirms_single_public_identity_binary() {
    let root = repo_root();
    let output = Command::new("cargo")
        .args([
            "metadata",
            "--format-version",
            "1",
            "--no-deps",
            "--manifest-path",
            "core/Cargo.toml",
        ])
        .current_dir(root)
        .output()
        .expect("cargo metadata should execute");
    assert!(
        output.status.success(),
        "cargo metadata failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("cargo metadata must be valid JSON");
    let packages = metadata["packages"]
        .as_array()
        .expect("metadata packages must be an array");
    let binaries = packages
        .iter()
        .flat_map(|package| {
            let package_name = package["name"]
                .as_str()
                .expect("package name must be a string");
            package["targets"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|target| is_binary_target(target))
                .map(move |target| {
                    (
                        package_name,
                        target["name"]
                            .as_str()
                            .expect("target name must be a string"),
                    )
                })
        })
        .collect::<Vec<_>>();
    assert!(
        binaries
            .iter()
            .all(|(_package, target)| { !matches!(*target, "sol" | "solstone" | "journal") })
    );

    let solstone_core = binaries
        .iter()
        .filter(|(_package, target)| *target == "solstone-core")
        .collect::<Vec<_>>();
    assert_eq!(solstone_core.len(), 1);
    assert_eq!(solstone_core[0].0, "solstone-core");

    let journal_cli = packages
        .iter()
        .find(|package| package["name"] == "solstone-core-journal-cli")
        .expect("solstone-core-journal-cli package must be present");
    assert!(
        journal_cli["targets"]
            .as_array()
            .expect("journal CLI targets must be an array")
            .iter()
            .all(|target| !is_binary_target(target))
    );
}
