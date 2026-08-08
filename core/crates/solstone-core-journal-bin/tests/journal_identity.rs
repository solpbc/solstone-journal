// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::env;
use std::fs;
use std::io::{ErrorKind, Read};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::thread::{self, sleep};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use sha2::{Digest, Sha256};

const LOCAL_OPS_JSON: &str = include_str!("../../../fixtures/journal-cli/local-ops-v1.json");
const LOCAL_OPS_SHA256: &str = "6f64998350f8088dcfce8b76e7393c38370b4f1e710501cdd16d0de019ee9fa6";
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
    env!("CARGO_BIN_EXE_solstone-core-journal")
}

fn local_ops_fixture() -> Value {
    serde_json::from_str(LOCAL_OPS_JSON).expect("parse journal local operations fixture")
}

fn local_op_paths(fixture: &Value) -> Vec<String> {
    fixture["commands"]
        .as_array()
        .expect("commands must be an array")
        .iter()
        .map(|command| {
            command["path"]
                .as_array()
                .expect("command path must be an array")
                .iter()
                .map(|token| token.as_str().expect("path token must be a string"))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect()
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
    let full = args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>();
    output_with_retry(&full, path)
}

fn run_journal_with_journal(args: &[&str], path: Option<&Path>, journal: &Path) -> Output {
    let full = args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>();
    for _ in 0..100 {
        let mut command = Command::new(bin());
        command.args(&full).env("SOLSTONE_JOURNAL", journal);
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

struct InstalledLayout {
    binary: PathBuf,
    bin: PathBuf,
    site_packages: PathBuf,
}

fn installed_layout(temp: &TempDir) -> InstalledLayout {
    let prefix = temp.path.join("prefix");
    let bin_dir = prefix.join("bin");
    let site_packages = prefix.join("lib/python3.95/site-packages");
    fs::create_dir_all(site_packages.join("solstone")).expect("create installed package fixture");
    fs::write(site_packages.join("solstone/__init__.py"), "").expect("write package marker");
    fs::create_dir_all(&bin_dir).expect("create installed binary directory");
    let binary = bin_dir.join("solstone-core-journal");
    fs::copy(bin(), &binary).expect("copy solstone-core-journal binary");
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o755))
        .expect("make copied binary executable");
    for (directory, name) in [
        ("solstone-1.2.3.dist-info", "solstone"),
        ("solstone_journal-1.2.3.dist-info", "solstone-journal"),
    ] {
        let dist_info = site_packages.join(directory);
        fs::create_dir(&dist_info).expect("create installed metadata fixture");
        fs::write(
            dist_info.join("METADATA"),
            format!("Name: {name}\nVersion: 1.2.3\n\n"),
        )
        .expect("write installed metadata fixture");
    }
    InstalledLayout {
        binary,
        bin: bin_dir,
        site_packages,
    }
}

fn write_recording_interpreter(path: &Path) {
    fs::write(
        path,
        "#!/bin/sh\nprintf '%s\\0' \"$@\" > \"$RECORD_FILE\"\nif [ -n \"${VERBOSE_RECORD_FILE:-}\" ]; then\n  printf '%s' \"$5\" > \"$VERBOSE_RECORD_FILE\"\nfi\nexec /bin/sleep 60\n",
    )
    .expect("write recording interpreter");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
        .expect("make recording interpreter executable");
}

fn installed_output(layout: &InstalledLayout, args: &[&str]) -> Output {
    for _ in 0..100 {
        let mut command = Command::new(&layout.binary);
        command.args(args);
        match command.output() {
            Ok(output) => return output,
            Err(error) if error.kind() == ErrorKind::ExecutableFileBusy => {
                sleep(Duration::from_millis(20));
            }
            Err(error) => panic!("installed solstone-core should execute: {error:?}"),
        }
    }
    panic!("installed solstone-core stayed busy after retries")
}

fn wait_for_record(path: &Path) -> Vec<Vec<u8>> {
    for _ in 0..100 {
        if let Ok(bytes) = fs::read(path) {
            return bytes
                .split(|byte| *byte == 0)
                .filter(|part| !part.is_empty())
                .map(Vec::from)
                .collect();
        }
        sleep(Duration::from_millis(20));
    }
    panic!("recording interpreter did not write argv")
}

#[test]
fn journal_binary_cannot_switch_to_the_sol_identity() {
    let journal = run_journal(&["--version"], None);

    assert_eq!(journal.status.code(), Some(0));
    let journal_stdout = String::from_utf8(journal.stdout).expect("journal stdout should be utf-8");
    assert!(journal_stdout.starts_with("journal (solstone) "));

    let sol_marker = output_with_retry(
        &["__solstone_identity=sol".to_owned(), "--version".to_owned()],
        None,
    );
    assert_eq!(sol_marker.status.code(), Some(64));
    assert_eq!(sol_marker.stdout, b"");
    assert_eq!(
        String::from_utf8(sol_marker.stderr).expect("stderr should be utf-8"),
        solstone_core_journal_cli::JOURNAL_USAGE
    );

    let journal_think = run_journal(&["think"], None);
    assert_eq!(journal_think.status.code(), Some(69));
    assert!(
        String::from_utf8(journal_think.stderr)
            .expect("journal stderr should be utf-8")
            .contains("native journal Python is missing")
    );
}

#[test]
fn journal_identity_keeps_deferred_local_tokens_unavailable_without_spawning() {
    let temp = TempDir::new("journal-known-tokens");
    let (path, sentinel) = poison_path(&temp);
    let fixture = local_ops_fixture();
    let local_paths = local_op_paths(&fixture);

    for token in ["archive merge", "facet merge"] {
        assert!(
            local_paths.iter().any(|path| path == token),
            "deferred path must remain in the frozen local-operation census: {token}"
        );
        let parts = token.split_once(' ').expect("unavailable path has a group");
        let output = run_journal(&[parts.0, parts.1], Some(&path));
        assert_eq!(output.status.code(), Some(69), "{token}");
        assert_eq!(output.stdout, b"", "{token}");
        let stderr = String::from_utf8(output.stderr).expect("stderr should be utf-8");
        assert!(stderr.contains(token), "{token}: {stderr}");
        assert!(
            stderr.contains("journal_command_unavailable"),
            "{token}: {stderr}"
        );
    }
    assert_sentinel_untouched(&sentinel);
}

#[test]
fn journal_local_operations_fixture_is_the_boundary_census() {
    assert_eq!(
        format!("{:x}", Sha256::digest(LOCAL_OPS_JSON.as_bytes())),
        LOCAL_OPS_SHA256,
        "journal local behavior corpus changed without an independent contract review"
    );
    let fixture = local_ops_fixture();
    assert_eq!(fixture["schema"], "journal-local-ops-v1");
    assert_eq!(fixture["source"], "P-CLI");
    assert_eq!(fixture["exit_codes"]["success"], 0);
    assert_eq!(fixture["exit_codes"]["operation_failed"], 1);
    assert_eq!(fixture["exit_codes"]["usage"], 64);
    assert_eq!(fixture["exit_codes"]["unsafe_or_malformed_input"], 65);
    assert_eq!(fixture["exit_codes"]["io_or_lock_failure"], 74);
    assert_eq!(
        fixture["shared_invariants"]
            .as_array()
            .expect("shared_invariants must be an array")
            .len(),
        8
    );
    let local_paths = local_op_paths(&fixture);
    assert_eq!(local_paths.len(), 5);
    let unique = local_paths
        .iter()
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        unique.len(),
        local_paths.len(),
        "local operation paths repeat"
    );

    for command in fixture["commands"]
        .as_array()
        .expect("commands must be an array")
    {
        assert_eq!(command["owner"], "journal-direct");
        assert!(
            command["grammar"]
                .as_str()
                .is_some_and(|grammar| grammar.starts_with("journal "))
        );
        assert!(
            command["cases"]
                .as_array()
                .is_some_and(|cases| cases.len() >= 2),
            "each local operation needs executable success/failure cases"
        );
        assert!(
            command["capabilities"]
                .as_array()
                .is_some_and(|capabilities| !capabilities.is_empty()),
            "each local operation needs pinned capabilities"
        );
        assert!(
            command["retired_spellings"]
                .as_array()
                .is_some_and(|spellings| !spellings.is_empty()),
            "each intentional break needs an explicit retired spelling"
        );
        for case in command["cases"].as_array().expect("cases must be an array") {
            assert!(case["name"].is_string(), "case name must be a string");
            assert!(case["argv"].is_array(), "case argv must be an array");
            assert!(case["stdin"].is_string(), "case stdin must be a string");
            assert!(case["fixture"].is_string(), "case fixture must be a string");
            assert!(
                case["expected_exit"].is_number() || case["expected_recovery_exit"].is_number(),
                "case must pin a direct or recovery exit"
            );
        }
    }

    let boundary: Value =
        serde_json::from_str(CLI_BOUNDARY_JSON).expect("parse CLI boundary fixture");
    let planned = boundary["identities"]["journal"]["planned_local_paths"]
        .as_array()
        .expect("planned_local_paths must be an array")
        .iter()
        .map(|path| path.as_str().expect("planned local path must be a string"))
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        planned,
        local_paths.iter().map(String::as_str).collect(),
        "journal local behavior corpus drifted from the identity boundary"
    );

    let retired = boundary["identities"]["sol"]["retired_invocations"]
        .as_array()
        .expect("retired_invocations must be an array")
        .iter()
        .map(|path| path.as_str().expect("retired invocation must be a string"))
        .collect::<Vec<_>>();
    for path in [
        "archive export",
        "archive merge",
        "facet doctor",
        "facet merge",
        "news write",
    ] {
        assert!(
            fixture["commands"]
                .as_array()
                .expect("commands must be an array")
                .iter()
                .any(|command| command["path"]
                    .as_array()
                    .expect("command path must be an array")
                    .iter()
                    .map(|token| token.as_str().expect("path token must be a string"))
                    .collect::<Vec<_>>()
                    .join(" ")
                    == path),
            "missing journal-owned path {path}"
        );
    }
    assert!(retired.contains(&"call journal export"));
    assert!(retired.contains(&"call journal facet doctor"));
    assert!(retired.contains(&"call journal facet merge"));
    assert!(retired.contains(&"call journal merge"));
    assert!(retired.contains(&"call journal news --write"));

    let archive_export = fixture["commands"]
        .as_array()
        .expect("commands must be an array")
        .iter()
        .find(|command| command["path"] == serde_json::json!(["archive", "export"]))
        .expect("archive export must be in the local-operation census");
    assert_eq!(
        archive_export["retired_spellings"],
        serde_json::json!(["sol call journal export"]),
        "the retired archive reach must not retire the root observation service"
    );
    assert!(
        boundary["identities"]["journal"]["service_commands"]
            .as_array()
            .expect("journal service_commands must be an array")
            .iter()
            .any(|command| command == "export"),
        "root journal export remains the observation-service process"
    );
}

