// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Proves the native `health` and service-log bodies exist independently before
//! their journal dispatcher cuts. This deliberately invokes the real
//! `solstone-core` artifact directly: dispatcher ownership is separate work.

#![cfg(unix)]

use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

const POISON_INTERPRETER: &str = r#"#!/bin/sh
printf '%s:%s\n' "${POISON_ROUTE:-reached}" "${0##*/}" >> "$POISON_MARKER"
exit 97
"#;

#[derive(Debug)]
struct Probe {
    token: &'static str,
    args: &'static [&'static str],
    expected_exit: i32,
}

const PROBES: &[Probe] = &[
    Probe {
        token: "health",
        args: &["health"],
        expected_exit: 1,
    },
    Probe {
        token: "health-logs",
        args: &["health", "logs", "-f"],
        expected_exit: 0,
    },
    Probe {
        token: "service-logs",
        args: &["service", "logs"],
        expected_exit: 0,
    },
];

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
            "solstone-core-journal-health-body-{label}-{}-{stamp}",
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

struct Harness {
    _temp: TempDir,
    binary: PathBuf,
    bin_dir: PathBuf,
    home: PathBuf,
    journal: PathBuf,
    poison_marker: PathBuf,
}

impl Harness {
    fn new() -> Self {
        let temp = TempDir::new("bin");
        let bin_dir = temp.path.join("bin");
        fs::create_dir(&bin_dir).expect("create binary directory");

        let source = locate_workspace_binary("solstone-core", "solstone-core");
        let binary = bin_dir.join("solstone-core");
        fs::copy(source, &binary).expect("copy current solstone-core artifact");
        make_executable(&binary);

        for interpreter in ["python", "python3", "pytest", "uv", "ruff"] {
            let path = bin_dir.join(interpreter);
            fs::write(&path, POISON_INTERPRETER).expect("write poison interpreter");
            make_executable(&path);
        }

        let home = temp.path.join("home");
        let journal = temp.path.join("journal");
        fs::create_dir_all(&home).expect("create isolated home");
        fs::create_dir_all(&journal).expect("create isolated journal");

        Self {
            poison_marker: temp.path.join("python-invoked.txt"),
            _temp: temp,
            binary,
            bin_dir,
            home,
            journal,
        }
    }

    fn prove_poison_liveness(&self) {
        let _ = fs::remove_file(&self.poison_marker);
        for name in ["python", "python3", "pytest", "uv", "ruff"] {
            let sibling = Command::new(self.bin_dir.join(name))
                .env("POISON_MARKER", &self.poison_marker)
                .env("POISON_ROUTE", "sibling")
                .status()
                .expect("execute sibling poison");
            assert_eq!(sibling.code(), Some(97), "{name}: sibling poison exit");

            let path = Command::new(name)
                .env("PATH", &self.bin_dir)
                .env("POISON_MARKER", &self.poison_marker)
                .env("POISON_ROUTE", "path")
                .status()
                .expect("execute PATH poison");
            assert_eq!(path.code(), Some(97), "{name}: PATH poison exit");
        }

        let observed = fs::read_to_string(&self.poison_marker).expect("poison liveness record");
        assert_eq!(
            observed.lines().count(),
            10,
            "poison liveness must record exactly one row per route and name"
        );
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
            expected,
            "every sibling and PATH poison must be proven live"
        );
        fs::remove_file(&self.poison_marker).expect("clear poison liveness epoch");
    }

    fn run(&self, args: &[&str]) -> Output {
        let _ = fs::remove_file(&self.poison_marker);
        Command::new(&self.binary)
            .args(args)
            .env("HOME", &self.home)
            .env("SOLSTONE_JOURNAL", &self.journal)
            .env("PATH", &self.bin_dir)
            .env("POISON_MARKER", &self.poison_marker)
            .output()
            .expect("run real native health body")
    }
}

fn make_executable(path: &Path) {
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("make executable");
}

