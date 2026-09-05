// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[allow(dead_code)]
#[path = "../../solstone-core-journal-cli/src/processes.rs"]
mod production_processes;

use production_processes::{NATIVE_PROCESS_SPECS, NativeProcessSpec, PROCESS_SPECS};
use sha2::{Digest, Sha256};
use solstone_core_cli::{
    CHECK_HELP, CHECK_USAGE, DESCRIBE_USAGE, HEALTH_USAGE, INSTALL_MODELS_HELP,
    INSTALL_MODELS_USAGE, INSTALL_PROVIDER_HELP, INSTALL_PROVIDER_USAGE, MCP_USAGE, SCHEDULE_USAGE,
    SPL_USAGE, THINKING_USAGE, TOP_USAGE,
};

const POISON_INTERPRETER: &str = r#"#!/bin/sh
printf '%s:%s\n' "${POISON_ROUTE:-reached}" "${0##*/}" >> "$POISON_MARKER"
exit 97
"#;
const STORAGE_OPS_REFERENCE_GRAMMAR: &str =
    include_str!("../../../fixtures/journal-storage-ops-reference-grammar.txt");
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
// Maintenance bodies perform real filesystem scans and backup-tool probes.
// Keep their correctness contract bounded without coupling it to the stricter
// parser-probe latency budget used by the rest of this suite.
const MAINTENANCE_PROBE_TIMEOUT: Duration = Duration::from_secs(15);
const CHECK_USAGE_ANCHOR: &[u8] = CHECK_USAGE.as_bytes();
const INSTALL_MODELS_USAGE_ANCHOR: &[u8] = INSTALL_MODELS_USAGE.as_bytes();
const INSTALL_PROVIDER_USAGE_ANCHOR: &[u8] = INSTALL_PROVIDER_USAGE.as_bytes();
const CONVEY_USAGE_ANCHOR: &[u8] = b"usage: journal convey [-h] --port PORT [-v] [-d]\n";
const DESCRIBE_USAGE_ANCHOR: &[u8] = DESCRIBE_USAGE.as_bytes();
const CHECK_JSON_TOP_LEVEL_KEYS: &[&str] =
    &["platform", "checks", "overall", "feedback_url", "version"];
const OWNER_VERB_REQUIRED_NATIVE_TOKENS: &[&str] =
    &["engage", "maintenance", "heartbeat", "backup"];
const REQUIRED_NATIVE_TOKENS: &[&str] = &["brain"];
const THINK_AND_SETUP_REQUIRED_NATIVE_TOKENS: &[&str] = &["think", "setup"];
const TALENT_LIFECYCLE_NATIVE_TOKENS: &[&str] = &["cortex", "talent"];
// Think's token has a dedicated binding because REQUIRED_NATIVE_TOKENS is already
// bound to ["brain"] on purpose, so a second binding is required or this assertion
// would be deleted.
// THINK_AND_SETUP_REQUIRED_NATIVE_TOKENS already covers the setup token set; this
// binding keeps the think assertion explicit.
const THINK_REQUIRED_NATIVE_TOKENS: &[&str] = &["think"];
// `install-provider` is the last owner-facing provider-install verb still
// routed to Python. Its dedicated binding is required because
// REQUIRED_NATIVE_TOKENS is already bound to ["brain"] on this tree, so
// rebinding it would not compile and would delete that assertion. The value is
// the contract; the binding name is not.
const INSTALL_PROVIDER_REQUIRED_NATIVE_TOKENS: &[&str] = &["install-provider"];

fn run_talent_worker_with_output(
    context: &VerdictContext<'_>,
    request: &str,
) -> std::process::Output {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace = manifest_dir
        .parent()
        .expect("crates directory")
        .parent()
        .expect("core directory")
        .parent()
        .expect("workspace directory");
    Command::new(context.sibling_dir.join("solstone-core"))
        .arg("__talent-worker")
        .current_dir(workspace)
        .env("POISON_MARKER", context.poison_marker)
        .env("HOME", context.home)
        .env("SOLSTONE_JOURNAL", context.journal)
        // The sibling directory contains python shims, and PATH is pinned to
        // that same directory: both interpreter-poison routes are active.
        .env("PATH", context.sibling_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child
                .stdin
                .as_mut()
                .expect("talent worker stdin")
                .write_all(request.as_bytes())?;
            child.wait_with_output()
        })
        .expect("run native talent worker")
}

fn prepare_talent_worker_journal(context: &VerdictContext<'_>) {
    fs::create_dir_all(context.journal.join("config")).expect("create talent config directory");
    fs::write(
        context.journal.join("config/journal.json"),
        r#"{"providers":{"active":{"provider":"local","model":"fixture-model"}}}"#,
    )
    .expect("write talent config");
}

#[test]
fn criterion_16_talent_worker_is_closed_at_journal_and_reaches_native_body() {
    let harness = Harness::new();
    let context = harness.context();
    prove_poison_interpreters_live(&context);
    fs::create_dir_all(context.journal).expect("create journal");
    let before = snapshot_tree(context.journal);
    let closed = run_dispatcher_with_output(&context, "__talent-worker", &[])
        .expect("run closed dispatcher spelling");
    assert_eq!(closed.status.code(), Some(64));
    assert!(String::from_utf8_lossy(&closed.stderr).contains("Usage: journal <command> [args...]"));
    assert_eq!(snapshot_tree(context.journal), before);
    assert!(!context.poison_marker.exists());

    prepare_talent_worker_journal(&context);
    let output =
        run_talent_worker_with_output(&context, "{\"name\":\"partner\",\"prompt\":\"hello\"}\n");
    let events = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(serde_json::from_str::<serde_json::Value>)
        .collect::<Result<Vec<_>, _>>()
        .expect("talent worker NDJSON");
    assert!(
        events
            .iter()
            .any(|event| event["event"] == "start" && event["name"] == "partner")
    );
}

#[test]
fn criterion_17_talent_worker_reaches_start_under_sibling_and_path_poison() {
    let harness = Harness::new();
    let context = harness.context();
    prove_poison_interpreters_live(&context);
    prepare_talent_worker_journal(&context);
    let output =
        run_talent_worker_with_output(&context, "{\"name\":\"partner\",\"prompt\":\"hello\"}\n");
    assert!(output.status.success());
    assert!(!context.poison_marker.exists());
    let events = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(serde_json::from_str::<serde_json::Value>)
        .collect::<Result<Vec<_>, _>>()
        .expect("talent worker NDJSON");
    // This start event is the depth observable: it is emitted after preparation
    // and before generation, so parser-only success cannot satisfy this test.
    assert!(
        events
            .iter()
            .any(|event| event["event"] == "start" && event["name"] == "partner")
    );
}

#[test]
fn criterion_18_cortex_spawns_native_talent_worker_without_interpreter_resolution() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates directory")
        .join("solstone-core-cortex/src/process.rs");
    let body = fs::read_to_string(source).expect("read cortex process source");
    assert!(body.contains("sibling_native_in_dir(executable_dir, \"solstone-core\")"));
    assert!(body.contains("arguments: vec![OsString::from(\"__talent-worker\")]"));

    let crates = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates directory")
        .to_path_buf();
    let resolvers = rust_sources_under(&crates)
        .into_iter()
        .filter_map(|path| {
            let body = fs::read_to_string(&path).ok()?;
            let cutoff = body.find("#[cfg(test)]").unwrap_or(body.len());
            if !body[..cutoff].contains("sibling_python") {
                return None;
            }
            let relative = path.strip_prefix(&crates).ok()?;
            let owner = relative.components().next()?;
            Some(owner.as_os_str().to_string_lossy().into_owned())
        })
        .collect::<BTreeSet<_>>();
    assert!(
        resolvers.is_empty(),
        "unexpected interpreter resolvers: {resolvers:?}"
    );
}

#[derive(Debug, Clone, Copy)]
struct Probe {
    token: &'static str,
    argv: &'static [&'static str],
    expected_exit: i32,
    stderr_anchor: Option<&'static [u8]>,
}

#[test]
fn owner_verbs_are_registered_for_native_dispatch() {
    let native_tokens = NATIVE_PROCESS_SPECS
        .iter()
        .map(|spec| spec.token)
        .collect::<BTreeSet<_>>();
    let missing = OWNER_VERB_REQUIRED_NATIVE_TOKENS
        .iter()
        .copied()
        .filter(|token| !native_tokens.contains(token))
        .collect::<Vec<_>>();

    assert!(
        missing.is_empty(),
        "required owner-verb native process tokens are missing: {missing:?}"
    );
}

#[test]
fn brain_is_registered_for_native_dispatch() {
    let native_tokens = NATIVE_PROCESS_SPECS
        .iter()
        .map(|spec| spec.token)
        .collect::<BTreeSet<_>>();
    let missing = REQUIRED_NATIVE_TOKENS
        .iter()
        .copied()
        .filter(|token| !native_tokens.contains(token))
        .collect::<Vec<_>>();

    assert!(
        missing.is_empty(),
        "required brain native process tokens are missing: {missing:?}"
    );
}

// Registration is the load-bearing half of this assertion, not a bookkeeping
// one: `native_process_dispatch_and_poison_liveness_contract` requires the
// PROBES token set to equal NATIVE_PROCESS_SPECS exactly, so a token added
// here without a probe reddens that contract, and a probe runs the real
// dispatcher against sibling- and PATH-poisoned interpreters. Registering a
// token is therefore inseparable from proving its owner grammar reaches a
// native sibling with no interpreter in its path.
#[test]
fn think_and_setup_are_registered_for_native_dispatch() {
    let native_tokens = NATIVE_PROCESS_SPECS
        .iter()
        .map(|spec| spec.token)
        .collect::<BTreeSet<_>>();
    let missing = THINK_AND_SETUP_REQUIRED_NATIVE_TOKENS
        .iter()
        .copied()
        .filter(|token| !native_tokens.contains(token))
        .collect::<Vec<_>>();

    assert!(
        missing.is_empty(),
        "required native process tokens are missing: {missing:?}"
    );
}

// Keep both lifecycle tokens bound to their native owner grammars. The shared
// poison-liveness contract below requires an exact probe row for every native
// process-table entry, so a registration without executable proof stays red.
#[test]
fn talent_lifecycle_tokens_are_registered_for_native_dispatch() {
    let native_tokens = NATIVE_PROCESS_SPECS
        .iter()
        .map(|spec| spec.token)
        .collect::<BTreeSet<_>>();
    let missing = TALENT_LIFECYCLE_NATIVE_TOKENS
        .iter()
        .copied()
        .filter(|token| !native_tokens.contains(token))
        .collect::<Vec<_>>();

    assert!(
        missing.is_empty(),
        "talent lifecycle process tokens are still Python-routed: {missing:?}"
    );
}

// This test is committed to fail until `journal install-provider` dispatches to
// the native sibling. Registration is the load-bearing half: the poison
// liveness contract requires PROBES to equal NATIVE_PROCESS_SPECS exactly, so
// the row that turns this green also arms a probe that runs the real dispatcher
// with both interpreter poisons live. What resolves it: a NATIVE_PROCESS_SPECS
// row for `install-provider` plus its PROBES entry. What does not resolve it:
// do not #[ignore] this test, and do not shrink or reword
// INSTALL_PROVIDER_REQUIRED_NATIVE_TOKENS.
#[test]
fn install_provider_is_registered_for_native_dispatch() {
    let native_tokens = NATIVE_PROCESS_SPECS
        .iter()
        .map(|spec| spec.token)
        .collect::<BTreeSet<_>>();
    let missing = INSTALL_PROVIDER_REQUIRED_NATIVE_TOKENS
        .iter()
        .copied()
        .filter(|token| !native_tokens.contains(token))
        .collect::<Vec<_>>();

    assert!(
        missing.is_empty(),
        "install-provider is still Python-routed: {missing:?}"
    );
}

