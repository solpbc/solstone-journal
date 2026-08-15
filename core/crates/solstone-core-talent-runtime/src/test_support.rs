// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(test)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

/// Install a one-shot v2 response stub and return its executable path.
pub fn one_shot_stub(root: &std::path::Path, text: &str) -> PathBuf {
    let path = root.join("one-shot-stub.sh");
    let response = serde_json::json!({
        "schema":"solstone-generate-response-v2", "id":null,
        "outcome":"generated", "text":text, "model":"test-model", "usage":{},
        "finish_reason":"stop", "thinking":null, "schema_validation":null,
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
