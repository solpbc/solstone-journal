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
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[allow(dead_code)]
#[path = "../../solstone-core-journal-cli/src/processes.rs"]
mod production_processes;

use production_processes::{NATIVE_PROCESS_SPECS, NativeProcessSpec, PROCESS_SPECS};
use solstone_core_cli::{
    CHECK_HELP, CHECK_USAGE, DESCRIBE_USAGE, HEALTH_USAGE, INSTALL_MODELS_HELP,
    INSTALL_MODELS_USAGE, TOP_USAGE,
};

const POISON_INTERPRETER: &str = r#"#!/bin/sh
printf '%s:%s\n' "${POISON_ROUTE:-reached}" "${0##*/}" >> "$POISON_MARKER"
exit 97
"#;
const STORAGE_OPS_REFERENCE_GRAMMAR: &str =
    include_str!("../../../fixtures/journal-storage-ops-reference-grammar.txt");
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const CHECK_USAGE_ANCHOR: &[u8] = CHECK_USAGE.as_bytes();
const INSTALL_MODELS_USAGE_ANCHOR: &[u8] = INSTALL_MODELS_USAGE.as_bytes();
const CONVEY_USAGE_ANCHOR: &[u8] = b"usage: journal convey [-h] --port PORT [-v] [-d]\n";
const RESTART_CONVEY_USAGE_ANCHOR: &[u8] =
    b"usage: journal restart-convey [-h] [--timeout TIMEOUT] [-v] [-d]\n";
const DESCRIBE_USAGE_ANCHOR: &[u8] = DESCRIBE_USAGE.as_bytes();
const CHECK_JSON_TOP_LEVEL_KEYS: &[&str] =
    &["platform", "checks", "overall", "feedback_url", "version"];
const LANE_AU_REQUIRED_NATIVE_TOKENS: &[&str] =
    &["engage", "maintenance", "heartbeat", "maint", "backup"];
const REQUIRED_NATIVE_TOKENS: &[&str] = &["brain"];

#[derive(Debug, Clone, Copy)]
struct Probe {
    token: &'static str,
    argv: &'static [&'static str],
    expected_exit: i32,
    stderr_anchor: Option<&'static [u8]>,
}

#[test]
fn lane_au_owner_verbs_are_registered_for_native_dispatch() {
    let native_tokens = NATIVE_PROCESS_SPECS
        .iter()
        .map(|spec| spec.token)
        .collect::<BTreeSet<_>>();
    let missing = LANE_AU_REQUIRED_NATIVE_TOKENS
        .iter()
        .copied()
        .filter(|token| !native_tokens.contains(token))
        .collect::<Vec<_>>();

    assert!(
        missing.is_empty(),
        "required Lane AU native process tokens are missing: {missing:?}"
    );
}

#[test]
fn lane_av_brain_is_registered_for_native_dispatch() {
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
        "required Lane AV native process tokens are missing: {missing:?}"
    );
}

const SUPERVISOR_USAGE_ANCHOR: &[u8] =
    b"usage: journal supervisor [-h] [--no-daily] [--no-cortex] [--no-spl]\n";
const SERVICE_UNKNOWN_ANCHOR: &[u8] = b"Unknown subcommand: --nonsense; Available: install, uninstall, start, stop, restart, status, logs\n";

