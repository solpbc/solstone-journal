// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::io;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[allow(dead_code)]
#[path = "../../solstone-core-journal-cli/src/processes.rs"]
mod production_processes;

use production_processes::{NATIVE_PROCESS_SPECS, NativeProcessSpec, PROCESS_SPECS};

const POISON_INTERPRETER: &str = r#"#!/bin/sh
printf '%s:%s\n' "${POISON_ROUTE:-reached}" "${0##*/}" >> "$POISON_MARKER"
exit 97
"#;
const STORAGE_OPS_REFERENCE_GRAMMAR: &str =
    include_str!("../../../fixtures/journal-storage-ops-reference-grammar.txt");
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const KILL_REAP_GRACE: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Copy)]
struct Probe {
    token: &'static str,
    argv: &'static [&'static str],
    expected_exit: i32,
}

const PROBES: &[Probe] = &[
    Probe {
        token: "depict",
        argv: &[],
        expected_exit: 1,
    },
    Probe {
        token: "spl",
        argv: &["--nope"],
        expected_exit: 64,
    },
    Probe {
        token: "schedule",
        argv: &["--nonsense"],
        expected_exit: 2,
    },
    // NO PROBE FOR supervisor OR start, DELIBERATELY, AND THIS RED IS CORRECT.
    //
    // 1d1523b4b added both tokens to NATIVE_PROCESS_SPECS without probes, so
    // this contract is red. Registering them at exit 64 makes it green and
    // makes it check less. Measured in a replica of this harness:
    //
    //   journal grab       --nonsense -> 2,  "usage: journal grab [-h] ..."
    //   journal export     --nonsense -> 2,  "usage: journal export [-h] ..."
    //   journal identity   --nonsense -> 2,  "usage: journal identity [-h] ..."
    //   journal supervisor --nonsense -> 64, top-level "Usage:"
    //   journal start      --nonsense -> 64, top-level "Usage:"
    //   journal spl        --nope     -> 64, top-level "Usage:"
    //   solstone-core bogus-verb --nonsense -> 64, top-level "Usage:"
    //
    // parse_supervisor and parse_spl have no verb-level usage variant, so every
    // bad argv for those verbs falls through to the top-level arm -- the SAME
    // arm that answers a verb the sibling does not have at all. A row expecting
    // 64 therefore certifies the token on a code the binary would still emit
    // with supervisor deleted from the sibling entirely. That is the exact
    // failure class this guard exists to catch, reproduced inside the guard.
    //
    // The fix is to give parse_supervisor and parse_spl a verb-level usage path
    // so a bad argv exits 2, then register against that. It is scoped and
    // scope-checked as Lane Z's Z6-pre, which also covers spl -- whose existing
    // row carries this same defect today. Leave this red for it.
    Probe {
        token: "grab",
        argv: &["--nonsense"],
        expected_exit: 2,
    },
    Probe {
        token: "contract",
        argv: &["--nonsense"],
        expected_exit: 2,
    },
    Probe {
        token: "transfer",
        argv: &["--nonsense"],
        expected_exit: 2,
    },
    Probe {
        token: "export",
        argv: &["--nonsense"],
        expected_exit: 2,
    },
    Probe {
        token: "transcribe",
        argv: &["--nonsense"],
        expected_exit: 2,
    },
    Probe {
        token: "observer",
        argv: &["--nonsense"],
        expected_exit: 2,
    },
    Probe {
        token: "facet-candidates",
        argv: &["--nonsense"],
        expected_exit: 2,
    },
    Probe {
        token: "navigate",
        argv: &["--nonsense"],
        expected_exit: 2,
    },
    Probe {
        token: "identity",
        argv: &["--nonsense"],
        expected_exit: 2,
    },
    Probe {
        token: "settings",
        argv: &["--nonsense"],
        expected_exit: 2,
    },
    Probe {
        token: "streams",
        argv: &["--nonsense"],
        expected_exit: 2,
    },
    Probe {
        token: "segment",
        argv: &["--nonsense"],
        expected_exit: 2,
    },
    Probe {
        token: "journal-stats",
        argv: &["--nonsense"],
        expected_exit: 2,
    },
    Probe {
        token: "reprocess",
        argv: &["--nonsense"],
        expected_exit: 2,
    },
    Probe {
        token: "backfill-processing-records",
        argv: &["--nonsense"],
        expected_exit: 2,
    },
];