#[test]
fn journal_identity_runs_path_status_and_root_without_spawning() {
    let temp = TempDir::new("journal-host-primitives");
    let (path, sentinel) = poison_path(&temp);
    let journal = temp.path.join("journal");

    for token in ["--path", "path"] {
        let output = run_journal_with_journal(&[token], Some(&path), &journal);
        assert_eq!(output.status.code(), Some(0), "{token}");
        assert_eq!(
            String::from_utf8(output.stdout).expect("stdout should be utf-8"),
            format!("{}\n", journal.display()),
            "{token}"
        );
        assert_eq!(output.stderr, b"", "{token}");
        assert!(!journal.exists(), "{token} must not create the journal");
    }

    let status = run_journal_with_journal(&["status"], Some(&path), &journal);
    assert_eq!(status.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(status.stdout).expect("stdout should be utf-8"),
        format!(
            "Journal: {}\nSource: env\nExists: no\nDays: 0\n",
            journal.display()
        )
    );
    assert_eq!(status.stderr, b"");

    let root = run_journal_with_journal(&["root"], Some(&path), &journal);
    assert_eq!(root.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(root.stdout).expect("stdout should be utf-8"),
        format!("{}\n", repo_root().display())
    );
    assert_eq!(root.stderr, b"");
    assert_sentinel_untouched(&sentinel);
}