const SUPERVISOR_USAGE_ANCHOR: &[u8] =
    b"usage: journal supervisor [-h] [--no-daily] [--no-cortex] [--no-spl]\n";
const START_USAGE_ANCHOR: &[u8] =
    b"usage: journal start [-h] [--no-daily] [--no-cortex] [--no-spl]\n";
const SERVICE_UNKNOWN_ANCHOR: &[u8] = b"Unknown subcommand: --nonsense; Available: install, uninstall, start, stop, restart, status, logs\n";
const BACKUP_USAGE_ANCHOR: &[u8] = b"usage: journal backup <command> [options]\n";
const MAINTENANCE_USAGE_ANCHOR: &[u8] = b"usage: journal maintenance <command> [options]\n";

const PROBES: &[Probe] = &[
    Probe {
        token: "backup",
        argv: &["--nonsense"],
        expected_exit: 2,
        stderr_anchor: Some(BACKUP_USAGE_ANCHOR),
    },
    Probe {
        token: "maintenance",
        argv: &["--nonsense"],
        expected_exit: 2,
        stderr_anchor: Some(MAINTENANCE_USAGE_ANCHOR),
    },
    Probe {
        token: "brain",
        argv: &["--nonsense"],
        expected_exit: 2,
        stderr_anchor: Some(b"usage: journal brain"),
    },
    Probe {
        token: "config",
        argv: &["--nonsense"],
        expected_exit: 2,
        stderr_anchor: Some(b"usage: journal config"),
    },
    Probe {
        token: "depict",
        argv: &[],
        expected_exit: 1,
        // This schema/reason prefix proves depict reached its malformed-request path.
        stderr_anchor: Some(
            b"{\"schema\":\"solstone-depict-error-v1\",\"reason\":\"malformed-request\"",
        ),
    },
    Probe {
        token: "describe",
        argv: &["--nonsense"],
        expected_exit: 2,
        stderr_anchor: Some(DESCRIBE_USAGE_ANCHOR),
    },
    Probe {
        token: "spl",
        argv: &["--nope"],
        expected_exit: 2,
        stderr_anchor: Some(SPL_USAGE.as_bytes()),
    },
    // This parse-owned usage path proves the MCP dispatcher is present before
    // the endpoint feature can enter service startup.
    Probe {
        token: "mcp",
        argv: &["--nonsense"],
        expected_exit: 2,
        stderr_anchor: Some(MCP_USAGE.as_bytes()),
    },
    Probe {
        token: "schedule",
        argv: &["--nonsense"],
        expected_exit: 2,
        stderr_anchor: Some(SCHEDULE_USAGE.as_bytes()),
    },
    Probe {
        token: "convey",
        argv: &["--nonsense"],
        expected_exit: 2,
        stderr_anchor: Some(CONVEY_USAGE_ANCHOR),
    },
    Probe {
        token: "health",
        argv: &["--nonsense"],
        expected_exit: 2,
        stderr_anchor: Some(HEALTH_USAGE.as_bytes()),
    },
    Probe {
        token: "heartbeat",
        argv: &["--nonsense"],
        expected_exit: 2,
        stderr_anchor: Some(b"usage: journal heartbeat [-h] [--force]\n"),
    },
    Probe {
        token: "engage",
        argv: &["--nonsense"],
        expected_exit: 2,
        stderr_anchor: Some(b"usage: journal engage [-h] [--wait]"),
    },
    Probe {
        token: "top",
        argv: &["--nonsense"],
        expected_exit: 2,
        stderr_anchor: Some(TOP_USAGE.as_bytes()),
    },
    Probe {
        token: "cortex",
        argv: &["--nonsense"],
        expected_exit: 2,
        stderr_anchor: Some(b"usage: journal cortex"),
    },
    Probe {
        token: "talent",
        argv: &["--nonsense"],
        expected_exit: 2,
        stderr_anchor: Some(
            b"usage: journal talent [-h] [-v] [-d] {list,inventory,show,logs,log} ...",
        ),
    },
    Probe {
        token: "service",
        argv: &["--nonsense"],
        expected_exit: 1,
        stderr_anchor: Some(SERVICE_UNKNOWN_ANCHOR),
    },
    Probe {
        token: "up",
        argv: &["--nonsense"],
        expected_exit: 1,
        stderr_anchor: Some(SERVICE_UNKNOWN_ANCHOR),
    },
    Probe {
        token: "down",
        argv: &["--nonsense"],
        expected_exit: 1,
        stderr_anchor: Some(SERVICE_UNKNOWN_ANCHOR),
    },
    // Doctor's invalid-argument path exits 2 only after the sibling recognizes
    // the verb and emits its owner-facing usage. If the doctor arm were absent,
    // this invocation would fall through to top-level USAGE and exit 64 instead;
    // the journal-doctor anchor makes that regression distinguishable.
    Probe {
        token: "doctor",
        argv: &["--nonsense"],
        expected_exit: 2,
        stderr_anchor: Some(b"usage: journal doctor [-h]"),
    },
    Probe {
        token: "setup",
        argv: &["--nonsense"],
        expected_exit: 2,
        stderr_anchor: Some(b"usage: journal setup [-h] [--journal PATH] [--port INT]"),
    },
    // Exit 2 and the parser-owned usage anchors distinguish each present verb
    // from top-level exit 64, which would also result if the sibling lacked it.
    Probe {
        token: "check",
        argv: &["--nonsense"],
        expected_exit: 2,
        stderr_anchor: Some(CHECK_USAGE_ANCHOR),
    },
    Probe {
        token: "install-models",
        argv: &["--nonsense"],
        expected_exit: 2,
        stderr_anchor: Some(INSTALL_MODELS_USAGE_ANCHOR),
    },
    // Exit 2 with the verb's own usage on stderr, never the top-level 64 --
    // which is the code the binary emits for a verb the sibling does not have
    // at all, so a row registered there certifies nothing. `--nonsense` also
    // stops at argument parsing, before the supervisor gate this verb runs, so
    // the probe does not depend on whether a journal service is up.
    Probe {
        token: "install-provider",
        argv: &["--nonsense"],
        expected_exit: 2,
        stderr_anchor: Some(INSTALL_PROVIDER_USAGE_ANCHOR),
    },
    Probe {
        token: "thinking",
        argv: &["--nonsense"],
        expected_exit: 2,
        stderr_anchor: Some(THINKING_USAGE.as_bytes()),
    },
    // supervisor and start were deliberately unprobed until a verb-level usage
    // path existed for them. 1d1523b4b added both tokens to NATIVE_PROCESS_SPECS
    // without probes; registering them at exit 64 would have made this contract
    // green while making it check less, because 64 is the same code the binary
    // emits for a verb it does not have at all -- a row expecting it certifies
    // the token on a code that would survive deleting supervisor from the
    // sibling entirely. parse_supervisor now has a verb-level usage path, so
    // both tokens exit 2 with supervisor's own usage and are registered against
    // that, with a stderr anchor. The red is resolved by proof, not by relaxing
    // the guard. `spl` now carries the same verb-level proof in its row above.
    //
    // `start` is the owner-facing daemon verb and keeps its own invocation token.
    // `supervisor` remains as the internal/debug spelling. Each usage banner names
    // the token the owner typed.
    //
    Probe {
        token: "supervisor",
        argv: &["--nonsense"],
        expected_exit: 2,
        stderr_anchor: Some(SUPERVISOR_USAGE_ANCHOR),
    },
    Probe {
        token: "start",
        argv: &["--nonsense"],
        expected_exit: 2,
        stderr_anchor: Some(START_USAGE_ANCHOR),
    },
    Probe {
        token: "grab",
        argv: &["--nonsense"],
        expected_exit: 2,
        stderr_anchor: None,
    },
    Probe {
        token: "contract",
        argv: &["--nonsense"],
        expected_exit: 2,
        stderr_anchor: None,
    },
    Probe {
        token: "transfer",
        argv: &["--nonsense"],
        expected_exit: 2,
        stderr_anchor: None,
    },
    Probe {
        token: "transcribe",
        argv: &["--nonsense"],
        expected_exit: 2,
        stderr_anchor: None,
    },
    Probe {
        token: "sense",
        argv: &["--nonsense"],
        expected_exit: 2,
        stderr_anchor: Some(b"usage: journal sense"),
    },
    Probe {
        token: "facet-candidates",
        argv: &["--nonsense"],
        expected_exit: 2,
        stderr_anchor: None,
    },
    Probe {
        token: "navigate",
        argv: &["--nonsense"],
        expected_exit: 2,
        stderr_anchor: None,
    },
    Probe {
        token: "identity",
        argv: &["--nonsense"],
        expected_exit: 2,
        stderr_anchor: None,
    },
    Probe {
        token: "importer",
        argv: &["--nonsense"],
        expected_exit: 2,
        stderr_anchor: Some(b"usage: journal importer [-h] [options] media [timestamp]\n"),
    },
    Probe {
        token: "settings",
        argv: &["--nonsense"],
        expected_exit: 2,
        stderr_anchor: None,
    },
    Probe {
        token: "streams",
        argv: &["--nonsense"],
        expected_exit: 2,
        stderr_anchor: None,
    },
    Probe {
        token: "segment",
        argv: &["--nonsense"],
        expected_exit: 2,
        stderr_anchor: None,
    },
    Probe {
        token: "journal-stats",
        argv: &["--nonsense"],
        expected_exit: 2,
        stderr_anchor: None,
    },
    Probe {
        token: "reprocess",
        argv: &["--nonsense"],
        expected_exit: 2,
        stderr_anchor: None,
    },
    Probe {
        token: "backfill-processing-records",
        argv: &["--nonsense"],
        expected_exit: 2,
        stderr_anchor: None,
    },
    Probe {
        token: "think",
        argv: &["--nonsense"],
        expected_exit: 2,
        stderr_anchor: Some(b"usage: journal think [-h]"),
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
    StderrAnchorMismatch {
        token: &'static str,
        expected: &'static [u8],
        actual: Vec<u8>,
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
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("core workspace root")
            .join("target/journal-native-process-contract-tmp");
        fs::create_dir_all(&base).expect("create contract test target directory");
        let path = base.join(format!(
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

/// Snapshots an artifact that Cargo produced during this test-binary run. The
/// process-owned bytes remain stable if another CI suite subsequently rebuilds
/// the shared Cargo target.
#[derive(Clone)]
struct Artifact {
    bytes: Arc<[u8]>,
}

/// The three sibling helpers are built once for this integration-test binary.
/// Their fresh outputs are snapshotted before any harness needs them, so every
/// harness can preserve its own filesystem and PATH isolation without trusting
/// a mutable shared target directory.
static WORKSPACE_BINARY_ARTIFACTS: OnceLock<BTreeMap<&'static str, Artifact>> = OnceLock::new();
static WORKSPACE_DISPATCHER_ARTIFACT: OnceLock<Artifact> = OnceLock::new();

struct Harness {
    _temp: TempDir,
    dispatcher: PathBuf,
    sibling_dir: PathBuf,
    dispatcher_artifact: Artifact,
    sibling_artifacts: BTreeMap<&'static str, Artifact>,
    home: PathBuf,
    journal: PathBuf,
    poison_marker: PathBuf,
    probe_stderr: PathBuf,
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
    probe_stderr: &'a Path,
}

impl Harness {
    fn new() -> Self {
        let temp = TempDir::new("bin");
        let sibling_dir = temp.path.join("bin");
        fs::create_dir(&sibling_dir).expect("create binary directory");

        let dispatcher_artifact = workspace_dispatcher_artifact().clone();
        let sibling_artifacts = workspace_binary_artifacts().clone();
        let core_artifact = &sibling_artifacts["solstone-core"];
        let depict_artifact = &sibling_artifacts["solstone-core-depict"];
        let describe_artifact = &sibling_artifacts["solstone-core-describe"];

        let dispatcher = sibling_dir.join("solstone-core-journal");
        copy_artifact(&dispatcher_artifact, &dispatcher);
        std::os::unix::fs::symlink(&dispatcher, sibling_dir.join("journal"))
            .expect("link journal command to dispatcher sibling");
        copy_artifact(core_artifact, &sibling_dir.join("solstone-core"));
        copy_artifact(depict_artifact, &sibling_dir.join("solstone-core-depict"));
        copy_artifact(
            describe_artifact,
            &sibling_dir.join("solstone-core-describe"),
        );
        let speakers_helper = sibling_dir.join("solstone-core-speakers-analyze");
        fs::write(&speakers_helper, "#!/bin/sh\nexit 1\n")
            .expect("write speakers-analyze placeholder");
        make_executable(&speakers_helper);
        for interpreter in ["python", "python3", "pytest", "uv", "ruff"] {
            let path = sibling_dir.join(interpreter);
            fs::write(&path, POISON_INTERPRETER).expect("write poison interpreter");
            make_executable(&path);
        }

        let home = temp.path.join("home");
        let journal = temp.path.join("journal");
        let poison_marker = temp.path.join("python-invoked.txt");
        let probe_stderr = temp.path.join("probe-stderr.txt");
        Self {
            _temp: temp,
            dispatcher,
            sibling_dir,
            dispatcher_artifact,
            sibling_artifacts,
            home,
            journal,
            poison_marker,
            probe_stderr,
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
            probe_stderr: &self.probe_stderr,
        }
    }
}

fn artifact(path: &Path) -> Artifact {
    Artifact {
        bytes: fs::read(path)
            .unwrap_or_else(|error| panic!("read built artifact {}: {error}", path.display()))
            .into(),
    }
}

fn workspace_binary_artifacts() -> &'static BTreeMap<&'static str, Artifact> {
    WORKSPACE_BINARY_ARTIFACTS.get_or_init(|| {
        [
            ("solstone-core", "solstone-core", "solstone-core"),
            (
                "solstone-core-depict",
                "solstone-core-depict",
                "solstone-core-depict",
            ),
            (
                "solstone-core-describe",
                "solstone-core-describe",
                "solstone-core-describe",
            ),
        ]
        .into_iter()
        .map(|(name, package, binary)| (name, snapshot_workspace_binary(package, binary)))
        .collect()
    })
}

fn workspace_dispatcher_artifact() -> &'static Artifact {
    WORKSPACE_DISPATCHER_ARTIFACT.get_or_init(|| {
        snapshot_workspace_binary("solstone-core-journal-bin", "solstone-core-journal")
    })
}

fn snapshot_workspace_binary(package: &str, binary: &str) -> Artifact {
    for attempt in 0..2 {
        let source = locate_workspace_binary(package, binary);
        match fs::read(&source) {
            Ok(bytes) => {
                return Artifact {
                    bytes: bytes.into(),
                };
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound && attempt == 0 => continue,
            Err(error) => panic!(
                "read built {package}/{binary} artifact {}: {error}",
                source.display()
            ),
        }
    }
    unreachable!("two snapshot attempts either return or report their read error")
}

fn copy_executable(source: &Path, destination: &Path) {
    fs::copy(source, destination).expect("copy executable");
    make_executable(destination);
}

fn copy_artifact(source: &Artifact, destination: &Path) {
    fs::write(destination, source.bytes.as_ref()).expect("write copied executable");
    make_executable(destination);
}

fn make_executable(path: &Path) {
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("make fixture executable");
}

fn install_ready_restic_fixture(home: &Path) -> PathBuf {
    let (tool_dir, platform_os) = if cfg!(target_os = "macos") {
        (
            home.join("Library/Application Support/solstone/restic"),
            "darwin",
        )
    } else {
        (home.join(".cache/solstone/restic"), "linux")
    };
    let platform_arch = match env::consts::ARCH {
        "x86_64" | "amd64" => "amd64",
        "aarch64" | "arm64" => "arm64",
        other => panic!("unsupported Restic fixture architecture: {other}"),
    };
    fs::create_dir_all(&tool_dir).expect("create Restic fixture directory");
    let binary = tool_dir.join("restic");
    fs::write(
        &binary,
        "#!/bin/sh\ncase \" $* \" in\n*\" version \"*) printf 'restic 0.19.0 compiled with go1.test on test/test\\n' ;;\n*\" backup \"*) printf '[{\"message_type\":\"summary\",\"snapshot_id\":\"fixture-snapshot\"}]\\n' ;;\n*) exit 0 ;;\nesac\n",
    )
    .expect("write Restic fixture");
    make_executable(&binary);
    let digest = format!(
        "{:x}",
        Sha256::digest(fs::read(&binary).expect("read Restic fixture"))
    );
    fs::write(
        tool_dir.join(".install-complete"),
        serde_json::to_vec(&serde_json::json!({
            "schema_version": 1,
            "tool": "restic",
            "version": "0.19.0",
            "sha256": digest,
            "platform": {"os": platform_os, "arch": platform_arch},
            "binary_path": binary.to_string_lossy(),
        }))
        .expect("encode Restic fixture sentinel"),
    )
    .expect("write Restic fixture sentinel");
    binary
}

fn locate_workspace_binary(package: &str, binary: &str) -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_manifest = manifest_dir
        .parent()
        .expect("crates dir")
        .parent()
        .expect("core dir")
        .join("Cargo.toml");
    let mut command = Command::new(env!("CARGO"));
    command
        .args(["build", "--manifest-path"])
        .arg(&workspace_manifest)
        .args(["-p", package, "--bin", binary]);
    if package == "solstone-core-describe" && binary.starts_with("solstone-describe-") {
        command.args(["--features", "test-stubs"]);
    }
    let output = command
        .arg("--message-format=json")
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
            child.wait()?;
            return Ok(None);
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn run_dispatcher_with_bounded_output(
    context: &VerdictContext<'_>,
    token: &str,
    argv: &[&str],
    timeout: Duration,
) -> io::Result<Option<std::process::Output>> {
    let _ = fs::remove_file(context.poison_marker);
    let _ = fs::remove_file(context.probe_stderr);
    let stdout_path = context.probe_stderr.with_extension("stdout");
    let _ = fs::remove_file(&stdout_path);
    let stdout = fs::File::create(&stdout_path)?;
    let stderr = fs::File::create(context.probe_stderr)?;
    let mut child = Command::new(context.dispatcher)
        .arg(token)
        .args(argv)
        .env("POISON_MARKER", context.poison_marker)
        .env("HOME", context.home)
        .env("SOLSTONE_JOURNAL", context.journal)
        .env("PATH", context.sibling_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()?;
    let status = wait_for_child(&mut child, Instant::now() + timeout)?;
    let stdout = fs::read(stdout_path)?;
    let stderr = fs::read(context.probe_stderr)?;
    Ok(status.map(|status| std::process::Output {
        status,
        stdout,
        stderr,
    }))
}

fn run_dispatcher_with_timeout(
    context: &VerdictContext<'_>,
    token: &str,
    argv: &[&str],
    timeout: Duration,
) -> io::Result<Option<ExitStatus>> {
    let _ = fs::remove_file(context.poison_marker);
    let _ = fs::remove_file(context.probe_stderr);
    // PATH is pinned to the poisoned sibling directory, and that is an ADDITION
    // to the sibling poison rather than a substitute for it. The two poisons are
    // not interchangeable and must both hold:
    //
    //   * the SIBLING poison is the load-bearing one. It tests the shipped
    //     dispatch path: the retired interpreter-resolution family
    //     (`sibling_python_for_executable`) only ever resolved `python3`
    //     beside `current_exe()`, never from PATH. A PATH shim alone proves
    //     nothing about that route.
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
    let stderr = fs::File::create(context.probe_stderr)?;
    let mut child = Command::new(context.dispatcher)
        .arg(token)
        .args(argv)
        .env("POISON_MARKER", context.poison_marker)
        .env("HOME", context.home)
        .env("SOLSTONE_JOURNAL", context.journal)
        .env("PATH", context.sibling_dir)
        .stdout(Stdio::null())
        .stderr(Stdio::from(stderr))
        .spawn()?;
    wait_for_child(&mut child, Instant::now() + timeout)
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

fn run_dispatcher_with_output_and_environment(
    context: &VerdictContext<'_>,
    token: &str,
    argv: &[&str],
    environment: &[(&str, &str)],
) -> io::Result<std::process::Output> {
    Command::new(context.dispatcher)
        .arg(token)
        .args(argv)
        .env("POISON_MARKER", context.poison_marker)
        .env("HOME", context.home)
        .env("SOLSTONE_JOURNAL", context.journal)
        .env("PATH", context.sibling_dir)
        .envs(environment.iter().copied())
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

struct BrainProbeStub {
    url: String,
    requests: Arc<Mutex<Vec<String>>>,
    worker: thread::JoinHandle<()>,
}

impl BrainProbeStub {
    fn start(expected_requests: usize) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind brain probe stub");
        listener
            .set_nonblocking(true)
            .expect("make brain probe listener nonblocking");
        let url = format!("http://{}", listener.local_addr().expect("stub address"));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let worker_requests = Arc::clone(&requests);
        let worker = thread::spawn(move || {
            let deadline = Instant::now() + PROBE_TIMEOUT;
            while Instant::now() < deadline
                && worker_requests.lock().expect("stub request lock").len() < expected_requests
            {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream
                            .set_nonblocking(false)
                            .expect("make accepted brain probe stream blocking");
                        stream
                            .set_read_timeout(Some(PROBE_TIMEOUT))
                            .expect("bound accepted brain probe stream reads");
                        let request = read_http_request(&mut stream);
                        let tool_call = request.contains("emit_final");
                        worker_requests
                            .lock()
                            .expect("stub request lock")
                            .push(request);
                        let body = if tool_call {
                            r#"{"choices":[{"message":{"content":"","tool_calls":[{"id":"final-1","type":"function","function":{"name":"emit_final","arguments":"{\"content\":\"OK\"}"}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}"#
                        } else {
                            r#"{"choices":[{"message":{"content":"OK"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}"#
                        };
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        stream
                            .write_all(response.as_bytes())
                            .expect("write stub response");
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("accept brain probe request: {error}"),
                }
            }
        });
        Self {
            url,
            requests,
            worker,
        }
    }

    fn finish(self) -> Vec<String> {
        self.worker.join().expect("brain probe stub joins");
        Arc::try_unwrap(self.requests)
            .expect("brain probe request ownership")
            .into_inner()
            .expect("brain probe request lock")
    }
}

fn read_http_request(stream: &mut TcpStream) -> String {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 1024];
    let header_end = loop {
        let read = stream.read(&mut buffer).expect("read brain probe request");
        assert!(read > 0, "brain probe request closed before headers");
        bytes.extend_from_slice(&buffer[..read]);
        if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let header = std::str::from_utf8(&bytes[..header_end]).expect("brain probe headers UTF-8");
    let content_length = header.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("content-length")
            .then(|| value.trim().parse::<usize>().ok())
            .flatten()
    });
    if let Some(content_length) = content_length {
        while bytes.len() < header_end + content_length {
            let read = stream
                .read(&mut buffer)
                .expect("read brain probe request body");
            assert!(read > 0, "brain probe request closed before body");
            bytes.extend_from_slice(&buffer[..read]);
        }
    } else {
        stream
            .set_read_timeout(Some(Duration::from_millis(100)))
            .expect("set chunked request timeout");
        loop {
            match stream.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => bytes.extend_from_slice(&buffer[..read]),
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                    ) =>
                {
                    break;
                }
                Err(error) => panic!("read chunked brain probe request: {error}"),
            }
        }
    }
    String::from_utf8(bytes).expect("brain probe request UTF-8")
}

fn write_brain_byo_endpoint_config(journal: &Path, endpoint_url: &str) {
    fs::create_dir_all(journal.join("config")).expect("create brain config directory");
    fs::write(
        journal.join("config/journal.json"),
        format!(
            r#"{{"providers":{{"active":{{"provider":"local"}},"local":{{"endpoint_url":"{endpoint_url}","served_model_id":"brain-stub"}}}}}}"#
        ),
    )
    .expect("write brain endpoint config");
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
    verdict_for_with_timeout(spec, probe, context, checked, PROBE_TIMEOUT)
}

fn verdict_for_with_timeout(
    spec: &NativeProcessSpec,
    probe: Option<&Probe>,
    context: &VerdictContext<'_>,
    checked: &mut BTreeSet<&'static str>,
    timeout: Duration,
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
    let status = match run_dispatcher_with_timeout(context, spec.token, probe.argv, timeout) {
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
    if let Some(anchor) = probe.stderr_anchor {
        let actual = match fs::read(context.probe_stderr) {
            Ok(stderr) => stderr,
            Err(error) => {
                return Verdict::LaunchFailure {
                    token: spec.token,
                    error: format!("read captured probe stderr: {error}"),
                };
            }
        };
        if !actual.starts_with(anchor) {
            return Verdict::StderrAnchorMismatch {
                token: spec.token,
                expected: anchor,
                actual,
            };
        }
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
        probe_tokens.len(),
        PROBES.len(),
        "native process probe tokens must be globally unique"
    );
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
    let process_tokens = PROCESS_SPECS
        .iter()
        .map(|spec| spec.token)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        process_tokens, native_tokens,
        "every PROCESS_SPECS token must have a NATIVE_PROCESS_SPECS row"
    );
}

#[test]
fn native_backup_grammar_never_reaches_a_poisoned_interpreter() {
    let harness = Harness::new();
    let context = harness.context();
    prove_poison_interpreters_live(&context);
    let shell = context.sibling_dir.join("sh");
    fs::write(&shell, POISON_INTERPRETER).expect("write poison shell");
    make_executable(&shell);
    fs::create_dir_all(context.journal).expect("create backup journal root");
    let restic = install_ready_restic_fixture(context.home);
    assert!(restic.starts_with(context.home));

    // These cover every owner-facing backup leaf under the same sibling/PATH
    // interpreter poison as the registration contract. The success cases use
    // empty configuration; the error cases stop before any live tool/network
    // seam, so no backup provider or package host is contacted.
    for (argv, expected_exit) in [
        (["status"].as_slice(), 0),
        (["destination", "show"].as_slice(), 0),
        (["destination", "set"].as_slice(), 1),
        (["destination", "set-hosted"].as_slice(), 1),
        (["enable"].as_slice(), 1),
        (["run"].as_slice(), 0),
        (["prune"].as_slice(), 0),
        (["offload", "status"].as_slice(), 0),
        (["offload", "run"].as_slice(), 0),
        (["offload", "restore"].as_slice(), 1),
        (["recovery-key", "show"].as_slice(), 1),
        (["recovery-key", "rotate"].as_slice(), 0),
        (["restore"].as_slice(), 1),
        (["off", "--yes"].as_slice(), 0),
    ] {
        let output = run_dispatcher_with_bounded_output(&context, "backup", argv, PROBE_TIMEOUT)
            .expect("run native backup grammar through dispatcher")
            .expect("backup grammar did not time out");
        assert_eq!(
            output.status.code(),
            Some(expected_exit),
            "backup {argv:?} stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            !context.poison_marker.exists(),
            "backup {argv:?} reached a poisoned interpreter"
        );
    }

    // The representative success body receives a release-shaped restic JSON
    // fixture. It proves the native runner boundary without a live repository.
    let recovery = "0123456789ABCDEFGHJKMNPQRSTVWXYZ0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let config = context.journal.join("config/journal.json");
    fs::create_dir_all(config.parent().expect("backup config parent"))
        .expect("create backup config parent");
    fs::write(
        config,
        serde_json::to_vec(&serde_json::json!({"backup": {
            "enabled": true,
            "daily_key": "daily",
            "recovery_key": recovery,
            "destination": {
                "repository": "s3:bucket/prefix",
                "backend": "s3",
                "credentials": {"access_key_id": "fixture-access", "secret_access_key": "fixture-secret"}
            }
        }}))
        .expect("encode backup config"),
    )
    .expect("write backup config");
    let output = run_dispatcher_with_bounded_output(&context, "backup", &["run"], PROBE_TIMEOUT)
        .expect("run native backup success fixture")
        .expect("backup success fixture did not time out");
    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        output.stdout,
        b"Backup complete (snapshot fixture-snapshot).\n"
    );
    assert!(
        !context.poison_marker.exists(),
        "native backup success reached a poisoned interpreter"
    );
}

#[test]
fn native_maintenance_bodies_reach_real_native_owners_without_python() {
    let harness = Harness::new();
    let context = harness.context();
    prove_poison_interpreters_live(&context);
    fs::create_dir_all(context.journal.join("config")).expect("create maintenance config parent");
    fs::create_dir_all(context.journal.join("chronicle")).expect("create empty chronicle");
    fs::write(
        context.journal.join("config/journal.json"),
        serde_json::to_vec(&serde_json::json!({
            "retention": {
                "raw_media": "keep",
                "journal_logs": {"enabled": false}
            }
        }))
        .expect("encode maintenance config"),
    )
    .expect("write maintenance config");
    let shell = context.sibling_dir.join("sh");
    fs::write(&shell, POISON_INTERPRETER).expect("write poison shell");
    make_executable(&shell);

    for (argv, expected_exit, witness) in [
        (["list"].as_slice(), 0, "backup:run"),
        (["sync"].as_slice(), 0, "added:"),
        (["run", "backup:run"].as_slice(), 0, "backup:"),
        (["run", "backup:prune"].as_slice(), 0, "backup prune:"),
        (["run", "backup:verify"].as_slice(), 0, "backup verify:"),
        (
            ["run", "backup:offload", "--dry-run"].as_slice(),
            0,
            "backup offload:",
        ),
        (["run", "health:mark-raw"].as_slice(), 0, "new items: 0"),
        (
            ["run", "health:prune-logs"].as_slice(),
            0,
            "prune-logs: disabled",
        ),
        (
            ["run", "timeline:rollup-day", "--day", "20260301"].as_slice(),
            66,
            "no verified segment timeline.json found",
        ),
        (
            ["run", "timeline:rollup-master"].as_slice(),
            66,
            "no day-level timeline.json",
        ),
    ] {
        let output = run_dispatcher_with_bounded_output(
            &context,
            "maintenance",
            argv,
            MAINTENANCE_PROBE_TIMEOUT,
        )
        .expect("dispatch native maintenance body")
        .unwrap_or_else(|| panic!("maintenance {argv:?} completes before deadline"));
        assert_eq!(
            output.status.code(),
            Some(expected_exit),
            "maintenance {argv:?} stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).contains(witness),
            "maintenance {argv:?} stdout: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert!(
            !context.poison_marker.exists(),
            "maintenance {argv:?} reached a poisoned interpreter"
        );
    }
}

#[test]
fn native_setup_full_run_never_reaches_a_poisoned_interpreter() {
    let harness = Harness::new();
    let context = harness.context();
    prove_poison_interpreters_live(&context);
    let fixture = locate_workspace_binary("solstone-core-journal-bin", "setup-fixture-journal");
    let journal = context.sibling_dir.join("journal");
    let solstone = context.sibling_dir.join("solstone");
    fs::remove_file(&journal).expect("replace dispatcher journal link");
    copy_executable(&fixture, &journal);
    copy_executable(&fixture, &solstone);
    let run = |path: &Path, journal_path: &Path| {
        let _ = fs::remove_file(context.poison_marker);
        let output = Command::new(&journal)
            .args(["setup", "--yes"])
            .env("HOME", context.home)
            .env("SOLSTONE_JOURNAL", journal_path)
            .env("SETUP_FIXTURE_BIN_DIR", context.sibling_dir)
            .env("PATH", path)
            .env("POISON_MARKER", context.poison_marker)
            .output()
            .expect("run native setup fixture");
        assert_eq!(
            output.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            !context.poison_marker.exists(),
            "setup reached poisoned interpreter"
        );
        let manifest: serde_json::Value = serde_json::from_slice(
            &fs::read(journal_path.join("health/setup-state.json")).expect("setup manifest"),
        )
        .expect("manifest JSON");
        assert_eq!(manifest["steps"].as_array().map(Vec::len), Some(8));
        assert!(manifest["completed_at"].is_string());
    };
    run(context.sibling_dir, context.journal);
    let empty_path = context
        .sibling_dir
        .parent()
        .expect("fixture parent")
        .join("empty-path");
    fs::create_dir(&empty_path).expect("create empty PATH");
    let second_journal = context.journal.join("second");
    run(&empty_path, &second_journal);
}

#[test]
fn brain_internal_protocol_verbs_are_closed_at_the_real_owner_dispatcher() {
    let harness = Harness::new();
    let context = harness.context();
    prove_poison_interpreters_live(&context);
    fs::create_dir_all(context.journal).expect("create isolated brain journal");
    let before = snapshot_tree(context.journal);

    for argv in [
        &["inspect"][..],
        &["fingerprint"][..],
        &["record-runtime-failure"][..],
        &["refresh", "--session"][..],
        &["prerequisite-renewal", "--session"][..],
    ] {
        let output = run_dispatcher_with_bounded_output(&context, "brain", argv, PROBE_TIMEOUT)
            .expect("run internal brain spelling")
            .expect("internal brain spelling completes");
        assert_eq!(output.status.code(), Some(2), "argv={argv:?}");
        assert!(output.stdout.is_empty(), "argv={argv:?}");
        assert!(
            output.stderr.starts_with(b"usage: journal brain"),
            "argv={argv:?} stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            !context.poison_marker.exists(),
            "argv={argv:?} reached poisoned interpreter"
        );
        assert_eq!(
            snapshot_tree(context.journal),
            before,
            "argv={argv:?} mutated durable state"
        );
    }
}

#[test]
fn brain_owner_short_paths_are_poison_clean_through_the_real_dispatcher() {
    let harness = Harness::new();
    let context = harness.context();
    prove_poison_interpreters_live(&context);
    fs::create_dir_all(context.journal).expect("create isolated brain journal");

    // Status and a stale expected refresh are deliberately short paths. They
    // establish that the owner grammar and fence short-circuit never recover
    // through the retained Python owner.
    let status = run_dispatcher_with_bounded_output(&context, "brain", &["status"], PROBE_TIMEOUT)
        .expect("run brain status")
        .expect("brain status completes");
    assert_eq!(status.status.code(), Some(2));
    assert!(status.stdout.starts_with(b"Brain unknown:"));
    assert!(
        !context.poison_marker.exists(),
        "status reached poisoned interpreter"
    );

    let stale = run_dispatcher_with_bounded_output(
        &context,
        "brain",
        &["refresh", "--expected-fingerprint", "not-a-fingerprint"],
        PROBE_TIMEOUT,
    )
    .expect("run stale brain refresh")
    .expect("stale brain refresh completes");
    assert_eq!(stale.status.code(), Some(3));
    assert_eq!(stale.stdout, b"Brain unknown: stale expected fingerprint\n");
    assert!(
        !context.poison_marker.exists(),
        "stale refresh reached poisoned interpreter"
    );

    // The full owner refresh must cross the native generate and diagnostic
    // cogitate child boundaries. The local server distinguishes cogitate by
    // its `emit_final` tool declaration; both requests stay below the real
    // journal dispatcher, with every possible interpreter poisoned.
    let stub = BrainProbeStub::start(4);
    write_brain_byo_endpoint_config(context.journal, &stub.url);
    let refresh =
        run_dispatcher_with_bounded_output(&context, "brain", &["refresh"], PROBE_TIMEOUT)
            .expect("run full brain refresh")
            .expect("full brain refresh completes");
    assert!(
        matches!(refresh.status.code(), Some(0..=2)),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&refresh.stdout),
        String::from_utf8_lossy(&refresh.stderr)
    );
    assert!(
        !context.poison_marker.exists(),
        "full refresh reached poisoned interpreter"
    );

    // A BYO endpoint is not SPP-safe, so renewal delegates to the same full
    // refresh path. The same stub must see its second generate/cogitate pair.
    let renewal = run_dispatcher_with_bounded_output(
        &context,
        "brain",
        &["renew-prerequisites"],
        PROBE_TIMEOUT,
    )
    .expect("run unsafe prerequisite renewal")
    .expect("unsafe prerequisite renewal completes");
    assert!(
        matches!(renewal.status.code(), Some(0..=2)),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&renewal.stdout),
        String::from_utf8_lossy(&renewal.stderr)
    );
    assert!(
        !context.poison_marker.exists(),
        "unsafe renewal fallback reached poisoned interpreter"
    );

    let requests = stub.finish();
    assert_eq!(requests.len(), 4, "two refreshes must each probe twice");
    assert!(
        requests
            .iter()
            .any(|request| request.contains("emit_final")),
        "cogitate request was not observed"
    );
    assert!(
        requests
            .iter()
            .any(|request| !request.contains("emit_final")),
        "generate request was not observed"
    );

    // Hardware-backed SPP attestation cannot reach Started in this isolated
    // dispatcher fixture. Its stale fence is still a real owner short path.
    let spp_stale = run_dispatcher_with_bounded_output(
        &context,
        "brain",
        &[
            "renew-prerequisites",
            "--expected-fingerprint",
            "not-a-fingerprint",
        ],
        PROBE_TIMEOUT,
    )
    .expect("run stale prerequisite renewal")
    .expect("stale prerequisite renewal completes");
    assert_eq!(spp_stale.status.code(), Some(3));
    assert!(
        !context.poison_marker.exists(),
        "renewal short path reached poisoned interpreter"
    );
}