#[allow(dead_code)]
#[derive(Debug)]
enum Verdict {
    Pass,
    ProbeUnregistered {
        token: &'static str,
    },
    InterpreterReached {
        token: &'static str,
        exit: Option<i32>,
    },
    WrongExit {
        token: &'static str,
        expected: i32,
        actual: Option<i32>,
    },
    TimedOut {
        token: &'static str,
    },
    LaunchFailure {
        token: &'static str,
        error: String,
    },
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(label: &str) -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be available")
            .as_nanos();
        let path = env::temp_dir().join(format!(
            "solstone-core-journal-native-process-contract-{label}-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create temporary directory");
        Self { path }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// Records that cargo produced this artifact during THIS test run, which is
/// what makes the probe below evidence about the current tree rather than
/// about whatever was last left in `target/`.
struct Artifact {
    #[allow(dead_code)]
    path: PathBuf,
}

struct Harness {
    _temp: TempDir,
    dispatcher: PathBuf,
    sibling_dir: PathBuf,
    dispatcher_artifact: Artifact,
    sibling_artifacts: BTreeMap<&'static str, Artifact>,
    home: PathBuf,
    journal: PathBuf,
    poison_marker: PathBuf,
}

struct VerdictContext<'a> {
    dispatcher: &'a Path,
    sibling_dir: &'a Path,
    #[allow(dead_code)]
    dispatcher_artifact: &'a Artifact,
    sibling_artifacts: &'a BTreeMap<&'static str, Artifact>,
    home: &'a Path,
    journal: &'a Path,
    poison_marker: &'a Path,
}

impl Harness {
    fn new() -> Self {
        let temp = TempDir::new("bin");
        let sibling_dir = temp.path.join("bin");
        fs::create_dir(&sibling_dir).expect("create binary directory");

        let dispatcher_source = PathBuf::from(env!("CARGO_BIN_EXE_solstone-core-journal"));
        let core_source = locate_workspace_binary("solstone-core", "solstone-core");
        let depict_source = locate_workspace_binary("solstone-core-depict", "solstone-core-depict");

        let dispatcher_artifact = artifact(&dispatcher_source);
        let mut sibling_artifacts = BTreeMap::new();
        sibling_artifacts.insert("solstone-core", artifact(&core_source));
        sibling_artifacts.insert("solstone-core-depict", artifact(&depict_source));

        let dispatcher = sibling_dir.join("solstone-core-journal");
        copy_executable(&dispatcher_source, &dispatcher);
        copy_executable(&core_source, &sibling_dir.join("solstone-core"));
        copy_executable(&depict_source, &sibling_dir.join("solstone-core-depict"));
        for interpreter in ["python", "python3", "pytest", "uv", "ruff"] {
            let path = sibling_dir.join(interpreter);
            fs::write(&path, POISON_INTERPRETER).expect("write poison interpreter");
            make_executable(&path);
        }

        let home = temp.path.join("home");
        let journal = temp.path.join("journal");
        let poison_marker = temp.path.join("python-invoked.txt");
        Self {
            _temp: temp,
            dispatcher,
            sibling_dir,
            dispatcher_artifact,
            sibling_artifacts,
            home,
            journal,
            poison_marker,
        }
    }

    fn context(&self) -> VerdictContext<'_> {
        VerdictContext {
            dispatcher: &self.dispatcher,
            sibling_dir: &self.sibling_dir,
            dispatcher_artifact: &self.dispatcher_artifact,
            sibling_artifacts: &self.sibling_artifacts,
            home: &self.home,
            journal: &self.journal,
            poison_marker: &self.poison_marker,
        }
    }
}