#[test]
fn journal_identity_notify_reuses_the_native_handler_and_socket_protocol() {
    let temp = TempDir::new("journal-notify");
    let (path, sentinel) = poison_path(&temp);
    let journal = temp.path.join("journal");

    let missing_socket = run_journal_with_journal(&["notify", "hello"], Some(&path), &journal);
    assert_eq!(missing_socket.status.code(), Some(1));
    assert_eq!(missing_socket.stdout, b"");
    assert_eq!(
        String::from_utf8(missing_socket.stderr).expect("missing socket stderr should be utf-8"),
        "Failed to send notification (is callosum running?)\n"
    );

    for args in [
        vec!["notify", "--auto-dismiss", "nope", "hello"],
        vec!["notify"],
    ] {
        let output = run_journal_with_journal(&args, Some(&path), &journal);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert_eq!(output.stdout, b"", "{args:?}");
        assert!(
            String::from_utf8(output.stderr)
                .expect("notify argparse stderr should be utf-8")
                .contains("sol notify: error:"),
            "{args:?}"
        );
    }

    let socket = journal.join("health/callosum.sock");
    fs::create_dir_all(socket.parent().expect("socket parent")).expect("create health directory");
    let listener = UnixListener::bind(&socket).expect("bind callosum socket");
    let received = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept notification connection");
        let mut line = String::new();
        stream
            .read_to_string(&mut line)
            .expect("read notification line");
        line
    });
    let output = run_journal_with_journal(
        &[
            "notify",
            "--title",
            "Test",
            "--icon",
            "triangle-alert",
            "--event",
            "custom",
            "--action",
            "/open",
            "--facet",
            "work",
            "--app",
            "alerts",
            "--badge",
            "7",
            "--auto-dismiss",
            "3000",
            "--no-dismiss",
            "-v",
            "-d",
            "hello",
            "world",
        ],
        Some(&path),
        &journal,
    );
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stdout, b"");
    assert_eq!(output.stderr, b"Notification sent\n");
    assert_eq!(
        received
            .join()
            .expect("notification listener should finish"),
        "{\"tract\": \"notification\", \"event\": \"custom\", \"message\": \"hello world\", \"title\": \"Test\", \"icon\": \"triangle-alert\", \"action\": \"/open\", \"facet\": \"work\", \"app\": \"alerts\", \"badge\": \"7\", \"autoDismiss\": 3000, \"dismissible\": false}\n"
    );
    assert_sentinel_untouched(&sentinel);
}