#[test]
fn native_sense_batch_keeps_transcribe_and_describe_free_of_any_interpreter() {
    let harness = Harness::new();
    let context = harness.context();
    let day = "20990101";
    let stream = "capture";
    let audio_segment = "120000_1";
    let audio_directory = context
        .journal
        .join("chronicle")
        .join(day)
        .join(stream)
        .join(audio_segment);
    fs::create_dir_all(&audio_directory).expect("create audio batch segment");
    fs::write(audio_directory.join("audio.flac"), b"not a flac file").expect("write garbage audio");

    // describe now resolves through NATIVE_PROCESS_SPECS, so this batch must
    // touch no interpreter at all. Tightening it is what the comment that
    // stood here asked for by name; it was not done when the cutover landed,
    // so the assertion went on requiring a shim touch that no longer happens
    // and failed on precisely the outcome it exists to protect.
    let audio_output = run_dispatcher_with_output_and_environment(
        &context,
        "sense",
        &["--day", day, "--stream", stream, "--segment", audio_segment],
        &[("SOL_SKIP_SUPERVISOR_CHECK", "1")],
    )
    .expect("run native audio sense batch through dispatcher");
    // On macOS native media decoders report corrupt input as a failed child,
    // while the Linux implementation records the same durable failure without
    // failing the batch. Both outcomes prove the dispatcher exercised native
    // handlers rather than an interpreter.
    assert!(
        if cfg!(target_os = "macos") {
            matches!(audio_output.status.code(), Some(0 | 75))
        } else {
            audio_output.status.code() == Some(0)
        },
        "sense stderr: {}",
        String::from_utf8_lossy(&audio_output.stderr),
    );
    #[cfg(not(target_os = "macos"))]
    {
        let audio_sidecar: serde_json::Value = serde_json::from_slice(
            &fs::read(audio_directory.join("audio.jsonl"))
                .expect("native transcribe decode-failure sidecar"),
        )
        .expect("transcribe sidecar JSON");
        assert_eq!(
            audio_sidecar["_solstone_processing"]["handler"],
            "transcribe"
        );
        assert_eq!(audio_sidecar["_solstone_processing"]["state"], "failed");
        assert_eq!(
            audio_sidecar["_solstone_processing"]["reason_code"],
            "corrupt_input"
        );
    }

    // A failed video decoder can short-circuit a batch before a sibling worker
    // writes its record. A separate segment keeps both native worker witnesses
    // deterministic while still exercising the dispatcher boundary.
    let screen_segment = "120001_2";
    let screen_directory = context
        .journal
        .join("chronicle")
        .join(day)
        .join(stream)
        .join(screen_segment);
    fs::create_dir_all(&screen_directory).expect("create screen batch segment");
    fs::write(screen_directory.join("screen.webm"), b"not a webm file")
        .expect("write garbage video");
    let screen_output = run_dispatcher_with_output_and_environment(
        &context,
        "sense",
        &[
            "--day",
            day,
            "--stream",
            stream,
            "--segment",
            screen_segment,
        ],
        &[("SOL_SKIP_SUPERVISOR_CHECK", "1")],
    )
    .expect("run native screen sense batch through dispatcher");
    assert!(
        if cfg!(target_os = "macos") {
            matches!(screen_output.status.code(), Some(0 | 75))
        } else {
            screen_output.status.code() == Some(0)
        },
        "sense stderr: {}",
        String::from_utf8_lossy(&screen_output.stderr),
    );
    let screen_sidecar: serde_json::Value = serde_json::from_slice(
        &fs::read(screen_directory.join("screen.jsonl"))
            .expect("native describe decode-failure sidecar"),
    )
    .expect("describe sidecar JSON");
    assert_eq!(
        screen_sidecar["_solstone_processing"]["handler"],
        "describe"
    );
    assert_eq!(screen_sidecar["_solstone_processing"]["state"], "failed");
    assert_eq!(
        screen_sidecar["_solstone_processing"]["reason_code"],
        "corrupt_input"
    );

    assert!(
        !context.poison_marker.exists(),
        "native sense batch reached a poisoned interpreter: {}",
        fs::read_to_string(context.poison_marker).unwrap_or_default()
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
fn native_install_verbs_help_matches_exported_cli_constants_through_dispatcher() {
    let harness = Harness::new();
    let context = harness.context();
    for (token, expected_help) in [
        ("check", CHECK_HELP),
        ("install-models", INSTALL_MODELS_HELP),
        ("install-provider", INSTALL_PROVIDER_HELP),
    ] {
        for argv in [["--help"], ["-h"]] {
            let output = run_dispatcher_with_output(&context, token, &argv)
                .expect("run native help through journal dispatcher");
            assert_eq!(output.status.code(), Some(0), "{token} {argv:?}");
            assert_eq!(output.stderr, b"", "{token} {argv:?}");
            assert_eq!(output.stdout, expected_help.as_bytes(), "{token} {argv:?}");
            assert!(!context.poison_marker.exists(), "{token} {argv:?}");
        }
    }
}

#[test]
fn native_install_verbs_malformed_argv_use_exported_usage_through_dispatcher() {
    let harness = Harness::new();
    let context = harness.context();
    for (token, expected_usage) in [
        ("check", CHECK_USAGE),
        ("install-models", INSTALL_MODELS_USAGE),
        ("install-provider", INSTALL_PROVIDER_USAGE),
    ] {
        let output = run_dispatcher_with_output(&context, token, &["--nonsense"])
            .expect("run malformed native argv through journal dispatcher");
        assert_eq!(output.status.code(), Some(2), "{token}");
        assert!(
            output.stderr.starts_with(expected_usage.as_bytes()),
            "{token}: stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            !String::from_utf8_lossy(&output.stderr).contains("solstone-core"),
            "{token}: stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!context.poison_marker.exists(), "{token}");
    }
}

#[test]
fn native_config_branches_remain_poison_clean_through_dispatcher() {
    let harness = Harness::new();
    let context = harness.context();
    prove_poison_interpreters_live(&context);

    let show = run_dispatcher_with_output(&context, "config", &["show"])
        .expect("run native config show through dispatcher");
    assert_eq!(show.status.code(), Some(0));
    assert!(show.stderr.is_empty());
    assert!(
        !context.poison_marker.exists(),
        "config show reached poison"
    );

    let missing_parent = context.home.join("missing-parent/target");
    let missing_parent_arg = missing_parent.display().to_string();
    let refusal = run_dispatcher_with_output(
        &context,
        "config",
        &["journal", &missing_parent_arg, "--move", "--yes"],
    )
    .expect("run native config refusal through dispatcher");
    assert_eq!(refusal.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&refusal.stderr).contains("move target parent does not exist"));
    assert!(
        !context.poison_marker.exists(),
        "config refusal reached poison"
    );

    let aliases = context.home.join(".local/bin");
    fs::create_dir_all(&aliases).expect("create wrapper directory");
    let current = context.home.join("current-journal");
    let solstone = aliases.join("solstone");
    fs::write(
        &solstone,
        format!(
            "#!/bin/bash\n# solstone — managed by 'journal config'. Edits will be overwritten.\n# managed-version: 7\n: \"${{SOLSTONE_JOURNAL:={}}}\"\nexport SOLSTONE_JOURNAL\nSOL_BIN='/native/solstone'\n# Warn when pyproject.toml or uv.lock is newer than .installed.\n# Skipped silently if .installed is absent.\nREPO_ROOT=\"${{SOL_BIN%/.venv/bin/solstone}}\"\nif [ -f \"$REPO_ROOT/.installed\" ]; then\n  if [ \"$REPO_ROOT/pyproject.toml\" -nt \"$REPO_ROOT/.installed\" ] \\\n     || [ \"$REPO_ROOT/uv.lock\" -nt \"$REPO_ROOT/.installed\" ]; then\n    echo \"solstone: WARNING — venv is stale (pyproject.toml or uv.lock changed since last install). Run: cd $REPO_ROOT && make install\" >&2\n  fi\nfi\nif [ ! -x \"$SOL_BIN\" ]; then\n    printf 'solstone: venv binary missing or not executable: %s\\n' \"$SOL_BIN\" >&2\n    exit 127\nfi\nexec \"$SOL_BIN\" \"$@\"\n",
            current.display()
        ),
    )
    .expect("write managed solstone wrapper");

    let plan_target = context.home.join("planned-journal");
    let plan_target_arg = plan_target.display().to_string();
    let plan = run_dispatcher_with_output(
        &context,
        "config",
        &["journal", &plan_target_arg, "--switch", "--dry-run"],
    )
    .expect("run native config plan through dispatcher");
    assert_eq!(plan.status.code(), Some(0));
    assert!(
        String::from_utf8_lossy(&plan.stdout).contains("dry-run: yes; nothing will be changed")
    );
    assert!(
        !context.poison_marker.exists(),
        "config plan reached poison"
    );

    let no_service_target = context.home.join("no-service-journal");
    let no_service_manifest = no_service_target.join("health/setup-state.json");
    fs::create_dir_all(no_service_manifest.parent().expect("setup-state parent"))
        .expect("create setup-state parent");
    fs::write(
        &no_service_manifest,
        br#"{"schema_version":1,"started_at":"2026-01-01T00:00:00Z","completed_at":null,"mode":"non_interactive","args_resolved":{},"steps":[]}"#,
    )
    .expect("write legacy setup-state evidence");
    let no_service_target_arg = no_service_target.display().to_string();
    let no_service = run_dispatcher_with_output(
        &context,
        "config",
        &["journal", &no_service_target_arg, "--switch", "--yes"],
    )
    .expect("run native config no-service branch through dispatcher");
    assert_eq!(
        no_service.status.code(),
        Some(0),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&no_service.stdout),
        String::from_utf8_lossy(&no_service.stderr)
    );
    assert_eq!(
        no_service.stdout,
        b"service not installed; wrapper updated.\n"
    );
    assert!(
        !context.poison_marker.exists(),
        "config no-service branch reached poison"
    );

    #[cfg(target_os = "macos")]
    let service_unit = context
        .home
        .join("Library/LaunchAgents/org.solpbc.solstone.plist");
    #[cfg(not(target_os = "macos"))]
    let service_unit = context.home.join(".config/systemd/user/solstone.service");
    fs::create_dir_all(service_unit.parent().expect("service parent"))
        .expect("create service parent");
    fs::write(&service_unit, "unit").expect("write service unit");
    let stopped_target = context.home.join("stopped-service-journal");
    let stopped_target_arg = stopped_target.display().to_string();
    let stopped = run_dispatcher_with_output(
        &context,
        "config",
        &["journal", &stopped_target_arg, "--switch", "--yes"],
    )
    .expect("run native config stopped-service branch through dispatcher");
    assert_eq!(stopped.status.code(), Some(0));
    assert_eq!(
        stopped.stdout,
        b"service installed but not running; wrapper updated.\n"
    );
    assert!(
        !context.poison_marker.exists(),
        "config stopped-service branch reached poison"
    );
}

#[test]
fn native_check_json_routes_without_python_and_has_contractual_top_level_keys() {
    let harness = Harness::new();
    let context = harness.context();
    let output = run_dispatcher_with_output(&context, "check", &["--json"])
        .expect("run native check JSON through journal dispatcher");

    assert!(output.stderr.is_empty());
    assert!(!context.poison_marker.exists());
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("native check JSON output");
    let actual_keys = json
        .as_object()
        .expect("native check JSON object")
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    // solstone-core-check/src/lib.rs and fixtures/check_corpus.json, exercised by
    // check_corpus.rs, define this schema. This guard proves routing, not
    // host-dependent check content or exit severity.
    let expected_keys = CHECK_JSON_TOP_LEVEL_KEYS
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    assert_eq!(actual_keys, expected_keys);
}

/// The registered probe for `install-provider` is a bad-argv probe, so it exits
/// inside argument parsing and proves nothing about the verb's own body. These
/// three argv sets each run the real body -- the supervisor gate reads the
/// journal's recorded Convey port, and the name arm runs past it -- and none of
/// them can reach a fetch, so the guard stays hermetic.
#[test]
fn native_install_provider_bodies_never_reach_a_poisoned_interpreter() {
    let harness = Harness::new();
    let context = harness.context();
    prove_poison_interpreters_live(&context);
    let _ = fs::remove_file(context.poison_marker);

    // Stack down, interactive: the owner line on stderr and exit 1.
    let down =
        run_dispatcher_with_output_and_environment(&context, "install-provider", &["local"], &[])
            .expect("run native install-provider with the stack down");
    assert_eq!(down.status.code(), Some(1));
    assert_eq!(
        down.stderr,
        b"journal isn't running. start it with 'journal up' and retry.\n"
    );
    assert!(down.stdout.is_empty());
    assert!(!context.poison_marker.exists(), "supervisor gate");

    // Stack down, supervisor-spawned: exit 75 and not one byte of stderr.
    let spawned = run_dispatcher_with_output_and_environment(
        &context,
        "install-provider",
        &["parakeet"],
        &[("SOL_SUPERVISOR_SPAWNED", "1")],
    )
    .expect("run native install-provider as a spawned child");
    assert_eq!(spawned.status.code(), Some(75));
    assert!(spawned.stderr.is_empty(), "{:?}", spawned.stderr);
    assert!(!context.poison_marker.exists(), "spawned refusal");

    // Past the gate, into the name arm: exit 2 with the reference's quoting.
    let unknown = run_dispatcher_with_output_and_environment(
        &context,
        "install-provider",
        &["bogus"],
        &[("SOL_SKIP_SUPERVISOR_CHECK", "1")],
    )
    .expect("run native install-provider past the gate");
    assert_eq!(unknown.status.code(), Some(2));
    assert_eq!(
        unknown.stderr,
        b"unsupported provider 'bogus'; supported: local, parakeet\n"
    );
    assert!(unknown.stdout.is_empty());
    assert!(!context.poison_marker.exists(), "unknown provider arm");
}

#[test]
fn native_backfill_commit_dispatches_without_python_and_is_idempotent() {
    let harness = Harness::new();
    let context = harness.context();
    prove_poison_interpreters_live(&context);

    fs::create_dir_all(context.home).expect("create home directory");
    let segment = context.journal.join("chronicle/20990101/090000_300");
    fs::create_dir_all(&segment).expect("create future journal segment");
    fs::write(segment.join("audio.flac"), b"audio").expect("write audio");
    let sidecar = segment.join("audio.jsonl");
    fs::write(&sidecar, b"{\"raw\":\"audio.flac\"}\n").expect("write sidecar");

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
        bytes: Arc::<[u8]>::from([]),
    };
    let poison_marker = temp.path.join("poison-not-run");
    let probe_stderr = temp.path.join("probe-stderr-not-run");
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
        probe_stderr: &probe_stderr,
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
        stderr_anchor: None,
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
fn mandatory_native_sibling_failures_are_operator_errors() {
    for (token, binary) in [
        ("check", "solstone-core"),
        ("health", "solstone-core"),
        ("top", "solstone-core"),
        ("service", "solstone-core"),
        ("up", "solstone-core"),
        ("down", "solstone-core"),
        ("depict", "solstone-core-depict"),
    ] {
        let harness = Harness::new();
        let context = harness.context();
        let sibling = context.sibling_dir.join(binary);
        fs::remove_file(&sibling).expect("remove mandatory sibling");
        let output = run_dispatcher_with_output(&context, token, &["--nonsense"])
            .expect("dispatch with missing sibling");
        assert_eq!(output.status.code(), Some(70), "{token}: missing");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("native-helper-missing:"),
            "{token}: missing stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            !context.poison_marker.exists(),
            "{token}: no Python fallback"
        );
    }

    for (token, binary) in [
        ("check", "solstone-core"),
        ("health", "solstone-core"),
        ("top", "solstone-core"),
        ("service", "solstone-core"),
        ("up", "solstone-core"),
        ("down", "solstone-core"),
        ("depict", "solstone-core-depict"),
    ] {
        let harness = Harness::new();
        let context = harness.context();
        let sibling = context.sibling_dir.join(binary);
        fs::set_permissions(&sibling, fs::Permissions::from_mode(0o644))
            .expect("make mandatory sibling non-executable");
        let output = run_dispatcher_with_output(&context, token, &["--nonsense"])
            .expect("dispatch with non-executable sibling");
        assert_eq!(output.status.code(), Some(70), "{token}: non-executable");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("native-helper-not-executable:"),
            "{token}: non-executable stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            !context.poison_marker.exists(),
            "{token}: no Python fallback"
        );
    }
}