fn artifact(path: &Path) -> Artifact {
    Artifact {
        path: path.to_path_buf(),
    }
}

fn copy_executable(source: &Path, destination: &Path) {
    fs::copy(source, destination).expect("copy executable");
    make_executable(destination);
}

fn make_executable(path: &Path) {
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("make fixture executable");
}

fn locate_workspace_binary(package: &str, binary: &str) -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_manifest = manifest_dir
        .parent()
        .expect("crates dir")
        .parent()
        .expect("core dir")
        .join("Cargo.toml");
    let output = Command::new(env!("CARGO"))
        .args(["build", "--manifest-path"])
        .arg(&workspace_manifest)
        .args(["-p", package, "--bin", binary, "--message-format=json"])
        .output()
        .expect("cargo build native sibling should execute");
    assert!(
        output.status.success(),
        "cargo build -p {package} --bin {binary} failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let Ok(message) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if message.get("reason").and_then(|value| value.as_str()) != Some("compiler-artifact") {
            continue;
        }
        let target = &message["target"];
        if target["name"].as_str() != Some(binary) {
            continue;
        }
        let is_bin = target["kind"]
            .as_array()
            .is_some_and(|kinds| kinds.iter().any(|kind| kind.as_str() == Some("bin")));
        if !is_bin {
            continue;
        }
        if let Some(executable) = message.get("executable").and_then(|value| value.as_str()) {
            return PathBuf::from(executable);
        }
    }
    panic!("cargo build did not report a {binary} binary artifact");
}

fn probe_for(token: &str) -> Option<&'static Probe> {
    PROBES.iter().find(|probe| probe.token == token)
}

fn is_executable(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
}