#[test]
fn journal_identity_exec_replaces_itself_and_forwards_process_argv() {
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;

    let temp = TempDir::new("journal-installed-exec");
    let layout = installed_layout(&temp);
    let record = temp.path.join("argv.nul");
    let verbose_record = temp.path.join("verbose.txt");
    write_recording_interpreter(&layout.bin.join("python3"));
    fs::write(layout.bin.join("python"), "#!/bin/sh\nexit 98\n")
        .expect("write fallback interpreter");
    fs::set_permissions(layout.bin.join("python"), fs::Permissions::from_mode(0o755))
        .expect("make fallback interpreter executable");

    let root = installed_output(&layout, &["root"]);
    assert_eq!(root.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(root.stdout).expect("root stdout should be utf-8"),
        format!("{}\n", layout.site_packages.display())
    );
    assert_eq!(root.stderr, b"");

    let owner = [
        "has space",
        "héllo",
        "--help",
        "-v",
        "--verbose",
        "-V",
        "a.b.c",
    ];
    let mut command = Command::new(&layout.binary);
    command
        .arg("-v")
        .arg("up")
        .args(owner)
        .env("RECORD_FILE", &record)
        .env("VERBOSE_RECORD_FILE", &verbose_record);
    let mut child = command.spawn().expect("installed journal should start");
    let pid = child.id();
    let recorded = wait_for_record(&record);
    let expected = [
        vec![
            b"-c".to_vec(),
            solstone_core_journal_cli::python_bootstrap_script()
                .as_bytes()
                .to_vec(),
            b"solstone.think.service".to_vec(),
            b"journal up".to_vec(),
            b"1".to_vec(),
            b"up".to_vec(),
        ],
        owner
            .iter()
            .map(|argument| argument.as_bytes().to_vec())
            .collect(),
    ]
    .concat();
    assert_eq!(recorded, expected);
    assert_eq!(
        fs::read_to_string(&verbose_record).expect("read verbose record"),
        "1"
    );

    kill(Pid::from_raw(pid as i32), Signal::SIGTERM).expect("terminate replaced process");
    let status = child.wait().expect("wait for replaced process");
    assert_eq!(status.signal(), Some(Signal::SIGTERM as i32));
    #[cfg(target_os = "linux")]
    assert!(
        !Path::new(&format!("/proc/{pid}")).exists(),
        "exec replacement must not leave the launched process alive"
    );
}

#[test]
fn journal_identity_requires_an_executable_sibling_interpreter() {
    let temp = TempDir::new("journal-missing-python");
    let layout = installed_layout(&temp);

    let missing = installed_output(&layout, &["think"]);
    assert_eq!(missing.status.code(), Some(69));
    assert_eq!(missing.stdout, b"");
    assert!(
        String::from_utf8(missing.stderr)
            .expect("missing interpreter stderr should be utf-8")
            .contains("native journal Python is missing")
    );

    fs::write(layout.bin.join("python3"), "not executable")
        .expect("write non-executable interpreter");
    fs::set_permissions(
        layout.bin.join("python3"),
        fs::Permissions::from_mode(0o644),
    )
    .expect("make interpreter non-executable");
    let non_executable = installed_output(&layout, &["think"]);
    assert_eq!(non_executable.status.code(), Some(69));
    assert_eq!(non_executable.stdout, b"");
    assert!(
        String::from_utf8(non_executable.stderr)
            .expect("non-executable interpreter stderr should be utf-8")
            .contains("native journal Python is not executable")
    );
}