#[test]
fn native_top_registered_probe_has_exact_clean_parser_output() {
    let harness = Harness::new();
    let context = harness.context();
    prove_poison_interpreters_live(&context);
    let probe = probe_for("top").expect("top must have a registered native probe");
    assert_eq!(probe.argv, &["--nonsense"]);
    assert_eq!(probe.expected_exit, 2);
    assert_eq!(probe.stderr_anchor, Some(TOP_USAGE.as_bytes()));

    let output =
        run_dispatcher_with_bounded_output(&context, probe.token, probe.argv, PROBE_TIMEOUT)
            .expect("dispatch native top parser probe")
            .expect("native top parser probe must complete before the deadline");
    assert_eq!(output.status.code(), Some(probe.expected_exit));
    assert!(output.stdout.is_empty());
    assert_eq!(
        output.stderr,
        b"usage: journal top [-h] [-v] [-d]\njournal top: error: invalid arguments\n"
    );
    assert!(!context.poison_marker.exists());
}

#[test]
fn native_service_registered_probes_have_exact_clean_parser_output() {
    let harness = Harness::new();
    let context = harness.context();
    prove_poison_interpreters_live(&context);

    for token in ["service", "up", "down"] {
        let probe = probe_for(token).expect("service route must have a registered native probe");
        assert_eq!(probe.argv, &["--nonsense"]);
        assert_eq!(probe.expected_exit, 1);
        assert_eq!(probe.stderr_anchor, Some(SERVICE_UNKNOWN_ANCHOR));

        let output = run_dispatcher_with_bounded_output(&context, token, probe.argv, PROBE_TIMEOUT)
            .expect("dispatch native service parser probe")
            .expect("native service parser probe must complete before the deadline");
        assert_eq!(output.status.code(), Some(probe.expected_exit), "{token}");
        assert!(output.stdout.is_empty(), "{token}");
        assert_eq!(output.stderr, SERVICE_UNKNOWN_ANCHOR, "{token}");
        assert!(!context.poison_marker.exists(), "{token}");
    }
}

