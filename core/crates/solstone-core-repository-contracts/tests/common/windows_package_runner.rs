// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const CASES: &[(&str, &str, &[&str])] = &[
    (
        "batch-pass-a",
        r#"#[test] fn passes() {
        assert_eq!(std::env::current_dir().unwrap().canonicalize().unwrap(), std::path::Path::new(env!("CARGO_MANIFEST_DIR")).canonicalize().unwrap());
        assert_eq!(std::env::var("CARGO_MANIFEST_DIR").unwrap(), env!("CARGO_MANIFEST_DIR"));
    }"#,
        &[],
    ),
    ("batch-pass-z", "#[test] fn passes() {}", &[]),
    (
        "batch-fail",
        r#"#[test] fn passes() {} #[test] fn fails() {
        println!("running 1 test\ntest child ... ok\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s");
        panic!("synthetic outer failure");
    }"#,
        &[],
    ),
    ("batch-empty", "pub fn value() -> u8 { 1 }", &[]),
    (
        "batch-feature",
        "#[test] fn base() {} #[cfg(feature=\"full-tests\")] #[test] fn enabled() {}",
        &["full-tests"],
    ),
    (
        "batch-timeout",
        "#[test] fn stalls() { std::thread::sleep(std::time::Duration::from_secs(60)); }",
        &[],
    ),
    (
        "batch-descendant",
        r#"#[test] fn descendant() {
        let child = std::process::Command::new(std::env::var("ComSpec").unwrap())
            .args(["/d", "/c", "ping -n 100 127.0.0.1 >nul"]).spawn().unwrap();
        std::fs::write("descendant.pid.tmp", child.id().to_string()).unwrap();
        std::fs::rename("descendant.pid.tmp", "descendant.pid").unwrap();
        while !std::path::Path::new("parent.release").exists() { std::thread::sleep(std::time::Duration::from_millis(10)); }
    }"#,
        &[],
    ),
];

struct Fixture {
    _temporary: tempfile::TempDir,
    root: PathBuf,
    control_mode: Option<String>,
}

impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("workspace with spaces");
        fs::create_dir_all(root.join("core")).unwrap();
        fs::write(root.join("Makefile"), "RUST_HOST_EXCLUDES := --exclude solstone-core-speakers-analyze --exclude solstone-core-speakers-onnx --exclude solstone-core-vad-analyze\n").unwrap();
        let members: Vec<_> = CASES.iter().map(|(name, _, _)| *name).collect();
        fs::write(
            root.join("core/Cargo.toml"),
            format!(
                "[workspace]\nmembers={}\nresolver=\"3\"\n",
                serde_json::to_string(&members).unwrap()
            ),
        )
        .unwrap();
        for (name, source, features) in CASES {
            let path = root.join("core").join(name);
            fs::create_dir_all(path.join("src")).unwrap();
            fs::write(
                path.join("Cargo.toml"),
                format!(
                    "[package]\nname=\"{name}\"\nversion=\"0.0.0\"\nedition=\"2024\"\n{}",
                    if features.is_empty() {
                        ""
                    } else {
                        "[features]\nfull-tests=[]\n"
                    }
                ),
            )
            .unwrap();
            fs::write(path.join("src/lib.rs"), source).unwrap();
        }
        assert!(
            Command::new("cargo")
                .args(["generate-lockfile", "--manifest-path", "core/Cargo.toml"])
                .current_dir(&root)
                .status()
                .unwrap()
                .success()
        );
        Self {
            _temporary: temporary,
            root,
            control_mode: None,
        }
    }

    fn registry(&self, selected: &[&str]) {
        let mut text = "version=1\nsets=[\"component\",\"differential\",\"race\",\"live\"]\nareas=[\"test\"]\nplatforms=[\"linux\",\"windows\"]\nprerequisites=[]\nserial_groups=[]\nruntimes=[\"none\"]\n[timeouts]\nquick=30\nstall=2\n".to_owned();
        for (name, _, features) in CASES {
            text.push_str(&format!("\n[[package_suites]]\nid=\"package::{name}\"\npackage=\"{name}\"\nset=\"component\"\nareas=[\"test\"]\nplatforms=[\"{}\"]\ntimeout=\"{}\"\ndefault_full=true\nruntime=\"none\"\nfeatures={}\n", if selected.contains(name) { "windows" } else { "linux" }, if *name == "batch-timeout" || *name == "batch-descendant" { "stall" } else { "quick" }, serde_json::to_string(features).unwrap()));
        }
        fs::write(self.root.join("registry.toml"), text).unwrap();
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_solstone-ci"));
        command
            .args([
                "windows-run",
                "--registry",
                "registry.toml",
                "--coverage",
                "coverage.json",
            ])
            .env("EXPECTED_JOURNAL_COMMIT", "a".repeat(40))
            .env("EXPECTED_JOURNAL_CARGO_LOCK_SHA256", "b".repeat(64))
            .current_dir(&self.root);
        if let Some(mode) = &self.control_mode {
            let mut paths = vec![self.root.join("cargo-proxy")];
            paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
            command
                .env("PATH", std::env::join_paths(paths).unwrap())
                .env("REAL_CARGO", env!("CARGO"))
                .env("CAPTURE_CONTROL_MODE", mode);
        }
        command
    }

    fn report(&self, output: &Output) -> Value {
        let path = self.root.join("coverage.json");
        assert!(
            path.exists(),
            "stdout={}\nstderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
    }

    fn install_outer_runner(&mut self, mode: &str) {
        let bin = self.root.join("core/batch-pass-a/src/bin");
        fs::create_dir_all(&bin).unwrap();
        // A disposable Cargo proxy changes evidence after the actual Cargo
        // invocation, independently of its runner-array configuration rules.
        // What a member's runner leaves behind when Cargo exits is its launch
        // record; the result is written later, from that record.
        fs::write(bin.join("capture-control.rs"), r#"
use std::{fs, process::Command};
fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let status = Command::new(std::env::var_os("REAL_CARGO").unwrap()).args(&args).status().unwrap();
    if args.first().is_some_and(|s| s == "test") {
        let mode = std::env::var("CAPTURE_CONTROL_MODE").unwrap();
        for entry in fs::read_dir("core/target").unwrap() {
            let entry = entry.unwrap();
            if !entry.file_name().to_string_lossy().starts_with("journal-win-package-captures-") { continue; }
            for child in fs::read_dir(entry.path()).unwrap() {
                let path = child.unwrap().path();
                if path.to_string_lossy().ends_with(".launch.json") && fs::read_to_string(&path).unwrap().contains("batch_pass_a-") {
                    if mode == "missing" { fs::remove_file(&path).unwrap(); }
                    if mode == "truncated" { fs::write(&path, b"{").unwrap(); }
                }
            }
        }
        if mode == "aggregate-fail" { std::process::exit(9); }
    }
    std::process::exit(status.code().unwrap_or(1));
}
"#).unwrap();
        let build = Command::new("cargo")
            .args([
                "build",
                "--manifest-path",
                "core/Cargo.toml",
                "--locked",
                "-p",
                "batch-pass-a",
                "--bin",
                "capture-control",
                "--message-format=json",
            ])
            .current_dir(&self.root)
            .output()
            .unwrap();
        assert!(
            build.status.success(),
            "{}",
            String::from_utf8_lossy(&build.stderr)
        );
        // The real gate exports CARGO_BUILD_TARGET. Let Cargo identify the
        // executable instead of guessing its target-specific output directory.
        let executables: Vec<_> = std::str::from_utf8(&build.stdout)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .filter(|v| {
                v["reason"] == "compiler-artifact" && v["target"]["name"] == "capture-control"
            })
            .filter_map(|v| v["executable"].as_str().map(PathBuf::from))
            .collect();
        assert_eq!(executables.len(), 1);
        let wrapper = &executables[0];
        let proxy = self.root.join("cargo-proxy");
        fs::create_dir(&proxy).unwrap();
        fs::copy(
            wrapper,
            proxy.join(format!("cargo{}", std::env::consts::EXE_SUFFIX)),
        )
        .unwrap();
        self.control_mode = Some(mode.to_owned());
    }
}

fn row<'a>(report: &'a Value, package: &str) -> &'a Value {
    report["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["suite_id"] == format!("package::{package}"))
        .unwrap()
}

#[test]
fn real_cargo_preserves_context_and_isolates_pass_failure_empty_features_and_timeout() {
    let fixture = Fixture::new();
    fixture.registry(&[
        "batch-pass-a",
        "batch-pass-z",
        "batch-fail",
        "batch-empty",
        "batch-feature",
        "batch-timeout",
    ]);
    let start = std::time::Instant::now();
    let output = fixture.command().output().unwrap();
    let report = fixture.report(&output);
    assert!(!output.status.success());
    assert!(start.elapsed().as_secs() < 60);
    for name in ["batch-pass-a", "batch-pass-z", "batch-feature"] {
        assert_eq!(row(&report, name)["disposition"], "passed", "{report}");
        assert_eq!(row(&report, name)["exit_codes"][1], 0);
    }
    assert_eq!(row(&report, "batch-feature")["executed_counts"][0], 2);
    assert_eq!(row(&report, "batch-fail")["disposition"], "failed");
    assert_eq!(row(&report, "batch-fail")["exit_codes"][1], 101);
    assert_eq!(row(&report, "batch-fail")["executed_counts"][0], 2);
    assert_eq!(row(&report, "batch-empty")["disposition"], "unexecuted");
    assert_eq!(row(&report, "batch-timeout")["disposition"], "timed-out");
    for item in report["rows"].as_array().unwrap() {
        let logs = item["log_paths"].as_array().unwrap();
        for log in logs {
            assert!(Path::new(log.as_str().unwrap()).is_file());
        }
        assert!(
            logs.iter()
                .any(|p| p.as_str().unwrap().ends_with(".result.json"))
        );
    }
    assert_ne!(
        row(&report, "batch-pass-a")["log_paths"],
        row(&report, "batch-pass-z")["log_paths"]
    );
}