fn locate_workspace_binary(package: &str, binary: &str) -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_manifest = manifest_dir
        .parent()
        .expect("crates dir")
        .parent()
        .expect("core dir")
        .join("Cargo.toml");
    let metadata = Command::new(env!("CARGO"))
        .args([
            "metadata",
            "--format-version=1",
            "--no-deps",
            "--manifest-path",
        ])
        .arg(&workspace_manifest)
        .output()
        .expect("cargo metadata should execute");
    assert!(
        metadata.status.success(),
        "cargo metadata failed:\n{}",
        String::from_utf8_lossy(&metadata.stderr)
    );
    let metadata: serde_json::Value =
        serde_json::from_slice(&metadata.stdout).expect("cargo metadata JSON");
    let package_id = metadata["packages"]
        .as_array()
        .expect("metadata packages")
        .iter()
        .find(|candidate| candidate["name"].as_str() == Some(package))
        .and_then(|candidate| candidate["id"].as_str())
        .expect("exact workspace package identity");

    let output = Command::new(env!("CARGO"))
        .args(["build", "--manifest-path"])
        .arg(&workspace_manifest)
        .args(["-p", package, "--bin", binary, "--message-format=json"])
        .output()
        .expect("cargo build should execute");
    assert!(
        output.status.success(),
        "cargo build -p {package} --bin {binary} failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let Ok(message) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if message["reason"].as_str() != Some("compiler-artifact")
            || message["package_id"].as_str() != Some(package_id)
            || message["target"]["name"].as_str() != Some(binary)
        {
            continue;
        }
        let is_bin = message["target"]["kind"]
            .as_array()
            .is_some_and(|kinds| kinds.iter().any(|kind| kind.as_str() == Some("bin")));
        if !is_bin {
            continue;
        }
        if let Some(executable) = message["executable"].as_str() {
            return PathBuf::from(executable);
        }
    }
    panic!("cargo build did not report the exact {package}/{binary} compiler artifact");
}

#[test]
fn health_and_service_log_real_native_bodies_survive_live_interpreter_poisons() {
    assert_eq!(
        PROBES.len(),
        PROBES
            .iter()
            .map(|probe| probe.token)
            .collect::<BTreeSet<_>>()
            .len(),
        "the pre-cut body registry must not contain duplicate tokens"
    );
    assert_eq!(
        PROBES
            .iter()
            .map(|probe| probe.token)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["health", "health-logs", "service-logs"]),
        "the pre-cut body registry is closed"
    );

    let harness = Harness::new();
    harness.prove_poison_liveness();

    for probe in PROBES {
        let output = harness.run(probe.args);
        assert_eq!(
            output.status.code(),
            Some(probe.expected_exit),
            "{}: wrong native-body exit; stderr={}",
            probe.token,
            String::from_utf8_lossy(&output.stderr)
        );
        let (expected_stdout, expected_stderr) = match probe.token {
            "health" => (
                concat!(
                    "Sound tagging is degraded because its CED assets are unavailable. ",
                    "Transcription will continue. Use `journal install-models` to check or repair the CED assets. ",
                    "If the signed CED app payload is unavailable on Windows, reinstall the journal app.\n",
                    "Object detection is degraded because its RF-DETR assets are unavailable. ",
                    "Screen descriptions will continue. Use `journal install-models` to check or repair the RF-DETR assets.\n",
                )
                .as_bytes()
                .to_vec(),
                format!(
                    "Cannot connect: callosum socket not found at {}/health/callosum.sock\n",
                    harness.journal.display()
                ),
            ),
            "health-logs" => (Vec::new(), "No log files found.\n".to_owned()),
            "service-logs" => (b"=== service.log === (not found)\n".to_vec(), String::new()),
            other => panic!("unregistered probe {other}"),
        };
        assert_eq!(
            output.stdout, expected_stdout,
            "{}: body stdout",
            probe.token
        );
        assert_eq!(
            output.stderr,
            expected_stderr.as_bytes(),
            "{}: body output",
            probe.token
        );
        assert!(
            !harness.poison_marker.exists(),
            "{}: native body invoked an interpreter",
            probe.token
        );
    }
}
