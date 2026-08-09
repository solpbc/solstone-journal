// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The live Python reference still agrees with the frozen overflow oracle.
//!
//! ⚠ This is the *faithfulness* half of the freeze, and it can only be run
//! while the reference exists. The Rust implementation is checked against the
//! recorded answers in `frozen_seam_oracles.rs`, which needs no interpreter and
//! runs in `make ci`. This target exists so that, for as long as the Python is
//! still here, a drift between it and the recording is caught rather than
//! assumed away.
//!
//! ⛔ When the conversion deletes `providers/local`'s generate half, this target
//! goes with it -- deliberately. Its replacement is already green.

use std::path::PathBuf;
use std::process::Command;

use serde_json::Value;

const OVERFLOW_ORACLE: &str = include_str!("../../../fixtures/endpoint_overflow_oracle.json");

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("repository root")
        .to_path_buf()
}

fn python() -> PathBuf {
    let venv = repository_root().join(".venv/bin/python3");
    assert!(venv.is_file(), "differential requires make install");
    venv
}

fn python_decision(body: &str, served_window: Option<u32>, attempt: u32) -> (String, Option<u32>) {
    let script = concat!(
        "import json, os, sys\n",
        "sys.path.insert(0, os.environ['SOLSTONE_REPO_ROOT'])\n",
        "from solstone.think.providers.local import _endpoint_overflow_decision\n",
        "window = None if sys.argv[2] == 'none' else int(sys.argv[2])\n",
        "decision = _endpoint_overflow_decision(sys.argv[1], window, int(sys.argv[3]))\n",
        "print(json.dumps([decision.kind, decision.max_tokens]))\n",
    );
    let output = Command::new(python())
        .args([
            "-c",
            script,
            body,
            &served_window.map_or_else(|| "none".into(), |value| value.to_string()),
            &attempt.to_string(),
        ])
        .env("SOLSTONE_REPO_ROOT", repository_root())
        .output()
        .expect("run Python overflow decision");
    assert!(
        output.status.success(),
        "Python stderr: {:?}",
        String::from_utf8_lossy(&output.stderr)
    );
    let decision: Value = serde_json::from_slice(&output.stdout).expect("Python decision JSON");
    (
        decision[0].as_str().expect("kind").to_owned(),
        decision[1].as_u64().map(|value| value as u32),
    )
}

#[test]
fn the_frozen_oracle_still_matches_the_live_reference() {
    let document: Value = serde_json::from_str(OVERFLOW_ORACLE).expect("oracle fixture parses");
    let cases = document["cases"].as_array().expect("cases");
    assert!(!cases.is_empty(), "the frozen corpus is empty");

    for case in cases {
        let name = case["name"].as_str().expect("case name");
        let (kind, max_tokens) = python_decision(
            case["body"].as_str().expect("body"),
            case["served_window"].as_u64().map(|value| value as u32),
            case["attempt"].as_u64().expect("attempt") as u32,
        );
        assert_eq!(
            kind,
            case["kind"].as_str().expect("recorded kind"),
            "case={name}: the recording no longer matches the reference"
        );
        assert_eq!(
            max_tokens,
            case["max_tokens"].as_u64().map(|value| value as u32),
            "case={name}: the recording no longer matches the reference"
        );
    }
}
