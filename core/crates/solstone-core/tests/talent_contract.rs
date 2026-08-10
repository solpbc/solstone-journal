// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::process::Command;

use serde_json::Value;

#[test]
fn talent_contract_reports_all_tiers_and_bound_tools() {
    let output = Command::new(env!("CARGO_BIN_EXE_solstone-core"))
        .args(["cogitate", "--talent-contract"])
        .output()
        .expect("run talent contract");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let contract: Value = serde_json::from_slice(&output.stdout).expect("talent contract JSON");
    assert_eq!(
        contract["journal_commands"],
        serde_json::json!(["identity", "health", "talent"])
    );
    assert_eq!(
        contract["finalization_modes"],
        serde_json::json!(["emit_final", "FinishTool"])
    );

    let tiers = contract["tiers"].as_array().expect("tiers array");
    assert_eq!(tiers.len(), 5);
    for (name, sol, reads, submit, talent_facing, tools) in [
        (
            "normal",
            true,
            true,
            false,
            true,
            serde_json::json!(["sol", "read_file", "list_directory", "glob", "grep_search"]),
        ),
        (
            "system-read",
            true,
            true,
            false,
            true,
            serde_json::json!(["sol", "read_file", "list_directory", "glob", "grep_search"]),
        ),
        (
            "outbound",
            true,
            false,
            true,
            true,
            serde_json::json!(["sol"]),
        ),
        (
            "synthesis",
            true,
            false,
            false,
            true,
            serde_json::json!(["sol"]),
        ),
        (
            "diagnostic",
            false,
            false,
            false,
            false,
            serde_json::json!([]),
        ),
    ] {
        let tier = tiers
            .iter()
            .find(|tier| tier["name"] == name)
            .expect("named tier");
        assert_eq!(tier["talent_facing"], talent_facing, "{name}");
        assert_eq!(tier["sol"], sol, "{name}");
        assert_eq!(tier["reads"], reads, "{name}");
        assert_eq!(tier["submit"], submit, "{name}");
        assert_eq!(tier["tools"], tools, "{name}");
    }
}
