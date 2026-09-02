// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(test)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

/// Install a one-shot v2 response stub and return its executable path.
pub fn one_shot_stub(root: &std::path::Path, text: &str) -> PathBuf {
    one_shot_stub_with_schema_validation(root, text, serde_json::Value::Null)
}

/// Install a stub that only accepts `cogitate --one-shot` and prints NDJSON events.
pub fn cogitate_one_shot_stub(root: &std::path::Path, events: &[&str]) -> PathBuf {
    let path = root.join("cogitate-one-shot-stub.sh");
    let mut script = String::from(
        "#!/bin/sh\n[ \"$1\" = cogitate ] && [ \"$2\" = --one-shot ] || exit 92\ncat >/dev/null\n",
    );
    for event in events {
        script.push_str("printf '%s\\n' '");
        script.push_str(event);
        script.push_str("'\n");
    }
    fs::write(&path, script).unwrap();
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&path, permissions).unwrap();
    path
}

/// Install a stub that only accepts `generate --one-shot`.
pub fn generate_one_shot_stub(root: &std::path::Path, text: &str) -> PathBuf {
    let path = root.join("generate-one-shot-stub.sh");
    let response = serde_json::json!({
        "schema":"solstone-generate-response-v2", "id":null,
        "outcome":"generated", "text":text, "model":"test-model", "usage":{},
        "finish_reason":"stop", "thinking":null, "schema_validation":null,
        "input_budget":null, "request_budget":null, "inference":null,
    });
    fs::write(
        &path,
        format!(
            "#!/bin/sh\n[ \"$1\" = generate ] && [ \"$2\" = --one-shot ] || exit 92\ncat >/dev/null\nprintf '%s\\n' '{}'\n",
            response
        ),
    )
    .unwrap();
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&path, permissions).unwrap();
    path
}

/// Install a one-shot v2 response stub with the supplied schema annotation.
pub fn one_shot_stub_with_schema_validation(
    root: &std::path::Path,
    text: &str,
    schema_validation: serde_json::Value,
) -> PathBuf {
    let path = root.join("one-shot-stub.sh");
    let response = serde_json::json!({
        "schema":"solstone-generate-response-v2", "id":null,
        "outcome":"generated", "text":text, "model":"test-model", "usage":{},
        "finish_reason":"stop", "thinking":null, "schema_validation":schema_validation,
        "input_budget":null, "request_budget":null, "inference":null,
    });
    fs::write(
        &path,
        format!("#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '{}'\n", response),
    )
    .unwrap();
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&path, permissions).unwrap();
    path
}

/// Install a one-shot v2 refused-response stub and return its executable path.
pub fn refused_one_shot_stub(
    root: &std::path::Path,
    reason_code: Option<&str>,
    retryable: bool,
    blocking: bool,
    provider: &str,
    detail: &str,
) -> PathBuf {
    let path = root.join("refused-one-shot-stub.sh");
    let response = serde_json::json!({
        "schema": "solstone-generate-response-v2",
        "id": null,
        "outcome": "refused",
        "reason": "provider-response-invalid",
        "reason_code": reason_code,
        "retryable": retryable,
        "blocking": blocking,
        "reset_at_ms": null,
        "provider": provider,
        "detail": detail,
    });
    fs::write(
        &path,
        format!("#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '{}'\n", response),
    )
    .unwrap();
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&path, permissions).unwrap();
    path
}