#[test]
fn native_service_dispatch_reaches_stable_real_bodies_without_python() {
    let harness = Harness::new();
    let context = harness.context();
    prove_poison_interpreters_live(&context);
    fs::create_dir_all(context.home).expect("create isolated service home");
    fs::create_dir_all(context.journal).expect("create isolated service journal");

    // `down` deliberately consults the fixed OS service manager, whose live
    // registration is outside this process fixture. Its exact parser path is
    // asserted above and its stop decision table is covered in the core crate.
    for (token, argv, expected_stdout, expected_stderr) in [
        (
            "service",
            &["status"][..],
            b"service: not installed\nrun 'journal setup' or 'journal service install' to install it.\n"
                .as_slice(),
            b"".as_slice(),
        ),
        (
            "up",
            &[][..],
            b"".as_slice(),
            b"error: service not installed. run 'journal service install' first.\n".as_slice(),
        ),
    ] {
        let output = run_dispatcher_with_bounded_output(&context, token, argv, PROBE_TIMEOUT)
            .expect("dispatch native service body")
            .expect("native service body must complete before the deadline");
        assert_eq!(output.status.code(), Some(1), "{token}");
        assert_eq!(output.stdout, expected_stdout, "{token}");
        assert_eq!(output.stderr, expected_stderr, "{token}");
        assert!(!context.poison_marker.exists(), "{token}");
    }
}