const PROBES: &[Probe] = &[
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
        // Exit 1 also covers a pre-spawn coherence failure; this schema/reason
        // prefix proves depict reached its malformed-request path.
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
        expected_exit: 64,
        stderr_anchor: None,
    },
    Probe {
        token: "schedule",
        argv: &["--nonsense"],
        expected_exit: 2,
        stderr_anchor: None,
    },
    Probe {
        token: "convey",
        argv: &["--nonsense"],
        expected_exit: 2,
        stderr_anchor: Some(CONVEY_USAGE_ANCHOR),
    },
    Probe {
        token: "restart-convey",
        argv: &["--nonsense"],
        expected_exit: 2,
        stderr_anchor: Some(RESTART_CONVEY_USAGE_ANCHOR),
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
    // supervisor and start were deliberately unprobed until a verb-level usage
    // path existed for them. 1d1523b4b added both tokens to NATIVE_PROCESS_SPECS
    // without probes; registering them at exit 64 would have made this contract
    // green while making it check less, because 64 is the same code the binary
    // emits for a verb it does not have at all -- a row expecting it certifies
    // the token on a code that would survive deleting supervisor from the
    // sibling entirely. parse_supervisor now has a verb-level usage path, so
    // both tokens exit 2 with supervisor's own usage and are registered against
    // that, with a stderr anchor. The red is resolved by proof, not by relaxing
    // the guard. `spl` still carries the same defect -- its row above expects
    // 64 -- and that fix is scoped separately.
    //
    // `supervisor` and `start` share one native invocation shape by design:
    // `native_process_args` prefixes both with `SUPERVISOR_SERVICE = ["supervisor"]`
    // and drops the dispatcher token. Consequently `journal start --help` is required
    // to identify itself as `usage: journal supervisor`; the sibling cannot recover the
    // original `start` spelling. The dedicated help assertion records that prog ceiling.
    //
    // Both historical census rows are Service rows, so `dispatch_process` applies the
    // installation coherence guard before selecting the native sibling. This contract
    // assumes Harness::new provides a coherent installation; a guard failure is a launch
    // failure and is not evidence about supervisor/start argument parsing.
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
        stderr_anchor: Some(SUPERVISOR_USAGE_ANCHOR),
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
        token: "export",
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
        token: "observer",
        argv: &["--nonsense"],
        expected_exit: 2,
        stderr_anchor: None,
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

        let dispatcher_source = PathBuf::from(env!("CARGO_BIN_EXE_solstone-core-journal"));
        let core_source = locate_workspace_binary("solstone-core", "solstone-core");
        let depict_source = locate_workspace_binary("solstone-core-depict", "solstone-core-depict");
        let describe_source =
            locate_workspace_binary("solstone-core-describe", "solstone-core-describe");

        let dispatcher_artifact = artifact(&dispatcher_source);
        let mut sibling_artifacts = BTreeMap::new();
        sibling_artifacts.insert("solstone-core", artifact(&core_source));
        sibling_artifacts.insert("solstone-core-depict", artifact(&depict_source));
        sibling_artifacts.insert("solstone-core-describe", artifact(&describe_source));

        let dispatcher = sibling_dir.join("solstone-core-journal");
        copy_executable(&dispatcher_source, &dispatcher);
        std::os::unix::fs::symlink(&dispatcher, sibling_dir.join("journal"))
            .expect("link journal command to dispatcher sibling");
        copy_executable(&core_source, &sibling_dir.join("solstone-core"));
        copy_executable(&depict_source, &sibling_dir.join("solstone-core-depict"));
        copy_executable(
            &describe_source,
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

fn run_dispatcher(
    context: &VerdictContext<'_>,
    token: &str,
    argv: &[&str],
) -> io::Result<Option<ExitStatus>> {
    run_dispatcher_with_timeout(context, token, argv, PROBE_TIMEOUT)
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
fn native_sense_batch_keeps_transcribe_native_while_describe_is_honestly_python_blocked() {
    let harness = Harness::new();
    let context = harness.context();
    let day = "20990101";
    let stream = "capture";
    let segment = "120000_1";
    let directory = context
        .journal
        .join("chronicle")
        .join(day)
        .join(stream)
        .join(segment);
    fs::create_dir_all(&directory).expect("create batch segment");
    fs::write(directory.join("audio.flac"), b"not a flac file").expect("write garbage audio");
    fs::write(directory.join("screen.webm"), b"not a webm file").expect("write garbage video");

    // AJ-D has not landed: describe still resolves through the Python process
    // table and must touch the poisoned sibling python3. When AJ-D becomes a
    // native process spec, tighten this expectation to require no shim touch.
    let output = run_dispatcher_with_output_and_environment(
        &context,
        "sense",
        &["--day", day, "--stream", stream, "--segment", segment],
        &[("SOL_SKIP_SUPERVISOR_CHECK", "1")],
    )
    .expect("run native sense batch through dispatcher");
    assert_eq!(
        output.status.code(),
        Some(0),
        "sense stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let sidecar: serde_json::Value = serde_json::from_slice(
        &fs::read(directory.join("audio.jsonl")).expect("native transcribe decode-failure sidecar"),
    )
    .expect("transcribe sidecar JSON");
    assert_eq!(sidecar["_solstone_processing"]["handler"], "transcribe");
    assert_eq!(sidecar["_solstone_processing"]["state"], "failed");
    assert_eq!(
        sidecar["_solstone_processing"]["reason_code"],
        "corrupt_input"
    );

    let poison = fs::read_to_string(context.poison_marker).expect("describe Python poison marker");
    assert_eq!(poison.lines().collect::<Vec<_>>(), ["reached:python3"]);
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
fn native_check_and_install_models_help_match_exported_cli_constants_through_dispatcher() {
    let harness = Harness::new();
    let context = harness.context();
    for (token, expected_help) in [
        ("check", CHECK_HELP),
        ("install-models", INSTALL_MODELS_HELP),
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
fn native_check_and_install_models_malformed_argv_use_exported_usage_through_dispatcher() {
    let harness = Harness::new();
    let context = harness.context();
    for (token, expected_usage) in [
        ("check", CHECK_USAGE),
        ("install-models", INSTALL_MODELS_USAGE),
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
    let sol = aliases.join("sol");
    fs::write(
        &sol,
        format!(
            "# managed-version: 7\n: \"${{SOLSTONE_JOURNAL:={}}}\"\nSOL_BIN='/native/sol'\n",
            current.display()
        ),
    )
    .expect("write managed sol wrapper");

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
    let no_service_target_arg = no_service_target.display().to_string();
    let no_service = run_dispatcher_with_output(
        &context,
        "config",
        &["journal", &no_service_target_arg, "--switch", "--yes"],
    )
    .expect("run native config no-service branch through dispatcher");
    assert_eq!(no_service.status.code(), Some(0));
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
        b"usage: solstone-core top [-h] [-v] [-d]\nsolstone-core top: error: invalid arguments\n"
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
fn native_service_dispatch_reaches_the_real_bodies_without_python() {
    let harness = Harness::new();
    let context = harness.context();
    prove_poison_interpreters_live(&context);
    fs::create_dir_all(context.home).expect("create isolated service home");

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
        (
            "down",
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
        b"solstone-core top: terminal failure: stdin is not a terminal\n"
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
    assert!(health.stdout.is_empty());
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
    assert_eq!(logs.stderr, b"No health directory found.\n");
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
        path: dispatcher.clone(),
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
    let dispatcher_artifact = Artifact {
        path: dispatcher.clone(),
    };
    let mut sibling_artifacts = BTreeMap::new();
    sibling_artifacts.insert(
        "synthetic-native",
        Artifact {
            path: sibling.clone(),
        },
    );
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
fn native_start_help_uses_supervisor_program_name() {
    let harness = Harness::new();
    let context = harness.context();
    let output = run_dispatcher_with_output(&context, "start", &["--help"])
        .expect("run start help through dispatcher");

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stderr, b"");
    assert!(output.stdout.starts_with(b"usage: journal supervisor"));
    assert!(!context.poison_marker.exists());
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