#[test]
fn missing_or_truncated_member_capture_preserves_a_verified_sibling_and_fails_the_cli() {
    for mode in ["missing", "truncated"] {
        let mut fixture = Fixture::new();
        fixture.registry(&["batch-pass-a", "batch-pass-z"]);
        fixture.install_outer_runner(mode);
        let output = fixture.command().output().unwrap();
        let report = fixture.report(&output);
        assert!(!output.status.success());
        assert_eq!(
            row(&report, "batch-pass-a")["disposition"],
            "failed",
            "{report}"
        );
        assert_eq!(row(&report, "batch-pass-a")["executed_counts"][0], 0);
        assert_eq!(
            row(&report, "batch-pass-z")["disposition"],
            "passed",
            "{report}"
        );
        assert_eq!(row(&report, "batch-pass-z")["executed_counts"][0], 1);
    }
}

#[test]
fn cargo_failure_after_valid_successful_captures_keeps_member_credit_and_fails_the_cli() {
    let mut fixture = Fixture::new();
    fixture.registry(&["batch-pass-a", "batch-pass-z"]);
    fixture.install_outer_runner("aggregate-fail");
    let output = fixture.command().output().unwrap();
    let report = fixture.report(&output);
    assert!(!output.status.success());
    for name in ["batch-pass-a", "batch-pass-z"] {
        assert_eq!(row(&report, name)["disposition"], "passed", "{report}");
        assert_ne!(row(&report, name)["exit_codes"][0], 0);
        assert_eq!(row(&report, name)["exit_codes"][1], 0);
    }
}

#[test]
fn inherited_array_runner_either_uses_the_adapter_or_fails_without_credit() {
    let fixture = Fixture::new();
    fixture.registry(&["batch-pass-a", "batch-pass-z"]);
    let key = if cfg!(windows) {
        "target.x86_64-pc-windows-msvc"
    } else {
        "target.'cfg(all())'"
    };
    fs::create_dir(fixture.root.join(".cargo")).unwrap();
    fs::write(
        fixture.root.join(".cargo/config.toml"),
        format!("[{key}]\nrunner=[\"nonexistent-inherited-runner\",\"unexpected-prefix\"]\n"),
    )
    .unwrap();
    let output = fixture.command().output().unwrap();
    let report = fixture.report(&output);
    if output.status.success() {
        for name in ["batch-pass-a", "batch-pass-z"] {
            assert_eq!(row(&report, name)["disposition"], "passed");
            assert!(
                row(&report, name)["log_paths"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|p| p.as_str().unwrap().ends_with(".result.json"))
            );
        }
    } else {
        assert!(
            report["rows"]
                .as_array()
                .unwrap()
                .iter()
                .all(|r| r["disposition"] != "passed")
        );
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
#[test]
fn cargo_adapter_cleans_the_native_descendant_on_timeout_and_success() {
    use std::process::Stdio;
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
    };
    for timeout in [false, true] {
        let fixture = Fixture::new();
        fixture.registry(&["batch-descendant", "batch-pass-z"]);
        let package = fixture.root.join("core/batch-descendant");
        let mut child = fixture
            .command()
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let start = Instant::now();
        let pid_path = package.join("descendant.pid");
        while !pid_path.exists() && start.elapsed() < Duration::from_secs(30) {
            std::thread::sleep(Duration::from_millis(10));
        }
        let pid: u32 = fs::read_to_string(pid_path)
            .expect("descendant starts before timeout")
            .parse()
            .unwrap();
        // Retain the measured lifetime before releasing its parent.
        let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        assert!(!handle.is_null());
        if !timeout {
            fs::write(package.join("parent.release"), b"release").unwrap();
        }
        assert_eq!(child.wait().unwrap().success(), !timeout);
        let stopped = unsafe { WaitForSingleObject(handle, 5000) };
        unsafe {
            CloseHandle(handle);
        }
        assert_eq!(stopped, WAIT_OBJECT_0);
        let report: Value =
            serde_json::from_slice(&fs::read(fixture.root.join("coverage.json")).unwrap()).unwrap();
        assert_eq!(
            row(&report, "batch-descendant")["disposition"],
            if timeout { "timed-out" } else { "passed" }
        );
    }
}