#[test]
fn native_top_dispatch_reaches_the_real_non_tty_body_without_python() {
    let harness = Harness::new();
    let context = harness.context();
    prove_poison_interpreters_live(&context);

    let output = run_dispatcher_with_bounded_output(&context, "top", &[], PROBE_TIMEOUT)
        .expect("dispatch native top body")
        .expect("native top body must complete before the deadline");
    assert_eq!(output.status.code(), Some(69));
    assert!(output.stdout.is_empty());
    assert_eq!(
        output.stderr,
        b"journal top: terminal failure: stdin is not a terminal\n"
    );
    assert!(!context.poison_marker.exists());
}

#[test]
fn native_health_dispatch_reaches_both_real_bodies_without_python() {
    let harness = Harness::new();
    let context = harness.context();
    prove_poison_interpreters_live(&context);
    fs::create_dir_all(context.journal).expect("create isolated health journal");

    let health =
        run_dispatcher_with_output(&context, "health", &[]).expect("dispatch native health body");
    assert_eq!(health.status.code(), Some(1));
    assert_eq!(
        health.stdout,
        concat!(
            "Sound tagging is degraded because its CED assets are unavailable. ",
            "Transcription will continue. Use `journal install-models` to check or repair the CED assets. ",
            "If the signed CED app payload is unavailable on Windows, reinstall the journal app.\n",
            "Object detection is degraded because its RF-DETR assets are unavailable. ",
            "Screen descriptions will continue. Use `journal install-models` to check or repair the RF-DETR assets.\n",
        )
        .as_bytes()
    );
    assert_eq!(
        health.stderr,
        format!(
            "Cannot connect: callosum socket not found at {}/health/callosum.sock\n",
            context.journal.display()
        )
        .as_bytes()
    );
    assert!(!context.poison_marker.exists());

    let logs = run_dispatcher_with_output(&context, "health", &["logs", "-f"])
        .expect("dispatch native health logs body");
    assert_eq!(logs.status.code(), Some(0));
    assert!(logs.stdout.is_empty());
    assert_eq!(logs.stderr, b"No log files found.\n");
    assert!(!context.poison_marker.exists());
}

