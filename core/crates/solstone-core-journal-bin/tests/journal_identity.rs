// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::env;
use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread::{self, sleep};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use sha2::{Digest, Sha256};

const LOCAL_OPS_JSON: &str = include_str!("../../../fixtures/journal-cli/local-ops-v1.json");
const LOCAL_OPS_SHA256: &str = "8f6d2ddaf44118f1b591b8dca817fa4ee3b1e806f04fee3124cb4820b1dd7180";
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

fn run_journal_with_input(args: &[&str], path: &Path, journal: &Path, input: &[u8]) -> Output {
    let mut child = Command::new(bin())
        .args(args)
        .env("SOLSTONE_JOURNAL", journal)
        .env("PATH", path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn journal binary");
    child
        .stdin
        .take()
        .expect("child stdin")
        .write_all(input)
        .expect("write child stdin");
    child.wait_with_output().expect("wait for journal binary")
}

fn seed_journal(root: &Path) {
    for directory in ["chronicle", "entities", "facets", "imports"] {
        fs::create_dir_all(root.join(directory)).expect("seed journal directory");
    }
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
    for name in ["python", "python3", "pytest", "uv", "ruff"] {
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
fn journal_indexer_writes_real_index_with_interpreters_poisoned() {
    let temp = TempDir::new("journal-indexer-native-poison");
    let journal = temp.path.join("journal");
    seed_journal(&journal);
    let talent = journal.join("chronicle/20260809/default/120000_1/talents/native.md");
    fs::create_dir_all(talent.parent().expect("talent parent")).expect("create talent directory");
    fs::write(&talent, "# Native index\n\ninterpreter poison needle\n").expect("write index input");
    let (path, sentinel) = poison_path(&temp);

    let output = run_journal_with_journal(&["indexer", "--rescan-full"], Some(&path), &journal);

    assert!(
        output.status.success(),
        "native journal indexer failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert_sentinel_untouched(&sentinel);
    let conn = rusqlite::Connection::open(journal.join("indexer/journal.sqlite"))
        .expect("open written index");
    let indexed: i64 = conn
        .query_row(
            "SELECT count(*) FROM chunks WHERE chunks MATCH 'interpreter' AND content LIKE '%poison needle%'",
            [],
            |row| row.get(0),
        )
        .expect("query written index");
    assert_eq!(indexed, 1, "native writer did not index the real file");

    let query = run_journal_with_journal(&["indexer", "-q", "interpreter"], Some(&path), &journal);
    assert!(
        query.status.success(),
        "native journal query failed: stdout={} stderr={}",
        String::from_utf8_lossy(&query.stdout),
        String::from_utf8_lossy(&query.stderr),
    );
    assert_sentinel_untouched(&sentinel);
    let stdout = String::from_utf8_lossy(&query.stdout);
    assert!(
        stdout.contains("Total: 1 chunks"),
        "unexpected query output: {stdout}"
    );
    assert!(
        stdout.contains("poison needle"),
        "query did not return indexed content: {stdout}"
    );
}

struct InstalledLayout {
    binary: PathBuf,
    bin: PathBuf,
}

fn installed_layout(temp: &TempDir) -> InstalledLayout {
    let prefix = temp.path.join("prefix");
    let bin_dir = prefix.join("bin");
    fs::create_dir_all(&bin_dir).expect("create installed binary directory");
    let binary = bin_dir.join("solstone-core-journal");
    fs::copy(bin(), &binary).expect("copy solstone-core-journal binary");
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o755))
        .expect("make copied binary executable");
    InstalledLayout {
        binary,
        bin: bin_dir,
    }
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
}

#[test]
fn journal_identity_runs_every_local_leaf_natively_without_spawning() {
    let temp = TempDir::new("journal-known-tokens");
    let (path, sentinel) = poison_path(&temp);
    let fixture = local_ops_fixture();
    let local_paths = local_op_paths(&fixture);

    for token in [
        "archive export",
        "archive merge",
        "facet doctor",
        "facet merge",
        "news write",
    ] {
        assert!(
            local_paths.iter().any(|path| path == token),
            "local path must remain in the frozen local-operation census: {token}"
        );
        let parts = token.split_once(' ').expect("unavailable path has a group");
        let output = run_journal(&[parts.0, parts.1, "--help"], Some(&path));
        assert_eq!(output.status.code(), Some(0), "{token}");
        assert!(
            String::from_utf8(output.stdout)
                .expect("stdout should be utf-8")
                .starts_with("Usage: journal "),
            "{token}"
        );
        assert_eq!(output.stderr, b"", "{token}");
    }
    assert_sentinel_untouched(&sentinel);
}

#[test]
fn journal_identity_executes_all_local_authorities_in_the_real_binary() {
    let temp = TempDir::new("journal-local-authorities");
    let (path, sentinel) = poison_path(&temp);
    let journal = temp.path.join("journal");
    seed_journal(&journal);

    let inside_exported_tree = journal.join("chronicle/archive.zip");
    let rejected_export = run_journal_with_journal(
        &[
            "archive",
            "export",
            "--out",
            inside_exported_tree.to_str().unwrap(),
        ],
        Some(&path),
        &journal,
    );
    assert_eq!(rejected_export.status.code(), Some(65));
    assert!(!inside_exported_tree.exists());

    let export = temp.path.join("journal.zip");
    let export_output = run_journal_with_journal(
        &["archive", "export", "--out", export.to_str().unwrap()],
        Some(&path),
        &journal,
    );
    assert_eq!(export_output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(export_output.stdout).expect("export stdout"),
        format!("{}\n", export.display())
    );
    assert_eq!(export_output.stderr, b"");
    assert_eq!(&fs::read(&export).expect("read archive")[..2], b"PK");

    let source = temp.path.join("source");
    seed_journal(&source);
    fs::create_dir_all(source.join("chronicle/20260807/120000_60")).expect("seed merge day");
    fs::write(source.join("chronicle/20260807/120000_60/value"), b"day\n")
        .expect("seed merge item");
    let source_zip = temp.path.join("source.zip");
    let export_source = run_journal_with_journal(
        &["archive", "export", "--out", source_zip.to_str().unwrap()],
        Some(&path),
        &source,
    );
    assert_eq!(export_source.status.code(), Some(0));
    let dry_run = run_journal_with_journal(
        &[
            "archive",
            "merge",
            source_zip.to_str().unwrap(),
            "--dry-run",
            "--json",
        ],
        Some(&path),
        &journal,
    );
    assert_eq!(dry_run.status.code(), Some(0));
    let dry_run_json: Value = serde_json::from_slice(&dry_run.stdout).expect("dry-run JSON");
    assert_eq!(dry_run_json["ok"], true);
    assert_eq!(dry_run_json["dry_run"], true);
    assert_eq!(dry_run_json["index_rebuild"], "not-requested");
    assert!(!journal.with_extension("merge").exists());

    let orphan = journal.join("facets/field-notes");
    fs::create_dir_all(orphan.join("news")).expect("seed orphan facet");
    fs::write(orphan.join("news/20260807.md"), b"existing\n").expect("seed orphan content");
    let doctor = run_journal_with_journal(&["facet", "doctor"], Some(&path), &journal);
    assert_eq!(doctor.status.code(), Some(0));
    assert_eq!(
        doctor.stdout,
        b"Orphan facets:\n- field-notes\n1 orphan facet(s) found. Run with --fix to register them.\n"
    );
    assert!(!orphan.join("facet.json").exists());

    fs::write(orphan.join("facet.json"), b"{\"title\":\"Field Notes\"}\n")
        .expect("declare news facet");
    let markdown = b"# Today\n\nKept byte-for-byte.\n";
    let news = run_journal_with_input(
        &["news", "write", "field-notes", "--day", "20260808"],
        &path,
        &journal,
        markdown,
    );
    assert_eq!(news.status.code(), Some(0));
    assert_eq!(news.stdout, b"News for 20260808 saved to field-notes.\n");
    assert_eq!(
        fs::read(orphan.join("news/20260808.md")).expect("read written news"),
        markdown
    );

    for (slug, title) in [("source", "Source"), ("dest", "Destination")] {
        let facet = journal.join("facets").join(slug);
        fs::create_dir_all(facet.join("news")).expect("seed merge facet");
        fs::write(
            facet.join("facet.json"),
            format!("{{\"title\":\"{title}\"}}\n"),
        )
        .expect("write facet declaration");
    }
    fs::write(
        journal.join("facets/source/news/20260807.md"),
        b"source-only\n",
    )
    .expect("seed source facet content");
    fs::create_dir_all(journal.join("config")).expect("seed journal config directory");
    fs::write(
        journal.join("config/convey.json"),
        b"{\"facets\":{\"selected\":\"source\",\"order\":[\"dest\",\"source\"]}}\n",
    )
    .expect("seed convey config");
    let merge = run_journal_with_journal(
        &["facet", "merge", "source", "--into", "dest", "--consent"],
        Some(&path),
        &journal,
    );
    assert_eq!(
        merge.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&merge.stderr)
    );
    assert!(!journal.join("facets/source").exists());
    assert_eq!(
        fs::read(journal.join("facets/dest/news/20260807.md")).unwrap(),
        b"source-only\n"
    );
    assert_eq!(
        fs::read(journal.join("config/convey.json")).unwrap(),
        b"{\"facets\":{\"selected\":\"source\",\"order\":[\"dest\",\"source\"]}}\n"
    );
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

    let retired = boundary["identities"]["solstone"]["retired_invocations"]
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
        "the retired sol-call spelling is retained for archive export's migration guidance"
    );
    assert!(
        boundary["identities"]["journal"]["service_commands"]
            .as_array()
            .expect("journal service_commands must be an array")
            .iter()
            .all(|command| command != "export"),
        "root journal export must not remain a service-process command"
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

    // `journal root` prints the installation root, which in a checkout is the
    // payload root and not the repository. This is the CLI-observable form of
    // the resolver guarantee: whatever this prints, `solstone/talent/...` joins
    // onto it and resolves, in all three layouts.
    let root = run_journal_with_journal(&["root"], Some(&path), &journal);
    assert_eq!(root.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(root.stdout).expect("stdout should be utf-8"),
        format!(
            "{}\n",
            repo_root()
                .join(solstone_core_journal::CHECKOUT_PAYLOAD_ROOT)
                .display()
        )
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
                .contains("journal notify: error:"),
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
        "{\"tract\": \"notification\", \"event\": \"custom\", \"message\": \"hello world\", \"title\": \"Test\", \"icon\": \"triangle-alert\", \"action\": \"/open\", \"app\": \"alerts\", \"badge\": \"7\", \"autoDismiss\": 3000, \"dismissible\": false}\n"
    );
    assert_sentinel_untouched(&sentinel);
}

#[test]
fn journal_identity_requires_an_executable_sibling_for_native_think() {
    let temp = TempDir::new("journal-missing-native-think");
    let layout = installed_layout(&temp);

    let missing = installed_output(&layout, &["think"]);
    assert_eq!(missing.status.code(), Some(70));
    assert_eq!(missing.stdout, b"");
    assert!(
        String::from_utf8(missing.stderr)
            .expect("missing native sibling stderr should be utf-8")
            .contains("native-helper-missing:")
    );

    fs::write(layout.bin.join("solstone-core"), "not executable")
        .expect("write non-executable native sibling");
    fs::set_permissions(
        layout.bin.join("solstone-core"),
        fs::Permissions::from_mode(0o644),
    )
    .expect("make native sibling non-executable");
    let non_executable = installed_output(&layout, &["think"]);
    assert_eq!(non_executable.status.code(), Some(70));
    assert_eq!(non_executable.stdout, b"");
    assert!(
        String::from_utf8(non_executable.stderr)
            .expect("non-executable native sibling stderr should be utf-8")
            .contains("native-helper-not-executable:")
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

fn seed_four_family_journal(root: &Path) {
    seed_journal(root);
    fs::create_dir_all(root.join("chronicle/20260807/120000_60")).expect("seed chronicle");
    fs::write(
        root.join("chronicle/20260807/120000_60/value"),
        b"day-bytes",
    )
    .expect("seed day");
    fs::create_dir_all(root.join("entities/person")).expect("seed entity dir");
    fs::write(
        root.join("entities/person/entity.json"),
        br#"{"id":"person","name":"Person","type":"Person"}"#,
    )
    .expect("seed entity");
    fs::create_dir_all(root.join("facets/work")).expect("seed facet");
    fs::write(root.join("facets/work/facet.json"), br#"{"title":"Work"}"#)
        .expect("seed facet json");
    fs::create_dir_all(root.join("imports/imp1")).expect("seed import");
    fs::write(root.join("imports/imp1/file"), b"import-bytes").expect("seed import file");
}

#[test]
fn archive_merge_zip_round_trips_all_four_families_and_reindexes() {
    let temp = TempDir::new("archive-merge-ac8");
    let (path, sentinel) = poison_path(&temp);
    let source = temp.path.join("journal-a");
    let target = temp.path.join("journal-b");
    seed_four_family_journal(&source);
    seed_journal(&target);
    let zip = temp.path.join("portable.zip");
    let export = run_journal_with_journal(
        &["archive", "export", "--out", zip.to_str().unwrap()],
        Some(&path),
        &source,
    );
    assert_eq!(
        export.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&export.stderr)
    );
    let nested = target.join("imports/20260311/file.zip");
    fs::create_dir_all(nested.parent().expect("nested parent")).expect("imports ts dir");
    fs::copy(&zip, &nested).expect("copy zip into target");
    let merge = run_journal_with_journal(
        &["archive", "merge", nested.to_str().unwrap(), "--json"],
        Some(&path),
        &target,
    );
    assert_eq!(
        merge.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&merge.stderr)
    );
    let payload: Value = serde_json::from_slice(&merge.stdout).expect("merge json");
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["code"], "ok");
    assert_eq!(payload["index_rebuild"], "completed");
    assert_eq!(
        fs::read(target.join("chronicle/20260807/120000_60/value")).expect("copied day"),
        b"day-bytes"
    );
    assert!(target.join("entities/person/entity.json").is_file());
    assert!(target.join("facets/work/facet.json").is_file());
    assert_eq!(
        fs::read(target.join("imports/imp1/file")).expect("copied import"),
        b"import-bytes"
    );
    let conn = rusqlite::Connection::open(target.join("indexer/journal.sqlite"))
        .expect("open written index");
    let (state, files_count, chunks_count): (String, i64, i64) = conn
        .query_row(
            "SELECT CAST(state AS TEXT), CAST(files_count AS INTEGER), CAST(chunks_count AS INTEGER) FROM index_build_state WHERE id=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("read index build state");
    assert_eq!(state, "complete");
    let live_files: i64 = conn
        .query_row("SELECT count(*) FROM files", [], |row| row.get(0))
        .expect("count files");
    let live_chunks: i64 = conn
        .query_row("SELECT count(*) FROM chunks", [], |row| row.get(0))
        .expect("count chunks");
    assert_eq!(files_count, live_files);
    assert_eq!(chunks_count, live_chunks);
    assert_sentinel_untouched(&sentinel);
}

#[test]
fn archive_merge_wrapper_folder_still_merges_and_skips_extra_trees() {
    let temp = TempDir::new("archive-merge-ac10");
    let (path, sentinel) = poison_path(&temp);
    let target = temp.path.join("journal-b");
    seed_journal(&target);
    let zip = temp.path.join("wrapped.zip");
    let file = fs::File::create(&zip).expect("create wrapped zip");
    let mut writer = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default();
    for (name, bytes) in [
        (
            "journal/chronicle/20260811/120000_60/value",
            b"wrapped-day".as_slice(),
        ),
        (
            "journal/config/journal.json",
            br#"{"secret":true}"#.as_slice(),
        ),
        ("journal/indexer/journal.sqlite", b"not-an-index".as_slice()),
        ("journal/apps/observer/x.json", b"{}".as_slice()),
    ] {
        writer.start_file(name, options).expect("zip member");
        writer.write_all(bytes).expect("zip bytes");
    }
    writer.finish().expect("finish zip");
    let merge = run_journal_with_journal(
        &["archive", "merge", zip.to_str().unwrap(), "--json"],
        Some(&path),
        &target,
    );
    assert_eq!(
        merge.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&merge.stderr)
    );
    let payload: Value = serde_json::from_slice(&merge.stdout).expect("merge json");
    assert_eq!(
        fs::read(target.join("chronicle/20260811/120000_60/value")).expect("copied wrapped day"),
        b"wrapped-day"
    );
    assert!(!target.join("config").exists());
    assert!(!target.join("apps").exists());
    let log = fs::read_to_string(payload["decision_log"].as_str().expect("decision log path"))
        .expect("read decision log");
    assert!(log.contains("\"state\":\"skipped\""));
    assert!(log.contains("config") && log.contains("indexer") && log.contains("apps"));
    assert_sentinel_untouched(&sentinel);
}

#[test]
fn archive_merge_directory_source_is_unsafe() {
    let temp = TempDir::new("archive-merge-directory");
    let (path, sentinel) = poison_path(&temp);
    let journal = temp.path.join("journal");
    let source = temp.path.join("source-dir");
    seed_journal(&journal);
    seed_journal(&source);
    let merge = run_journal_with_journal(
        &["archive", "merge", source.to_str().unwrap()],
        Some(&path),
        &journal,
    );
    assert_eq!(merge.status.code(), Some(65));
    let stderr = String::from_utf8_lossy(&merge.stderr);
    assert!(stderr.contains("SOURCE must be a regular file"), "{stderr}");
    assert_sentinel_untouched(&sentinel);
}