fn wait_for_child(child: &mut Child, deadline: Instant) -> io::Result<Option<ExitStatus>> {
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let reap_deadline = Instant::now() + KILL_REAP_GRACE;
            while Instant::now() < reap_deadline {
                if child.try_wait()?.is_some() {
                    break;
                }
                thread::sleep(Duration::from_millis(10));
            }
            return Ok(None);
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn run_dispatcher(
    context: &VerdictContext<'_>,
    token: &str,
    argv: &[&str],
) -> io::Result<Option<ExitStatus>> {
    let _ = fs::remove_file(context.poison_marker);
    // PATH is pinned to the poisoned sibling directory, and that is an ADDITION
    // to the sibling poison rather than a substitute for it. The two poisons are
    // not interchangeable and must both hold:
    //
    //   * the SIBLING poison is the load-bearing one. It tests the shipped
    //     dispatch path, because `sibling_python_for_executable` resolves
    //     `python3` beside `current_exe()` and never from PATH. A PATH shim
    //     proves nothing about that route.
    //   * PATH was previously INHERITED here, so a native verb that reached an
    //     interpreter the ordinary way -- `Command::new("python3")`, or any
    //     library that shells one -- found the host's real interpreter and this
    //     probe stayed green. The gate proved the dispatch was native and said
    //     nothing about what the native binary then did.
    //
    // `sibling_dir` already holds poison shims named python, python3, pytest, uv
    // and ruff, each of which writes POISON_MARKER and exits 97, so pointing
    // PATH at it closes that second route with the shims already present. The
    // dispatcher finds its sibling binary through current_exe(), not PATH, so
    // narrowing PATH does not affect the route under test.
    let mut child = Command::new(context.dispatcher)
        .arg(token)
        .args(argv)
        .env("POISON_MARKER", context.poison_marker)
        .env("HOME", context.home)
        .env("SOLSTONE_JOURNAL", context.journal)
        .env("PATH", context.sibling_dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    wait_for_child(&mut child, Instant::now() + PROBE_TIMEOUT)
}

fn run_dispatcher_with_output(
    context: &VerdictContext<'_>,
    token: &str,
    argv: &[&str],
) -> io::Result<std::process::Output> {
    Command::new(context.dispatcher)
        .arg(token)
        .args(argv)
        .env("POISON_MARKER", context.poison_marker)
        .env("HOME", context.home)
        .env("SOLSTONE_JOURNAL", context.journal)
        .env("PATH", context.sibling_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
}

fn prove_poison_interpreters_live(context: &VerdictContext<'_>) {
    let _ = fs::remove_file(context.poison_marker);
    for name in ["python", "python3", "pytest", "uv", "ruff"] {
        let sibling = Command::new(context.sibling_dir.join(name))
            .env("POISON_MARKER", context.poison_marker)
            .env("POISON_ROUTE", "sibling")
            .status()
            .expect("execute sibling poison shim");
        assert_eq!(sibling.code(), Some(97), "{name}: sibling poison exit");

        let path = Command::new(name)
            .env("PATH", context.sibling_dir)
            .env("POISON_MARKER", context.poison_marker)
            .env("POISON_ROUTE", "path")
            .status()
            .expect("execute PATH poison shim");
        assert_eq!(path.code(), Some(97), "{name}: PATH poison exit");
    }
    let observed = fs::read_to_string(context.poison_marker).expect("poison liveness record");
    let expected = ["sibling", "path"]
        .into_iter()
        .flat_map(|route| {
            ["python", "python3", "pytest", "uv", "ruff"]
                .into_iter()
                .map(move |name| format!("{route}:{name}"))
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(
        observed.lines().map(str::to_owned).collect::<BTreeSet<_>>(),
        expected
    );
    fs::remove_file(context.poison_marker).expect("clear poison liveness record");
}

fn reference_block(name: &str) -> &str {
    let header = format!("=== {name}\n");
    let start = STORAGE_OPS_REFERENCE_GRAMMAR
        .find(&header)
        .expect("reference grammar block")
        + header.len();
    let rest = &STORAGE_OPS_REFERENCE_GRAMMAR[start..];
    &rest[..rest.find("\n=== ").unwrap_or(rest.len())]
}

fn verdict_for(
    spec: &NativeProcessSpec,
    probe: Option<&Probe>,
    context: &VerdictContext<'_>,
    checked: &mut BTreeSet<&'static str>,
) -> Verdict {
    let Some(probe) = probe else {
        return Verdict::ProbeUnregistered { token: spec.token };
    };
    let sibling = context.sibling_dir.join(spec.binary);
    if !is_executable(&sibling) {
        return Verdict::WrongExit {
            token: spec.token,
            expected: probe.expected_exit,
            actual: Some(69),
        };
    }
    let Some(sibling_artifact) = context.sibling_artifacts.get(spec.binary) else {
        return Verdict::LaunchFailure {
            token: spec.token,
            error: format!("no source artifact recorded for {}", spec.binary),
        };
    };
    // Freshness is guaranteed by CONSTRUCTION, not by comparing mtimes:
    // locate_workspace_binary runs `cargo build -p <pkg> --bin <bin>` for this
    // sibling in this very test run, so cargo has either rebuilt it or proven
    // it up to date.
    //
    // An mtime comparison against the dispatcher looks like a stronger check
    // and is actually a WRONG one: cargo does not touch an artifact it finds
    // up to date, so a perfectly current binary keeps an old mtime. Measured
    // here -- solstone-core-depict sat at 18:06 against a 19:59 dispatcher,
    // `cargo build -p solstone-core-depict` finished in 0.14s with nothing to
    // do, and no source file was newer than the binary. The comparison
    // reported a stale sibling on a binary that was current, which is a false
    // red on the instrument the rest of the lane's cuts are gated by.
    let _ = sibling_artifact;
    let status = match run_dispatcher(context, spec.token, probe.argv) {
        Ok(status) => status,
        Err(error) => {
            return Verdict::LaunchFailure {
                token: spec.token,
                error: error.to_string(),
            };
        }
    };
    checked.insert(spec.token);
    let actual = status.and_then(|status| status.code());
    if context.poison_marker.exists() {
        return Verdict::InterpreterReached {
            token: spec.token,
            exit: actual,
        };
    }
    if status.is_none() {
        return Verdict::TimedOut { token: spec.token };
    }
    if actual != Some(probe.expected_exit) {
        return Verdict::WrongExit {
            token: spec.token,
            expected: probe.expected_exit,
            actual,
        };
    }
    Verdict::Pass
}

#[test]
fn native_process_dispatch_and_poison_liveness_contract() {
    let harness = Harness::new();
    let context = harness.context();
    prove_poison_interpreters_live(&context);
    let mut checked = BTreeSet::new();
    let verdicts = NATIVE_PROCESS_SPECS
        .iter()
        .map(|spec| {
            (
                spec.token,
                verdict_for(spec, probe_for(spec.token), &context, &mut checked),
            )
        })
        .collect::<Vec<_>>();
    let native_tokens = NATIVE_PROCESS_SPECS
        .iter()
        .map(|spec| spec.token)
        .collect::<BTreeSet<_>>();
    let probe_tokens = PROBES
        .iter()
        .map(|probe| probe.token)
        .collect::<BTreeSet<_>>();

    assert_eq!(
        checked, native_tokens,
        "native process checked-set mismatch; verdicts={verdicts:?}"
    );
    assert!(
        verdicts
            .iter()
            .all(|(_, verdict)| matches!(verdict, Verdict::Pass)),
        "native process verdict failure: {verdicts:?}"
    );
    assert_eq!(
        probe_tokens, native_tokens,
        "native process probe-table mismatch; verdicts={verdicts:?}"
    );
    let Some(token) = PROCESS_SPECS
        .iter()
        .find(|spec| {
            !NATIVE_PROCESS_SPECS
                .iter()
                .any(|native| native.token == spec.token)
        })
        .map(|spec| spec.token)
    else {
        eprintln!("skipping poison-liveness assertion: every process token is native");
        return;
    };
    let status = run_dispatcher(&context, token, &[])
        .expect("runtime-derived Python process should spawn through the dispatcher");

    assert_eq!(
        status.and_then(|status| status.code()),
        Some(97),
        "{token}: poison-liveness expected poisoned interpreter exit 97"
    );
    assert!(
        context.poison_marker.exists(),
        "{token}: poison-liveness expected the poisoned interpreter marker"
    );
}

#[test]
fn native_schedule_dispatch_reaches_the_real_read_only_body() {
    let harness = Harness::new();
    let context = harness.context();
    prove_poison_interpreters_live(&context);
    fs::create_dir_all(context.journal.join("config")).expect("schedule config directory");
    fs::create_dir_all(context.journal.join("health")).expect("schedule health directory");
    fs::write(
        context.journal.join("config/journal.json"),
        br#"{"setup":{"completed_at":1}}"#,
    )
    .expect("journal config");
    fs::write(
        context.journal.join("config/schedules.json"),
        br#"{"daily_time":"03:17","alpha:daily":{"cmd":["journal","heartbeat"],"every":"daily"},"beta:minute":{"cmd":"journal think --cadence","every":"1m"},"omega:disabled":{"cmd":["journal","noop"],"every":"hourly","enabled":false}}"#,
    )
    .expect("schedules config");
    fs::write(
        context.journal.join("health/scheduler.json"),
        br#"{"alpha:daily":{"last_run":0},"beta:minute":{"last_run":0}}"#,
    )
    .expect("scheduler state");
    let before = snapshot_tree(context.journal);

    for argv in [Vec::<&str>::new(), vec!["-v", "-d"]] {
        let output = run_dispatcher_with_output(&context, "schedule", &argv)
            .expect("run native schedule through journal dispatcher");
        assert_eq!(output.status.code(), Some(0));
        assert!(output.stderr.is_empty());
        let stdout = String::from_utf8(output.stdout).expect("schedule stdout");
        assert!(stdout.contains("alpha:daily"));
        assert!(stdout.contains("beta:minute"));
        assert!(stdout.contains("5m"));
        assert!(stdout.contains("omega:disabled"));
        assert!(stdout.contains("disabled"));
        assert!(
            stdout.contains(
                &context
                    .journal
                    .join("config/schedules.json")
                    .display()
                    .to_string()
            )
        );
    }
    let leading_verbose = Command::new(context.dispatcher)
        .args(["-v", "schedule"])
        .env("POISON_MARKER", context.poison_marker)
        .env("HOME", context.home)
        .env("SOLSTONE_JOURNAL", context.journal)
        .env("PATH", context.sibling_dir)
        .output()
        .expect("run leading verbose schedule");
    assert_eq!(leading_verbose.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&leading_verbose.stdout).contains("alpha:daily"));

    let invalid = run_dispatcher_with_output(&context, "schedule", &["--nonsense"])
        .expect("run invalid native schedule through journal dispatcher");
    assert_eq!(invalid.status.code(), Some(2));
    assert!(invalid.stdout.is_empty());
    assert_eq!(
        String::from_utf8_lossy(&invalid.stderr),
        "usage: journal schedule [-h] [-v] [-d]\njournal schedule: error: unrecognized arguments: --nonsense\n"
    );
    assert_eq!(snapshot_tree(context.journal), before);
    assert!(!context.poison_marker.exists());
}

#[derive(Debug, PartialEq, Eq)]
struct TreeEntry {
    inode: u64,
    mode: u32,
    kind: &'static str,
    content: Vec<u8>,
}

fn snapshot_tree(root: &Path) -> BTreeMap<PathBuf, TreeEntry> {
    fn walk(root: &Path, path: &Path, entries: &mut BTreeMap<PathBuf, TreeEntry>) {
        let metadata = fs::symlink_metadata(path).expect("snapshot metadata");
        let relative = path.strip_prefix(root).expect("snapshot relative path");
        let (kind, content) = if metadata.file_type().is_symlink() {
            (
                "symlink",
                fs::read_link(path)
                    .expect("snapshot symlink")
                    .as_os_str()
                    .as_encoded_bytes()
                    .to_vec(),
            )
        } else if metadata.is_dir() {
            ("directory", Vec::new())
        } else {
            ("file", fs::read(path).expect("snapshot file"))
        };
        entries.insert(
            relative.to_path_buf(),
            TreeEntry {
                inode: metadata.ino(),
                mode: metadata.permissions().mode(),
                kind,
                content,
            },
        );
        if metadata.is_dir() {
            let mut children = fs::read_dir(path)
                .expect("snapshot directory")
                .map(|entry| entry.expect("snapshot entry").path())
                .collect::<Vec<_>>();
            children.sort();
            for child in children {
                walk(root, &child, entries);
            }
        }
    }

    let mut entries = BTreeMap::new();
    walk(root, root, &mut entries);
    entries
}

#[test]
fn native_storage_ops_help_matches_reference_grammar_through_dispatcher() {
    let harness = Harness::new();
    let context = harness.context();
    for token in [
        "streams",
        "segment",
        "journal-stats",
        "reprocess",
        "backfill-processing-records",
    ] {
        let output = run_dispatcher_with_output(&context, token, &["--help"])
            .expect("run native storage operation help through dispatcher");
        assert_eq!(output.status.code(), Some(0), "{token}");
        assert_eq!(output.stderr, b"", "{token}");
        assert_eq!(
            output.stdout,
            reference_block(&format!("{token} --help")).as_bytes(),
            "{token}"
        );
    }
}

#[test]
fn native_backfill_commit_dispatches_without_python_and_is_idempotent() {
    let harness = Harness::new();
    let context = harness.context();
    let _ = fs::remove_file(context.poison_marker);

    fs::create_dir_all(context.home).expect("create home directory");
    let segment = context.journal.join("chronicle/20990101/090000_300");
    fs::create_dir_all(&segment).expect("create future journal segment");
    fs::write(segment.join("audio.flac"), b"audio").expect("write audio");
    let sidecar = segment.join("audio.jsonl");
    fs::write(&sidecar, b"{\"raw\":\"audio.flac\"}\n").expect("write sidecar");

    let python_status = run_dispatcher(&context, "backup", &[])
        .expect("run retained Python process")
        .and_then(|status| status.code());
    assert_eq!(python_status, Some(97));
    assert!(context.poison_marker.exists());
    fs::remove_file(context.poison_marker).expect("clear Python poison marker");

    let output = run_dispatcher_with_output(&context, "backfill-processing-records", &["--commit"])
        .expect("run native backfill through dispatcher");
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8(output.stdout).expect("backfill stdout is UTF-8");
    let total = stdout
        .lines()
        .find_map(|line| line.strip_prefix("total: "))
        .expect("backfill total")
        .parse::<u64>()
        .expect("backfill total is numeric");
    assert!(total > 0);
    assert!(stdout.contains("stamp_empty: 1\n"));
    let header: serde_json::Value =
        serde_json::from_slice(&fs::read(&sidecar).expect("read stamped sidecar"))
            .expect("stamped sidecar is JSON");
    assert_eq!(header["_solstone_processing"]["source"], "backfill");
    assert!(!context.poison_marker.exists());

    let output = run_dispatcher_with_output(&context, "backfill-processing-records", &["--commit"])
        .expect("rerun native backfill through dispatcher");
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8(output.stdout).expect("backfill stdout is UTF-8");
    assert!(stdout.contains("skip_has_record: 1\n"));
    assert!(stdout.contains("stamp_empty: 0\n"));
}

#[test]
fn missing_native_sibling_is_a_guard_verdict_without_a_spawn() {
    let temp = TempDir::new("missing-sibling");
    let dispatcher = temp.path.join("dispatcher-not-run");
    let empty_artifacts = BTreeMap::new();
    let dispatcher_artifact = Artifact {
        path: dispatcher.clone(),
    };
    let poison_marker = temp.path.join("poison-not-run");
    let home = temp.path.join("home");
    let journal = temp.path.join("journal");
    let context = VerdictContext {
        dispatcher: &dispatcher,
        sibling_dir: &temp.path,
        dispatcher_artifact: &dispatcher_artifact,
        sibling_artifacts: &empty_artifacts,
        home: &home,
        journal: &journal,
        poison_marker: &poison_marker,
    };
    let synthetic = NativeProcessSpec {
        token: "synthetic-missing-native",
        binary: "missing-native-helper",
        preset_argv: &[],
    };
    let probe = Probe {
        token: "synthetic-missing-native",
        argv: &[],
        expected_exit: 0,
    };
    let mut checked = BTreeSet::new();
    let verdict = verdict_for(&synthetic, Some(&probe), &context, &mut checked);

    assert!(
        matches!(
            verdict,
            Verdict::WrongExit {
                token: "synthetic-missing-native",
                expected: 0,
                actual: Some(69),
            }
        ),
        "synthetic-missing-native: expected WrongExit missing-sibling verdict, got {verdict:?}"
    );
    assert!(
        checked.is_empty(),
        "synthetic-missing-native: missing sibling must not spawn a dispatcher"
    );
}

#[test]
fn lane_w_process_tokens_are_native_cutovers() {
    let native_tokens = NATIVE_PROCESS_SPECS
        .iter()
        .map(|spec| spec.token)
        .collect::<BTreeSet<_>>();
    for token in ["grab", "transfer", "export", "observer", "transcribe"] {
        assert!(
            native_tokens.contains(token),
            "{token}: Lane W requires a native process cutover"
        );
    }
}