#[test]
fn describe_dispatch_writes_the_native_thinking_artifact_without_python() {
    let harness = Harness::new();
    let context = harness.context();
    let session_stub_source =
        locate_workspace_binary("solstone-core-describe", "solstone-describe-session-stub");
    let session_stub = context.sibling_dir.join("solstone-describe-session-stub");
    copy_executable(&session_stub_source, &session_stub);
    let corpus = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/describe_corpus/single_frame_vp8_screen.webm");
    fs::create_dir_all(context.journal).expect("create unique journal root");
    let video = context.journal.join("screen.webm");
    fs::copy(corpus, &video).expect("copy corpus screencast");

    let output = Command::new(context.dispatcher)
        .args(["describe"])
        .arg(&video)
        .args(["-j", "2", "-d", "-v"])
        .env("POISON_MARKER", context.poison_marker)
        .env("HOME", context.home)
        .env("SOLSTONE_JOURNAL", context.journal)
        .env("SOLSTONE_DESCRIBE_GENERATE_WIRE", &session_stub)
        .env("SOLSTONE_DESCRIBE_SESSION_STUB_MODE", "generated")
        .env("PATH", context.sibling_dir)
        .output()
        .expect("run describe through native dispatcher");
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !context.poison_marker.exists(),
        "native describe reached a poisoned interpreter"
    );

    let artifact = video.with_extension("jsonl");
    let header: serde_json::Value = fs::read_to_string(&artifact)
        .expect("read native describe artifact")
        .lines()
        .next()
        .expect("artifact header")
        .parse()
        .expect("artifact header JSON");
    assert!(header["_solstone_thinking"]["model"].is_string());
    assert!(header["_solstone_thinking"].get("provider").is_none());
}

#[test]
fn unregistered_native_probe_is_a_guard_verdict_without_a_spawn() {
    // Coverage restoration, not fix verification: ProbeUnregistered returns from
    // verdict_for's first statement, so checked-empty and marker-absent cannot
    // fail today. They guard a future reordering that moves lookup below a spawn.
    let temp = TempDir::new("unregistered-probe");
    let dispatcher = temp.path.join("dispatcher-not-run");
    let empty_artifacts = BTreeMap::new();
    let dispatcher_artifact = Artifact {
        bytes: Arc::<[u8]>::from([]),
    };
    let poison_marker = temp.path.join("poison-not-run");
    let probe_stderr = temp.path.join("probe-stderr-not-run");
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
        probe_stderr: &probe_stderr,
    };
    let synthetic = NativeProcessSpec {
        token: "synthetic-unregistered-native",
        binary: "missing-native-helper",
        preset_argv: &[],
    };
    let mut checked = BTreeSet::new();
    let verdict = verdict_for(
        &synthetic,
        probe_for(synthetic.token),
        &context,
        &mut checked,
    );

    assert!(
        probe_for("grab").is_some(),
        "probe lookup seam must resolve real tokens"
    );
    assert!(
        matches!(
            verdict,
            Verdict::ProbeUnregistered {
                token: "synthetic-unregistered-native"
            }
        ),
        "synthetic-unregistered-native: expected ProbeUnregistered, got {verdict:?}"
    );
    assert!(checked.is_empty());
    assert!(!poison_marker.exists());
}

#[test]
fn timed_out_native_probe_is_a_guard_verdict() {
    // Use a real token in a fresh harness: synthetic tokens panic in production
    // dispatch, and changing a shared harness sibling would poison later probes.
    let harness = Harness::new();
    let context = harness.context();
    let sibling = context.sibling_dir.join("solstone-core");
    fs::write(&sibling, "#!/bin/sh\n/bin/sleep 1\n").expect("write slow native sibling");
    make_executable(&sibling);
    let spec = NATIVE_PROCESS_SPECS
        .iter()
        .find(|spec| spec.token == "check")
        .expect("check native process spec");
    let probe = probe_for("check").expect("check native process probe");
    let mut checked = BTreeSet::new();

    let verdict = verdict_for_with_timeout(
        spec,
        Some(probe),
        &context,
        &mut checked,
        Duration::from_millis(100),
    );

    assert!(
        matches!(verdict, Verdict::TimedOut { token: "check" }),
        "expected TimedOut, got {verdict:?}"
    );
    assert_eq!(checked, BTreeSet::from(["check"]));
    assert!(!context.poison_marker.exists());
}

#[test]
fn stderr_anchor_mismatch_is_a_guard_verdict() {
    let temp = TempDir::new("stderr-anchor");
    let sibling_dir = temp.path.join("bin");
    fs::create_dir(&sibling_dir).expect("create sibling directory");
    let dispatcher = temp.path.join("dispatcher");
    fs::write(
        &dispatcher,
        "#!/bin/sh\nprintf 'wrong stderr\\n' >&2\nexit 2\n",
    )
    .expect("write dispatcher");
    make_executable(&dispatcher);
    let sibling = sibling_dir.join("synthetic-native");
    fs::write(&sibling, "#!/bin/sh\nexit 0\n").expect("write sibling");
    make_executable(&sibling);
    let dispatcher_artifact = artifact(&dispatcher);
    let mut sibling_artifacts = BTreeMap::new();
    sibling_artifacts.insert("synthetic-native", artifact(&sibling));
    let poison_marker = temp.path.join("poison-not-run");
    let probe_stderr = temp.path.join("probe-stderr");
    let home = temp.path.join("home");
    let journal = temp.path.join("journal");
    let context = VerdictContext {
        dispatcher: &dispatcher,
        sibling_dir: &sibling_dir,
        dispatcher_artifact: &dispatcher_artifact,
        sibling_artifacts: &sibling_artifacts,
        home: &home,
        journal: &journal,
        poison_marker: &poison_marker,
        probe_stderr: &probe_stderr,
    };
    let synthetic = NativeProcessSpec {
        token: "synthetic-stderr-anchor",
        binary: "synthetic-native",
        preset_argv: &[],
    };
    let probe = Probe {
        token: "synthetic-stderr-anchor",
        argv: &[],
        expected_exit: 2,
        stderr_anchor: Some(b"expected stderr\n"),
    };
    let mut checked = BTreeSet::new();
    let verdict = verdict_for(&synthetic, Some(&probe), &context, &mut checked);

    match verdict {
        Verdict::StderrAnchorMismatch {
            token,
            expected,
            actual,
        } => {
            assert_eq!(token, "synthetic-stderr-anchor");
            assert_eq!(expected, b"expected stderr\n");
            assert_eq!(actual, b"wrong stderr\n");
        }
        other => panic!("expected StderrAnchorMismatch, got {other:?}"),
    }
    assert_eq!(checked, BTreeSet::from(["synthetic-stderr-anchor"]));
    assert!(!poison_marker.exists());
}

#[test]
fn native_start_help_uses_start_program_name() {
    let harness = Harness::new();
    let context = harness.context();
    let output = run_dispatcher_with_output(&context, "start", &["--help"])
        .expect("run start help through dispatcher");

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stderr, b"");
    assert!(output.stdout.starts_with(b"usage: journal start"));
    assert!(!context.poison_marker.exists());
}

// --- `journal think` dispatches natively, and the talent runtime it reaches
// --- loads no Python module.
//
// Three assertions, because no one of them is sufficient:
//
//   1. the census row      -- `think` is registered for native dispatch;
//   2. the runtime         -- a real run-mode argv enters a run mode and reaches
//                             the talent plane with no interpreter touched;
//   3. the plugin host     -- no crate outside the dispatcher resolves an
//                             interpreter, which is what "no Python module is
//                             loaded or executed by the talent runtime" means
//                             mechanically.
//
// (1) alone leaves an unproven execution boundary: the poison-liveness
// probe for a run-mode verb exits at ARGUMENT PARSING, before any spawn, so a
// registration can go green over a run path that still execs an interpreter.
// (2) is red until a run mode exists; (3) is red until the talent runtime is
// native. Do not resolve any of them by relaxing the others.

#[test]
fn think_is_registered_for_native_dispatch() {
    let native_tokens = NATIVE_PROCESS_SPECS
        .iter()
        .map(|spec| spec.token)
        .collect::<BTreeSet<_>>();
    let missing = THINK_REQUIRED_NATIVE_TOKENS
        .iter()
        .copied()
        .filter(|token| !native_tokens.contains(token))
        .collect::<Vec<_>>();

    assert!(
        missing.is_empty(),
        "required think native process tokens are missing: {missing:?}"
    );
}

