// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::env;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::{
    GenerateRequest, GenerateResponse, ProtocolError, decode_one_shot_response,
    decode_protocol_error, encode_one_shot_request,
};

#[derive(Debug)]
pub enum ClientError {
    Resolve(String),
    Io(String),
    Protocol(ProtocolError),
    Decode(String),
}

#[derive(Clone)]
pub struct OneShotClient {
    executable: PathBuf,
    prefix_arguments: Vec<std::ffi::OsString>,
    environment: BTreeMap<String, String>,
}

impl OneShotClient {
    pub fn at_path(path: impl Into<PathBuf>) -> Self {
        Self {
            executable: path.into(),
            prefix_arguments: Vec::new(),
            environment: BTreeMap::new(),
        }
    }

    pub fn with_prefix_arguments(
        mut self,
        arguments: impl IntoIterator<Item = std::ffi::OsString>,
    ) -> Self {
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

    pub fn execute(&self, request: &GenerateRequest) -> Result<GenerateResponse, ClientError> {
        let input = encode_one_shot_request(request).map_err(ClientError::Decode)?;
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
            .write_all(input.as_bytes())
            .map_err(|error| ClientError::Io(error.to_string()))?;
        let output = child
            .wait_with_output()
            .map_err(|error| ClientError::Io(error.to_string()))?;
        if output.status.success() {
            decode_one_shot_response(
                std::str::from_utf8(&output.stdout)
                    .map_err(|error| ClientError::Decode(error.to_string()))?,
            )
            .map_err(ClientError::Decode)
        } else {
            Err(ClientError::Protocol(
                decode_protocol_error(
                    std::str::from_utf8(&output.stderr)
                        .map_err(|error| ClientError::Decode(error.to_string()))?,
                )
                .map_err(ClientError::Decode)?,
            ))
        }
    }
}
