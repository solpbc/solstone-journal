// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The live Python reference still agrees with the frozen schema-validation oracle.
//!
//! ⚠ This is the *faithfulness* half of the freeze, and it can only be run
//! while the reference exists. The Rust implementation is checked against the
//! recorded answers in `frozen_seam_oracles.rs`, which needs no interpreter and
//! runs in `make ci`.
//!
//! ⛔ When the conversion deletes `models`' generate half, this target goes with
//! it -- deliberately. Its replacement is already green.

use std::path::PathBuf;
use std::process::Command;

use serde_json::{Value, json};

const SCHEMA_ORACLE: &str = include_str!("../../../fixtures/schema_validation_oracle.json");

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

fn python_verdict(text: &str, schema: &Value) -> Value {
    let script = concat!(
        "import json, os, sys\n",
        "sys.path.insert(0, os.environ['SOLSTONE_REPO_ROOT'])\n",
        "from solstone.think.models import _validate_schema_with_annotations\n",
        "text, validation = _validate_schema_with_annotations(sys.argv[1], json.loads(sys.argv[2]))\n",
        "print(json.dumps({'text': text, 'validation': validation}, ensure_ascii=False))\n",
    );
    let output = Command::new(python())
        .args(["-c", script, text, &schema.to_string()])
        .env("SOLSTONE_REPO_ROOT", repository_root())
        .output()
        .expect("run Python schema validator");
    assert!(
        output.status.success(),
        "Python stderr: {:?}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("Python verdict JSON")
}

/// Messages intentionally differ between the Rust and Python jsonschema
/// implementations. Raw text is compared separately because truncation permits
/// different valid JSON formatting.
fn comparable(validation: &Value) -> Value {
    let mut validation = validation
        .as_object()
        .expect("validation is an object")
        .clone();
    let errors = validation
        .get("errors")
        .and_then(Value::as_array)
        .expect("errors is an array")
        .iter()
        .map(|error| {
            let error = error.as_object().expect("error is an object");
            json!({"path": error["path"], "constraint": error["constraint"]})
        })
        .collect::<Vec<_>>();
    validation.insert("errors".to_owned(), Value::Array(errors));
    Value::Object(validation)
}

#[test]
fn the_frozen_oracle_still_matches_the_live_reference() {
    let document: Value = serde_json::from_str(SCHEMA_ORACLE).expect("oracle fixture parses");
    let cases = document["cases"].as_array().expect("cases");
    assert!(!cases.is_empty(), "the frozen corpus is empty");

    for case in cases {
        let name = case["name"].as_str().expect("case name");
        let verdict = python_verdict(case["text"].as_str().expect("text"), &case["schema"]);

        // ⚠ The recording carries the reference's messages verbatim, so this
        // half compares the full verdict the consumers compare, not less.
        assert_eq!(
            comparable(&verdict["validation"]),
            comparable(&case["validation"]),
            "case={name}: the recording no longer matches the reference"
        );
        assert_eq!(
            verdict["text"], case["observed_text"],
            "case={name}: the reference's own output text changed"
        );
    }
}