#[test]
fn journal_identity_coherence_mismatch_blocks_the_interpreter() {
    let temp = TempDir::new("journal-coherence-mismatch");
    let layout = installed_layout(&temp);
    let sentinel = temp.path.join("interpreter.log");
    write_forbidden_shim(&layout.bin.join("python3"), &sentinel);
    fs::write(
        layout
            .site_packages
            .join("solstone_journal-1.2.3.dist-info/METADATA"),
        "Name: solstone-journal\nVersion: 1.2.2\n\n",
    )
    .expect("write mismatched metadata");

    let output = installed_output(&layout, &["think"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(output.stdout, b"");
    assert!(
        String::from_utf8(output.stderr)
            .expect("mismatch stderr should be utf-8")
            .contains("Journal package versions are out of sync.")
    );
    assert_sentinel_untouched(&sentinel);
}

#[test]
fn journal_identity_universal_command_bypasses_coherence_mismatch() {
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;

    let temp = TempDir::new("journal-universal-coherence");
    let layout = installed_layout(&temp);
    let record = temp.path.join("argv.nul");
    write_recording_interpreter(&layout.bin.join("python3"));
    fs::write(
        layout
            .site_packages
            .join("solstone_journal-1.2.3.dist-info/METADATA"),
        "Name: solstone-journal\nVersion: 1.2.2\n\n",
    )
    .expect("write mismatched metadata");

    let mut command = Command::new(&layout.binary);
    command.arg("doctor").env("RECORD_FILE", &record);
    let mut child = command.spawn().expect("universal command should start");
    let pid = child.id();
    let recorded = wait_for_record(&record);
    assert_eq!(
        recorded,
        vec![
            b"-c".to_vec(),
            solstone_core_journal_cli::python_bootstrap_script()
                .as_bytes()
                .to_vec(),
            b"solstone.think.doctor".to_vec(),
            b"journal doctor".to_vec(),
            b"0".to_vec(),
        ]
    );
    kill(Pid::from_raw(pid as i32), Signal::SIGTERM).expect("terminate replaced process");
    assert_eq!(
        child.wait().expect("wait for replaced process").signal(),
        Some(Signal::SIGTERM as i32)
    );
}

#[test]
fn journal_status_counts_days_and_rejects_a_file_root() {
    let temp = TempDir::new("journal-status");
    let journal = temp.path.join("journal");
    fs::create_dir_all(journal.join("chronicle/20260807")).expect("create day");
    fs::create_dir(journal.join("chronicle/2026080x")).expect("create lookalike");
    fs::write(journal.join("chronicle/20260808"), "not a directory").expect("write file");

    let status = run_journal_with_journal(&["status"], None, &journal);
    assert_eq!(status.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(status.stdout).expect("stdout should be utf-8"),
        format!(
            "Journal: {}\nSource: env\nExists: yes\nDays: 1\n",
            journal.display()
        )
    );
    assert_eq!(status.stderr, b"");

    let file_root = temp.path.join("journal-file");
    fs::write(&file_root, "not a directory").expect("write journal file");
    let rejected = run_journal_with_journal(&["status"], None, &file_root);
    assert_eq!(rejected.status.code(), Some(74));
    assert_eq!(rejected.stdout, b"");
    assert!(
        String::from_utf8(rejected.stderr)
            .expect("stderr should be utf-8")
            .contains("not a directory")
    );
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
fn journal_binary_rejects_internal_core_and_legacy_identity_tokens() {
    for args in [
        vec!["__solstone_identity=journal".to_owned()],
        vec!["journal-path".to_owned()],
        vec!["local".to_owned(), "plan".to_owned()],
    ] {
        let output = output_with_retry(&args, None);
        assert_eq!(output.status.code(), Some(64), "{args:?}");
        assert_eq!(output.stdout, b"", "{args:?}");
        assert_eq!(
            String::from_utf8(output.stderr).expect("stderr should be utf-8"),
            solstone_core_journal_cli::JOURNAL_USAGE,
            "{args:?}"
        );
    }
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
fn cargo_metadata_confirms_distinct_public_identity_binaries() {
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

    let sol = binaries
        .iter()
        .filter(|(_package, target)| *target == "solstone-core-sol")
        .collect::<Vec<_>>();
    assert_eq!(sol, vec![&("solstone-core-sol-bin", "solstone-core-sol")]);

    let journal = binaries
        .iter()
        .filter(|(_package, target)| *target == "solstone-core-journal")
        .collect::<Vec<_>>();
    assert_eq!(
        journal,
        vec![&("solstone-core-journal-bin", "solstone-core-journal")]
    );
}
