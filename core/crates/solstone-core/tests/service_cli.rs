// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;
use std::process::{Command, Output, Stdio};

const BINARY: &str = env!("CARGO_BIN_EXE_solstone-core");
const RECOVERY_HEADER: &str = "this installation couldn't be verified.";
const RECOVERY_SETUP: &str =
    "run `journal setup` to check it. if setup finishes successfully, try again.";
const TRUNCATION_MARKER: &str = "…[truncated]";

#[path = "support/hostile_binary.rs"]
mod hostile_binary;
#[path = "support/installation_binding.rs"]
mod installation_binding;

use hostile_binary::{copied_binary, hostile_binary};

fn service_install_output(binary: &Path, home: &Path) -> Output {
    Command::new(binary)
        .args(["service", "install"])
        .stdin(Stdio::null())
        .env("HOME", home)
        .env_remove("SOLSTONE_JOURNAL")
        .output()
        .expect("service install process runs")
}

fn recovery_details(stderr: &[u8]) -> String {
    let rendered = std::str::from_utf8(stderr).expect("recovery stderr is UTF-8");
    assert!(
        rendered.ends_with('\n'),
        "recovery stderr has one final newline"
    );
    let lines: Vec<_> = rendered
        .strip_suffix('\n')
        .expect("final newline")
        .split('\n')
        .collect();
    assert_eq!(lines.len(), 3, "recovery has exactly three lines");
    assert_eq!(lines[0], RECOVERY_HEADER);
    assert_eq!(lines[1], RECOVERY_SETUP);
    lines[2]
        .strip_prefix("details: ")
        .expect("recovery has details line")
        .to_owned()
}

#[test]
fn service_help_and_invalid_port_are_owned_by_the_native_binary() {
    let help = Command::new(BINARY)
        .args(["service", "--help"])
        .output()
        .expect("native service help runs");
    assert!(help.status.success());
    assert!(help.stderr.is_empty());
    assert_eq!(
        String::from_utf8(help.stdout).unwrap(),
        solstone_core_cli::SERVICE_USAGE
    );

    let invalid = Command::new(BINARY)
        .args(["service", "install", "--port", "not-a-port"])
        .output()
        .expect("native service parser runs");
    assert_eq!(invalid.status.code(), Some(1));
    assert!(invalid.stdout.is_empty());
    assert_eq!(invalid.stderr, b"Error: invalid port 'not-a-port'\n");
}

#[test]
fn absent_unit_status_and_top_level_up_have_exact_failures() {
    let home = tempfile::tempdir().expect("temporary home opens");
    std::fs::create_dir_all(home.path().join("journal")).expect("create journal root");
    // `down` must inspect the fixed OS service manager even when this HOME has
    // no unit, so its result legitimately depends on the caller's live runtime.
    // Its parser contract and the stop decision table are covered separately.
    for (arguments, expected_stdout) in [
        (
            &["service", "status"][..],
            "service: not installed\nrun 'journal setup' or 'journal service install' to install it.\n",
        ),
        (&["up"][..], ""),
    ] {
        let output = Command::new(BINARY)
            .args(arguments)
            .env("HOME", home.path())
            .env("PATH", "/definitely-not-a-service-manager")
            .output()
            .expect("native service command runs");
        assert_eq!(output.status.code(), Some(1), "{arguments:?}");
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            expected_stdout,
            "{arguments:?}"
        );
        if arguments == ["up"] {
            assert_eq!(
                output.stderr,
                b"error: service not installed. run 'journal service install' first.\n"
            );
        } else {
            assert!(output.stderr.is_empty());
        }
    }
}

#[test]
fn service_install_recovery_keeps_checksum_mismatch_byte_exact() {
    let journal = tempfile::Builder::new()
        .prefix("solstone-service-checksum-journal-")
        .tempdir_in("/var/tmp")
        .expect("temporary journal");
    let home = installation_binding::admit_for(journal.path());
    installation_binding::corrupt_admitted_record_checksum(&home);

    let output = service_install_output(Path::new(BINARY), &home);

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(
        output.stderr,
        b"this installation couldn't be verified.\n\
run `journal setup` to check it. if setup finishes successfully, try again.\n\
details: could not verify the saved installation: identity record checksum mismatch\n"
    );
}

#[test]
fn service_install_recovery_sanitizes_hostile_executable_paths() {
    let cases = [
        ("newline-control", "newline-\n-\x1b"),
        ("backslash", "backslash-\\-component"),
    ];
    for (name, component) in cases {
        let (_temporary, binary) = hostile_binary(component);
        let home = tempfile::Builder::new()
            .prefix("solstone-service-hostile-home-")
            .tempdir_in("/var/tmp")
            .expect("absolute hostile home");
        let output = service_install_output(&binary, home.path());
        assert_eq!(output.status.code(), Some(1), "{name}");
        assert!(output.stdout.is_empty(), "{name}");
        let details = recovery_details(&output.stderr);
        assert!(
            details.starts_with("could not find the solstone installation from "),
            "{name}: {details}"
        );
        match name {
            "newline-control" => {
                assert!(details.contains("\\n"), "{details}");
                assert!(details.contains("\\x1b"), "{details}");
                assert!(!details.contains('\n'), "{details}");
                assert!(!details.contains('\x1b'), "{details}");
            }
            "backslash" => {
                assert!(details.contains("\\\\"), "{details}");
                assert!(!details.contains("\\\\\\\\"), "{details}");
            }
            _ => unreachable!("known hostile path fixture"),
        }
    }

    let temporary = tempfile::Builder::new()
        .prefix("solstone-service-hostile-executable-")
        .tempdir_in("/var/tmp")
        .expect("isolated oversized binary root");
    let escaped_component = "\x1b".repeat(240);
    let mut binary_dir = temporary.path().to_path_buf();
    for _ in 0..6 {
        binary_dir.push(&escaped_component);
    }
    let binary = copied_binary(&binary_dir);
    let home = tempfile::Builder::new()
        .prefix("solstone-service-hostile-home-")
        .tempdir_in("/var/tmp")
        .expect("absolute hostile home");
    let output = service_install_output(&binary, home.path());
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let details = recovery_details(&output.stderr);
    assert!(details.ends_with(TRUNCATION_MARKER));
    assert_eq!(details.chars().count(), 2048);
}
