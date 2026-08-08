// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::PathBuf;
use std::process::Command;

use serde_json::{Value, json};
use solstone_core_generate_wire::validate_schema_with_annotations;

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
        output.stderr
    );
    serde_json::from_slice(&output.stdout).expect("Python verdict JSON")
}

fn rust_verdict(text: &str, schema: &Value) -> Value {
    let result = validate_schema_with_annotations(text, schema);
    json!({"text": result.text, "validation": result.validation})
}

// Messages intentionally differ between jsonschema and Python jsonschema. Raw text is also
// excluded because truncation permits different valid JSON formatting.
fn comparable(verdict: Value) -> Value {
    let validation = verdict
        .get("validation")
        .cloned()
        .expect("validation is present");
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
fn schema_validation_matches_python_except_messages() {
    let cases = vec![
        (
            "valid",
            r#"{"field":"ok"}"#,
            json!({"type": "object", "properties": {"field": {"type": "string"}}, "required": ["field"]}),
        ),
        (
            "type",
            r#"{"field":"bad"}"#,
            json!({"type": "object", "properties": {"field": {"type": "integer"}}}),
        ),
        (
            "required",
            "{}",
            json!({"type": "object", "required": ["field"]}),
        ),
        (
            "nested",
            r#"{"outer":{"inner":"bad"}}"#,
            json!({"type": "object", "properties": {"outer": {"type": "object", "properties": {"inner": {"type": "integer"}}}}}),
        ),
        (
            "array-element",
            r#"{"items":[1,"bad"]}"#,
            json!({"type": "object", "properties": {"items": {"type": "array", "items": {"type": "integer"}}}}),
        ),
        ("unparseable", "{", json!({"type": "object"})),
        (
            "uncompilable-schema",
            r#"{"field":"ok"}"#,
            json!({"type": "not-a-real-type"}),
        ),
        (
            "truncation",
            r#"{"word":"four"}"#,
            json!({"type": "object", "properties": {"word": {"type": "string", "maxLength": 3, "x-truncate": true}}}),
        ),
        (
            "unicode-truncation",
            r#"{"word":"éééé"}"#,
            json!({"type": "object", "properties": {"word": {"type": "string", "maxLength": 3, "x-truncate": true}}}),
        ),
        (
            "reference-hidden",
            r#"{"word":"four"}"#,
            json!({"$defs": {"word": {"type": "string", "maxLength": 3, "x-truncate": true}}, "properties": {"word": {"$ref": "#/$defs/word"}}}),
        ),
        (
            "all-of-hidden",
            r#"{"word":"four"}"#,
            json!({"type": "object", "properties": {"word": {"allOf": [{"type": "string", "maxLength": 3, "x-truncate": true}]}}}),
        ),
        (
            "pattern-properties-hidden",
            r#"{"word":"four"}"#,
            json!({"type": "object", "patternProperties": {"^word$": {"type": "string", "maxLength": 3, "x-truncate": true}}}),
        ),
        (
            "prefix-items-hidden",
            r#"["four"]"#,
            json!({"type": "array", "prefixItems": [{"type": "string", "maxLength": 3, "x-truncate": true}]}),
        ),
    ];
    for (name, text, schema) in cases {
        assert_eq!(
            comparable(rust_verdict(text, &schema)),
            comparable(python_verdict(text, &schema)),
            "case={name}"
        );
    }
}