/// A real `journal think` run mode, driven through the real dispatcher under
/// both interpreter poisons, must enter the run mode and reach the talent
/// plane without touching an interpreter.
///
/// `--cadence` is the cheapest mode that proves it: the reference creates its
/// run-log sidecar only AFTER mode selection and only when at least one cadence
/// talent is configured, and it names every considered talent in that log. So a
/// log at `chronicle/<day>/health/<ref>_cadence.jsonl` carrying `run.start`
/// with `"mode":"cadence"` and naming both seeded talents cannot be produced by
/// argument parsing, by the unavailable-run boundary, or by an empty run.
///
/// Both seeded talents declare a frontmatter `hook` object, so reaching them is
/// reaching the native talent runtime surface. No model endpoint is configured and
/// the journal has no completed work, so the run resolves them and does not
/// call a provider -- the test is hermetic and makes no network claim.
#[test]
fn native_think_cadence_run_reaches_the_talent_plane_without_python() {
    let harness = Harness::new();
    let context = harness.context();
    prove_poison_interpreters_live(&context);

    // A fake checkout beside the sibling directory, in the shape the resolver
    // recognises: `pyproject.toml` and `.git` at the root, and the shipped
    // payload under `core/payload` carrying the three layout anchors. The
    // payload is deliberately NOT in the package tree, so seeding
    // `<root>/solstone/talent` here would leave the resolver with nothing.
    let checkout_root = context
        .sibling_dir
        .parent()
        .expect("sibling directory parent");
    let install_root = seed_fixture_checkout(checkout_root);
    let talent_root = install_root.join("solstone/talent");
    let apps_root = install_root.join("solstone/apps");
    fs::create_dir_all(&talent_root).expect("create fixture talent root");
    fs::create_dir_all(&apps_root).expect("create fixture apps root");

    let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .expect("repository root")
        .to_path_buf();
    for name in ["pulse", "steward"] {
        let source = repository.join(format!("core/payload/solstone/talent/{name}.md"));
        let body = fs::read(&source)
            .unwrap_or_else(|error| panic!("read shipped talent {}: {error}", source.display()));
        // Both shipped cadence talents declare a hook object; if that ever
        // stops being true this fixture stops testing what it claims to.
        assert!(
            String::from_utf8_lossy(&body).contains("\"hook\""),
            "{name}: shipped cadence talent no longer declares a hook"
        );
        fs::write(talent_root.join(format!("{name}.md")), &body).expect("seed fixture talent");
    }

    fs::create_dir_all(context.journal).expect("create cadence journal root");
    let output = run_dispatcher_with_output_and_environment(
        &context,
        "think",
        &["--cadence"],
        &[("SOL_SKIP_SUPERVISOR_CHECK", "1")],
    )
    .expect("run native think cadence through the dispatcher");

    assert!(
        !context.poison_marker.exists(),
        "journal think --cadence reached a poisoned interpreter; exit={:?} stderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_ne!(
        output.status.code(),
        Some(69),
        "journal think --cadence is still refusing at the unavailable-run boundary"
    );

    let logs = cadence_run_logs(context.journal);
    assert_eq!(
        logs.len(),
        1,
        "expected exactly one cadence run log under {}; stderr={}",
        context.journal.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    let recorded = fs::read_to_string(&logs[0]).expect("read cadence run log");
    let start: serde_json::Value = recorded
        .lines()
        .map(|line| serde_json::from_str(line).expect("parse canonical run-log record"))
        .find(|event: &serde_json::Value| event.get("event").is_some())
        .expect("canonical run log contains an event after its admission record");
    assert_eq!(start["event"], "run.start");
    assert_eq!(start["mode"], "cadence");
    for name in ["pulse", "steward"] {
        assert!(
            recorded
                .lines()
                .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
                .any(|event| event["name"] == name),
            "{name}: cadence run never reached the talent plane; log={recorded}"
        );
    }
}

/// Collect canonical JSONL operational logs under `chronicle/<day>/health/`.
fn cadence_run_logs(journal: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(days) = fs::read_dir(journal.join("chronicle")) else {
        return found;
    };
    for day in days.filter_map(Result::ok) {
        let Ok(entries) = fs::read_dir(day.path().join("health")) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let leaf = entry.file_name();
            let leaf = leaf.to_string_lossy();
            if leaf.starts_with("oplog--") && leaf.ends_with(".jsonl") {
                found.push(entry.path());
            }
        }
    }
    found.sort();
    found
}

/// Make `root` look like a source checkout to the installation resolver and
/// return the payload root the resolver will hand back.
fn seed_fixture_checkout(root: &Path) -> PathBuf {
    fs::create_dir_all(root.join(".git")).expect("create fixture checkout marker");
    fs::write(root.join("pyproject.toml"), b"").expect("create fixture project marker");
    let payload = root.join(solstone_core_journal::CHECKOUT_PAYLOAD_ROOT);
    for anchor in [
        solstone_core_journal::LAYOUT_BUNDLE_ANCHOR,
        solstone_core_journal::LAYOUT_LAYOUT_ANCHOR,
        solstone_core_journal::LAYOUT_TEMPLATE_ANCHOR,
    ] {
        let path = payload.join(anchor);
        fs::create_dir_all(path.parent().expect("anchor parent")).expect("create anchor directory");
        fs::write(&path, anchor).expect("write fixture layout anchor");
    }
    payload
}

fn seed_think_install(context: &VerdictContext<'_>, talents: &[(&str, &str)]) {
    let install_root = seed_fixture_checkout(
        context
            .sibling_dir
            .parent()
            .expect("sibling directory parent"),
    );
    let talent_root = install_root.join("solstone/talent");
    fs::create_dir_all(&talent_root).expect("create fixture talent root");
    fs::create_dir_all(install_root.join("solstone/apps")).expect("create fixture apps root");
    for (name, body) in talents {
        fs::write(talent_root.join(format!("{name}.md")), body).expect("seed think talent");
    }
}

fn think_mode_events(journal: &Path, mode: &str) -> Vec<serde_json::Value> {
    let mut logs = Vec::new();
    let chronicle = journal.join("chronicle");
    let Ok(days) = fs::read_dir(chronicle) else {
        return logs;
    };
    for day in days.filter_map(Result::ok) {
        let Ok(entries) = fs::read_dir(day.path().join("health")) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let leaf = entry.file_name();
            let leaf = leaf.to_string_lossy();
            if leaf.starts_with("oplog--") && leaf.ends_with(".jsonl") {
                logs.extend(
                    fs::read_to_string(entry.path())
                        .expect("read think mode sidecar")
                        .lines()
                        .map(|line| serde_json::from_str(line).expect("parse think sidecar event"))
                        .filter(|event: &serde_json::Value| event["mode"] == mode),
                );
            }
        }
    }
    logs
}

fn run_native_think_mode(context: &VerdictContext<'_>, argv: &[&str]) -> std::process::Output {
    let output = run_dispatcher_with_output_and_environment(
        context,
        "think",
        argv,
        &[("SOL_SKIP_SUPERVISOR_CHECK", "1")],
    )
    .expect("run native think mode through dispatcher");
    assert!(
        !context.poison_marker.exists(),
        "journal think {:?} reached a poisoned interpreter; exit={:?} stderr={}",
        argv,
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_ne!(
        output.status.code(),
        Some(69),
        "think mode still unavailable"
    );
    output
}

/// Built-binary AC1 witnesses.  These are intentionally separate from the
/// recorder-seam unit tests: the dispatcher, native sibling, and run-log path
/// are the boundary under proof here.
#[test]
fn native_think_all_modes_produce_their_falsifying_observables_without_python() {
    // The restored native whole-day operation records its Sense batch before
    // segment repair and scheduled daily work.
    let daily = Harness::new();
    let context = daily.context();
    prove_poison_interpreters_live(&context);
    seed_think_install(
        &context,
        &[(
            "daily_probe",
            "{\n\"type\":\"generate\",\"schedule\":\"daily\",\"priority\":1,\"output\":\"md\"\n}\n",
        )],
    );
    run_native_think_mode(&context, &["--day", "20260101"]);
    assert!(
        think_mode_events(context.journal, "daily")
            .iter()
            .any(|event| { event["event"] == "phase.start" && event["phase"] == "sense_batch" })
    );

    // Source-derived, not measured: thinking.py:2559-2957 processes a
    // priority group only in the weekly run; the sidecar retains that group.
    let weekly = Harness::new();
    let context = weekly.context();
    prove_poison_interpreters_live(&context);
    seed_think_install(
        &context,
        &[(
            "weekly_probe",
            "{\n\"type\":\"generate\",\"schedule\":\"weekly\",\"priority\":91,\"output\":\"md\"\n}\n",
        )],
    );
    run_native_think_mode(&context, &["--day", "20260101", "--weekly"]);
    assert!(
        think_mode_events(context.journal, "weekly")
            .iter()
            .any(|event| event["event"] == "group.start" && event["priority"] == 91)
    );

    // Source-derived, not measured: thinking.py:2994-3001 records this
    // cadence-only no-new-work skip instead of silently omitting the talent.
    let cadence = Harness::new();
    let context = cadence.context();
    prove_poison_interpreters_live(&context);
    seed_think_install(
        &context,
        &[(
            "cadence_probe",
            "{\n\"type\":\"generate\",\"schedule\":\"cadence\",\"priority\":45,\"output\":\"json\"\n}\n",
        )],
    );
    run_native_think_mode(&context, &["--day", "20260101", "--cadence"]);
    assert!(
        think_mode_events(context.journal, "cadence")
            .iter()
            .any(|event| {
                event["event"] == "talent.skip"
                    && event["mode"] == "cadence"
                    && event["name"] == "cadence_probe"
                    && event["reason"] == "no_new_work"
            })
    );

    // Source-derived, not measured: thinking.py:3084-3499 carries the
    // activity id in activity-mode start records; other modes do not.
    let activity = Harness::new();
    let context = activity.context();
    prove_poison_interpreters_live(&context);
    seed_think_install(
        &context,
        &[(
            "activity_probe",
            "{\n\"type\":\"generate\",\"schedule\":\"activity\",\"priority\":1,\"output\":\"md\",\"activities\":[\"meeting\"]\n}\n",
        )],
    );
    let record = context
        .journal
        .join("facets/work/activities/20260101.jsonl");
    fs::create_dir_all(record.parent().expect("activity parent")).expect("create activity parent");
    fs::write(
        record,
        r#"{"id":"meeting_1","activity":"meeting","segments":["090000_60"],"source":"user","level_avg":1.0}"#,
    )
    .expect("seed activity");
    run_native_think_mode(
        &context,
        &[
            "--day",
            "20260101",
            "--activity",
            "meeting_1",
            "--facet",
            "work",
        ],
    );
    assert!(
        think_mode_events(context.journal, "activity")
            .iter()
            .any(|event| event["event"] == "started" && event["activity"] == "meeting_1")
    );

    // Source-derived, not measured: thinking.py:3500-3712 emits a flush-mode
    // sidecar only after filtering `hook.flush` eligible talents.
    let flush = Harness::new();
    let context = flush.context();
    prove_poison_interpreters_live(&context);
    seed_think_install(
        &context,
        &[(
            "flush_probe",
            "{\n\"type\":\"generate\",\"schedule\":\"segment\",\"priority\":1,\"output\":\"md\",\"hook\":{\"flush\":true}\n}\n",
        )],
    );
    run_native_think_mode(
        &context,
        &["--day", "20260101", "--segment", "090000_60", "--flush"],
    );
    assert!(
        think_mode_events(context.journal, "flush")
            .iter()
            .any(|event| event["event"] == "started" && event["segment"] == "090000_60")
    );

    // Source-derived, not measured: thinking.py:1536-1584 writes the idle
    // Sense artifact for an input-free segment, unique to segment sensing.
    let segment = Harness::new();
    let context = segment.context();
    prove_poison_interpreters_live(&context);
    seed_think_install(
        &context,
        &[(
            "sense",
            "{\n\"type\":\"generate\",\"schedule\":\"segment\",\"priority\":1,\"output\":\"json\",\"load\":{\"transcripts\":true}\n}\n",
        )],
    );
    fs::create_dir_all(context.journal.join("chronicle/20260101/090000_60"))
        .expect("seed empty segment");
    run_native_think_mode(&context, &["--day", "20260101", "--segment", "090000_60"]);
    assert!(
        context
            .journal
            .join("chronicle/20260101/090000_60/talents/sense.json")
            .is_file()
    );
}

fn rust_sources_under(crates: &Path) -> Vec<PathBuf> {
    let mut sources = Vec::new();
    let mut pending = match fs::read_dir(crates) {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .map(|entry| entry.path().join("src"))
            .filter(|path| path.is_dir())
            .collect::<Vec<_>>(),
        Err(error) => panic!("read {}: {error}", crates.display()),
    };
    while let Some(directory) = pending.pop() {
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|kind| kind == "rs") {
                sources.push(path);
            }
        }
    }
    sources.sort();
    sources
}

#[test]
fn process_tokens_are_native_cutovers() {
    let native_tokens = NATIVE_PROCESS_SPECS
        .iter()
        .map(|spec| spec.token)
        .collect::<BTreeSet<_>>();
    for token in ["grab", "transfer", "transcribe"] {
        assert!(
            native_tokens.contains(token),
            "{token}: native process dispatch is required"
        );
    }
    assert!(
        !native_tokens.contains("export"),
        "export must be intercepted by journal-cli rather than native-process dispatch"
    );
}
