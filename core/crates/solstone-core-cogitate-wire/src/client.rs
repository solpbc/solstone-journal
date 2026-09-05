// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::env;
use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;

use crate::CogitateRequest;

#[derive(Debug)]
pub enum ClientError {
    Resolve(String),
    Io(String),
    Protocol { status: i32, stderr: String },
    Decode(String),
}

#[derive(Clone)]
pub struct CogitateOneShotClient {
    executable: PathBuf,
    prefix_arguments: Vec<OsString>,
    environment: BTreeMap<String, String>,
}

pub struct CogitateOneShotRun {
    pub events: Vec<Value>,
}

impl CogitateOneShotClient {
    pub fn at_path(path: impl Into<PathBuf>) -> Self {
        Self {
            executable: path.into(),
            prefix_arguments: Vec::new(),
            environment: BTreeMap::new(),
        }
    }

    pub fn with_prefix_arguments(mut self, arguments: impl IntoIterator<Item = OsString>) -> Self {
        self.prefix_arguments.extend(arguments);
        self
    }

    pub fn with_env(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.environment.insert(name.into(), value.into());
        self
    }

    pub fn sibling() -> Result<Self, ClientError> {
        let current =
            env::current_exe().map_err(|error| ClientError::Resolve(error.to_string()))?;
        let parent = current
            .parent()
            .ok_or_else(|| ClientError::Resolve("current executable has no parent".to_owned()))?;
        let path = parent.join("solstone-core");
        if Path::new(&path).is_file() {
            Ok(Self::at_path(path))
        } else {
            Err(ClientError::Resolve(format!(
                "missing sibling executable {}",
                path.display()
            )))
        }
    }

    pub fn execute(&self, request: &CogitateRequest) -> Result<CogitateOneShotRun, ClientError> {
        let input = serde_json::to_vec(&request.to_value())
            .map_err(|error| ClientError::Decode(error.to_string()))?;
        let mut child = Command::new(&self.executable)
            .args(&self.prefix_arguments)
            .arg("--one-shot")
            .envs(&self.environment)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| ClientError::Io(error.to_string()))?;
        child
            .stdin
            .as_mut()
            .ok_or_else(|| ClientError::Io("wire stdin is unavailable".to_owned()))?
            .write_all(&input)
            .map_err(|error| ClientError::Io(error.to_string()))?;
        let output = child
            .wait_with_output()
            .map_err(|error| ClientError::Io(error.to_string()))?;
        if !output.status.success() {
            return Err(ClientError::Protocol {
                status: output.status.code().unwrap_or(-1),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            });
        }
        let stdout = std::str::from_utf8(&output.stdout)
            .map_err(|error| ClientError::Decode(error.to_string()))?;
        let mut events = Vec::new();
        for line in stdout.lines() {
            if line.trim().is_empty() {
                continue;
            }
            events.push(
                serde_json::from_str(line)
                    .map_err(|error| ClientError::Decode(error.to_string()))?,
            );
        }
        Ok(CogitateOneShotRun { events })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CogitateRequest, REQUEST_SCHEMA};
    use serde_json::json;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    fn fixture_request() -> CogitateRequest {
        CogitateRequest::from_value(&json!({
            "schema": REQUEST_SCHEMA,
            "access_tier": "normal",
            "max_turns": 4,
            "timeout_ms": 30_000,
            "read_call_budget": 5,
            "model": "fixture-model",
            "correlation_id": "corr-1",
            "initial_prompt": "Do the task.",
            "journal_root": "/var/tmp/solstone-cogitate-client-test",
            "diagnostic": false,
            "dry_run": false
        }))
        .expect("fixture request is valid")
    }

    fn test_root() -> PathBuf {
        let root = PathBuf::from("/var/tmp").join(format!(
            "solstone-cogitate-client-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn stub(root: &std::path::Path, body: &str) -> PathBuf {
        let path = root.join("cogitate-one-shot-stub.sh");
        fs::write(&path, body).unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&path, permissions).unwrap();
        path
    }

    #[test]
    fn to_value_round_trips_through_from_value() {
        let request = fixture_request();
        let parsed = CogitateRequest::from_value(&request.to_value()).unwrap();
        assert_eq!(parsed, request);
        assert!(request.to_value()["diagnostic"].is_boolean());
        assert!(request.to_value()["dry_run"].is_boolean());
    }

    #[test]
    fn execute_collects_distinct_ndjson_events() {
        let root = test_root();
        let path = stub(
            &root,
            "#!/bin/sh\n[ \"$1\" = --one-shot ] || exit 92\ncat >/dev/null\nprintf '%s\\n' '{\"event\":\"tool_start\",\"tool\":\"solstone\"}' '{\"event\":\"tool_end\",\"tool\":\"solstone\"}' '{\"event\":\"finish\",\"terminal\":true,\"result\":\"done\"}'\n",
        );
        let run = CogitateOneShotClient::at_path(path)
            .execute(&fixture_request())
            .expect("stub succeeds");
        assert_eq!(
            run.events
                .iter()
                .filter_map(|event| event.get("event").and_then(Value::as_str))
                .collect::<Vec<_>>(),
            ["tool_start", "tool_end", "finish"]
        );
    }

    #[test]
    fn execute_rejects_malformed_ndjson_and_nonzero_exit() {
        let root = test_root();
        let decode = CogitateOneShotClient::at_path(stub(
            &root,
            "#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' 'not-json'\n",
        ))
        .execute(&fixture_request());
        assert!(matches!(decode, Err(ClientError::Decode(_))));
        let protocol =
            CogitateOneShotClient::at_path(stub(&root, "#!/bin/sh\ncat >/dev/null\nexit 7\n"))
                .execute(&fixture_request());
        assert!(matches!(
            protocol,
            Err(ClientError::Protocol { status: 7, .. })
        ));
    }
}
