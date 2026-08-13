// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::process::Command;

const BINARY: &str = env!("CARGO_BIN_EXE_solstone-core");

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
fn absent_unit_status_and_top_level_up_down_have_exact_failures() {
    let home = tempfile::tempdir().expect("temporary home opens");
    for (arguments, expected_stdout) in [
        (
            &["service", "status"][..],
            "service: not installed\nrun 'journal setup' or 'journal service install' to install it.\n",
        ),
        (&["up"][..], ""),
        (&["down"][..], ""),
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
        if arguments == ["up"] || arguments == ["down"] {
            assert_eq!(
                output.stderr,
                b"error: service not installed. run 'journal service install' first.\n"
            );
        } else {
            assert!(output.stderr.is_empty());
        }
    }
}
