// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(test)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// Set only by the install probe; a stub run with it exits before its body.
#[cfg(all(test, feature = "full-tests"))]
const PROBE_ENV: &str = "SOLSTONE_TEST_STUB_PROBE";

/// Write an executable `/bin/sh` stub. Under `full-tests`, where stubs run,
/// return only once it can be executed.
pub fn install_stub(path: &Path, script: &str) {
    let body = script
        .strip_prefix("#!/bin/sh\n")
        .expect("stub is a /bin/sh script");
    fs::write(
        path,
        format!("#!/bin/sh\n[ -z \"$SOLSTONE_TEST_STUB_PROBE\" ] || exit 0\n{body}"),
    )
    .unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(path, permissions).unwrap();
    #[cfg(all(test, feature = "full-tests"))]
    wait_until_executable(path);
}

/// A test on another thread that forks while this one holds the file open for
/// writing keeps that descriptor in its child until the child execs, and an
/// exec in that window fails with ETXTBSY ("Text file busy"). Probing here,
/// as `solstone-core-system/tests/fixture_binary.rs` does, keeps the race out
/// of every test that runs a stub.
#[cfg(all(test, feature = "full-tests"))]
fn wait_until_executable(path: &Path) {
    use std::io::ErrorKind;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match Command::new(path)
            .env(PROBE_ENV, "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
        {
            Ok(status) if status.success() => return,
            Ok(status) => panic!("stub probe exited with {status}"),
            Err(error)
                if error.kind() == ErrorKind::ExecutableFileBusy && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("stub probe failed: {error}"),
        }
    }
}

/// Install a one-shot v2 response stub and return its executable path.
pub fn one_shot_stub(root: &std::path::Path, text: &str) -> PathBuf {
    one_shot_stub_with_schema_validation(root, text, serde_json::Value::Null)
}

#[cfg(all(test, feature = "full-tests"))]
/// Install a stub that only accepts `generate --one-shot`.
pub fn generate_one_shot_stub(root: &std::path::Path, text: &str) -> PathBuf {
    let path = root.join("generate-one-shot-stub.sh");
    let response = serde_json::json!({
        "schema":"solstone-generate-response-v2", "id":null,
        "outcome":"generated", "text":text, "model":"test-model", "usage":{},
        "finish_reason":"stop", "thinking":null, "schema_validation":null,
        "input_budget":null, "request_budget":null, "inference":null,
    });
    install_stub(
        &path,
        &format!(
            "#!/bin/sh\n[ \"$1\" = generate ] && [ \"$2\" = --one-shot ] || exit 92\ncat >/dev/null\nprintf '%s\\n' '{}'\n",
            response
        ),
    );
    path
}

/// Install a one-shot v2 response stub with the supplied schema annotation.
pub fn one_shot_stub_with_schema_validation(
    root: &std::path::Path,
    text: &str,
    schema_validation: serde_json::Value,
) -> PathBuf {
    one_shot_stub_with(root, text, schema_validation, serde_json::Value::Null)
}

#[cfg(all(test, feature = "full-tests"))]
/// Install a one-shot v2 response stub reporting the supplied input budget.
pub fn one_shot_stub_with_input_budget(
    root: &std::path::Path,
    text: &str,
    input_budget: serde_json::Value,
) -> PathBuf {
    one_shot_stub_with(root, text, serde_json::Value::Null, input_budget)
}

fn one_shot_stub_with(
    root: &std::path::Path,
    text: &str,
    schema_validation: serde_json::Value,
    input_budget: serde_json::Value,
) -> PathBuf {
    let path = root.join("one-shot-stub.sh");
    let response = serde_json::json!({
        "schema":"solstone-generate-response-v2", "id":null,
        "outcome":"generated", "text":text, "model":"test-model", "usage":{},
        "finish_reason":"stop", "thinking":null, "schema_validation":schema_validation,
        "input_budget":input_budget, "request_budget":null, "inference":null,
    });
    install_stub(
        &path,
        &format!("#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '{}'\n", response),
    );
    path
}

#[cfg(all(test, feature = "full-tests"))]
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
    let response = refused_response_value(reason_code, retryable, blocking, provider, detail);
    install_stub(
        &path,
        &format!("#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '{}'\n", response),
    );
    path
}

/// Construct a Generated response JSON value for stubs.
pub fn generated_response_value(
    text: &str,
    schema_validation: serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "schema": "solstone-generate-response-v2",
        "id": null,
        "outcome": "generated",
        "text": text,
        "model": "test-model",
        "usage": {},
        "finish_reason": "stop",
        "thinking": null,
        "schema_validation": schema_validation,
        "input_budget": null,
        "request_budget": null,
        "inference": null,
    })
}

/// Construct a Refused response JSON value for stubs.
pub fn refused_response_value(
    reason_code: Option<&str>,
    retryable: bool,
    blocking: bool,
    provider: &str,
    detail: &str,
) -> serde_json::Value {
    serde_json::json!({
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
    })
}

#[cfg(all(test, feature = "full-tests"))]
/// Install a one-shot v2 sequenced response stub and return its executable path.
pub fn sequenced_one_shot_stub(root: &std::path::Path, responses: &[serde_json::Value]) -> PathBuf {
    let script_path = root.join("sequenced-one-shot-stub.sh");
    for (index, response) in responses.iter().enumerate() {
        let resp_path = root.join(format!("sequenced-one-shot-stub.sh.response_{}", index + 1));
        fs::write(&resp_path, serde_json::to_string(response).unwrap()).unwrap();
    }
    let script = String::from(
        "#!/bin/sh\n\
        count_file=\"$0.count\"\n\
        count=$(cat \"$count_file\" 2>/dev/null || echo 0)\n\
        count=$((count + 1))\n\
        echo \"$count\" > \"$count_file\"\n\
        cat > \"$0.request_$count\"\n\
        resp_file=\"$0.response_$count\"\n\
        if [ -f \"$resp_file\" ]; then\n\
            cat \"$resp_file\"\n\
            printf '\\n'\n\
        else\n\
            exit 93\n\
        fi\n",
    );
    install_stub(&script_path, &script);
    script_path
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use std::io::ErrorKind;
    use std::process::Command;

    use super::*;

    #[test]
    fn install_stub_waits_out_a_child_holding_the_file_open_for_writing() {
        let root = tempfile::Builder::new()
            .prefix("solstone-talent-stub-busy-")
            .tempdir_in("/var/tmp")
            .unwrap();
        let path = root.path().join("stub.sh");
        let script = "#!/bin/sh\nprintf ok\n";
        fs::write(&path, script).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        let writer = fs::OpenOptions::new().write(true).open(&path).unwrap();
        let mut holder = Command::new("sleep")
            .arg("0.3")
            .stdout(writer)
            .spawn()
            .unwrap();
        let busy = Command::new(&path).output().unwrap_err();
        assert_eq!(busy.kind(), ErrorKind::ExecutableFileBusy);

        install_stub(&path, script);
        let output = Command::new(&path).output().unwrap();
        assert_eq!(output.stdout, b"ok");
        holder.wait().unwrap();
    }
}
