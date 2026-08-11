// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[allow(dead_code)]
#[path = "../../solstone-core-journal-cli/src/processes.rs"]
mod production_processes;

use production_processes::{NATIVE_PROCESS_SPECS, NativeProcessSpec, PROCESS_SPECS};

const POISON_INTERPRETER: &str = r#"#!/bin/sh
printf '%s\n' "$0" > "$POISON_MARKER"
exit 97
"#;
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
    // `supervisor` and `start` share one NativeProcessSpec preset (`supervisor`),
    // so both probes exercise the same native entry point. `--nonsense` is
    // rejected during argument parsing, before any journal or running solstone
    // is required, and the native root answers an unknown flag with exit 64.
    Probe {
        token: "supervisor",
        argv: &["--nonsense"],
        expected_exit: 64,
    },
    Probe {
        token: "start",
        argv: &["--nonsense"],
        expected_exit: 64,
    },
    Probe {
        token: "grab",
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
    let mut child = Command::new(context.dispatcher)
        .arg(token)
        .args(argv)
        .env("POISON_MARKER", context.poison_marker)
        .env("HOME", context.home)
        .env("SOLSTONE_JOURNAL", context.journal)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    wait_for_child(&mut child, Instant::now() + PROBE_TIMEOUT)
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
