// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::time::Duration;

use solstone_core_system::process::{
    BoundedHelperBudget, BoundedHelperRequest, BoundedHelperResources, run_bounded_helper,
};

#[path = "../src/service_capture_windows.rs"]
mod capture;

#[test]
#[ignore = "fresh child of the source-bound Windows capture receipt only"]
fn native_capture_child() {
    let mode = std::env::var("SOLSTONE_CAPTURE_CONTROL").expect("capture mode");
    let journal = std::env::var_os("SOLSTONE_CAPTURE_JOURNAL").expect("capture journal");
    match capture::native_control(std::path::Path::new(&journal), &mode) {
        Ok(code) => {
            println!("\nCAPTURE_CONTROL_{mode}=PASS");
            std::process::exit(code);
        }
        Err(error) => panic!("native capture control failed: {error}"),
    }
}

#[test]
#[ignore = "operator-run native capture receipt"]
fn windows_service_capture_receipt() {
    for (mode, expected) in [
        ("normal", 0),
        ("startup-refusal", 75),
        ("panic", 74),
        ("second-install", 74),
        ("rollback-failure", 74),
        ("restore-failure", 74),
        ("withheld-writer", 0),
        ("rollover", 0),
    ] {
        let journal = std::sync::Arc::new(tempfile::tempdir().unwrap());
        let executable = std::env::current_exe().unwrap();
        let package_root = executable.parent().unwrap().to_path_buf();
        let mut resources = BoundedHelperResources::new();
        resources.retain(journal.clone());
        let output = run_bounded_helper(BoundedHelperRequest {
            executable,
            current_directory: package_root.clone(),
            package_root,
            arguments: vec![
                "--ignored".into(),
                "--exact".into(),
                "windows_service_capture::native_capture_child".into(),
                "--nocapture".into(),
                "--test-threads=1".into(),
            ],
            environment: BTreeMap::from([
                (
                    OsString::from("SystemRoot"),
                    std::env::var_os("SystemRoot").unwrap(),
                ),
                (
                    OsString::from("SOLSTONE_CAPTURE_CONTROL"),
                    OsString::from(mode),
                ),
                (
                    OsString::from("SOLSTONE_CAPTURE_JOURNAL"),
                    journal.path().as_os_str().to_owned(),
                ),
            ]),
            stdin: Vec::new(),
            budget: BoundedHelperBudget {
                timeout: Duration::from_secs(15),
                stdin_limit_bytes: 1,
                stdout_limit_bytes: 64 * 1024,
                stderr_limit_bytes: 64 * 1024,
            },
            resources,
            resource_limits: None,
        })
        .unwrap_or_else(|error| panic!("{mode}: {error}"));
        assert!(output.quiescent, "{mode}: fixture Job is not empty");
        assert_eq!(output.exit_code, expected, "{mode}");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let original_output = format!("{stdout}\n{stderr}");
        let marker = format!("CAPTURE_CONTROL_{mode}=PASS");
        assert_eq!(
            stdout
                .lines()
                .chain(stderr.lines())
                .filter(|line| *line == marker)
                .count(),
            1,
            "{mode}: {original_output}"
        );
        if matches!(mode, "normal" | "startup-refusal" | "panic") {
            let mut captured = Vec::new();
            for day in std::fs::read_dir(journal.path().join("chronicle")).unwrap() {
                let health = day.unwrap().path().join("health");
                for entry in std::fs::read_dir(health).unwrap() {
                    let path = entry.unwrap().path();
                    if path.extension().is_some_and(|extension| extension == "log") {
                        captured.extend(std::fs::read(path).unwrap());
                    }
                }
            }
            let captured = String::from_utf8_lossy(&captured);
            assert!(
                captured.contains("capture stdout witness"),
                "{mode}: missing stdout"
            );
            assert!(
                captured.contains("capture stderr witness"),
                "{mode}: missing stderr"
            );
            assert!(!original_output.contains("capture stdout witness"));
            assert!(!original_output.contains("capture stderr witness"));
            if mode == "panic" {
                assert!(original_output.contains("windows installed supervisor panicked"));
                assert!(!captured.contains("windows installed supervisor panicked"));
            }
        }
    }
    println!("JOURNAL_WIN_CI_SERVICE_CAPTURE=PASS");
}
