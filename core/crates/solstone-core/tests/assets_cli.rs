// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::process::Command;

#[test]
fn assets_command_emits_the_complete_round_trippable_catalog() {
    let output = Command::new(env!("CARGO_BIN_EXE_solstone-core"))
        .arg("assets")
        .output()
        .expect("assets command executes");
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let emitted: Vec<serde_json::Value> =
        serde_json::from_slice(&output.stdout).expect("assets JSON");
    let expected = serde_json::to_value(solstone_core_assets::catalog())
        .unwrap()
        .as_array()
        .unwrap()
        .to_vec();
    assert_eq!(emitted, expected);
    assert!(emitted.iter().any(|row| row["origin_key"]
        == "runtimes/llama-cuda13/b10068/llama-b10068-bin-linux-cuda13-amd64-sol1.tar.gz"));
    assert!(emitted.iter().any(|row| row["origin_key"]
        == "runtimes/llama-cuda13/b10068/llama-b10068-bin-linux-cuda13-arm64-sol1.tar.gz"));
    assert!(emitted.iter().all(|row| row["unit"] != "mlx-snapshot"));
}

#[test]
fn assets_rejects_arguments_with_standard_usage() {
    let output = Command::new(env!("CARGO_BIN_EXE_solstone-core"))
        .args(["assets", "unexpected"])
        .output()
        .expect("assets command executes");
    assert_eq!(output.status.code(), Some(64));
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        solstone_core_cli::USAGE
    );
}
