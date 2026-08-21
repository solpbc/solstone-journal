// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use solstone_core_backup::{BackupError, Destination, assemble_backend_env};

use crate::runner::{ToolRunner, reason_for_returncode, run_restic};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DestinationStatus {
    pub reachable: bool,
    pub repo_exists: bool,
    pub reason_code: &'static str,
    pub message: &'static str,
}
pub fn validate_destination(
    runner: &dyn ToolRunner,
    destination: &Destination,
    password: &str,
    restic_path: &Path,
    timeout: Option<Duration>,
) -> Result<DestinationStatus, BackupError> {
    let raw = assemble_backend_env(destination)?;
    let backend = raw
        .into_iter()
        .map(|(key, value)| {
            (
                key,
                value
                    .as_str()
                    .map(|value| Some(value.to_owned()))
                    .unwrap_or(None),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let result = run_restic(
        runner,
        &["cat".into(), "config".into()],
        &destination.repository,
        password,
        restic_path,
        Some(&backend),
        false,
        None,
        timeout,
        &[],
    )
    .map_err(|_| BackupError::InvalidDestinationShape)?;
    Ok(match result.returncode {
        0 => DestinationStatus {
            reachable: true,
            repo_exists: true,
            reason_code: "repo_exists",
            message: "backup repository is reachable",
        },
        10 => DestinationStatus {
            reachable: true,
            repo_exists: false,
            reason_code: "repo_missing",
            message: "backup destination is reachable and needs setup",
        },
        12 => DestinationStatus {
            reachable: true,
            repo_exists: true,
            reason_code: "auth_failed",
            message: "repository password was rejected",
        },
        11 => DestinationStatus {
            reachable: true,
            repo_exists: true,
            reason_code: "locked",
            message: "repository is locked; try again shortly",
        },
        124 => DestinationStatus {
            reachable: false,
            repo_exists: false,
            reason_code: "timeout",
            message: "could not reach the backup destination",
        },
        _ => {
            let _ = reason_for_returncode(result.returncode);
            DestinationStatus {
                reachable: false,
                repo_exists: false,
                reason_code: "unreachable",
                message: "could not reach the backup destination",
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::{ToolOutput, ToolRequest};
    use serde_json::json;
    use std::cell::RefCell;
    use std::io;

    struct Fixture {
        code: i32,
        requests: RefCell<Vec<ToolRequest<'static>>>,
    }
    impl ToolRunner for Fixture {
        fn run(&self, request: &ToolRequest<'_>) -> io::Result<ToolOutput> {
            self.requests.borrow_mut().push(ToolRequest {
                program: request.program.clone(),
                argv: request.argv.clone(),
                env: request.env.clone(),
                timeout: request.timeout,
                pass_fds: vec![],
            });
            Ok(ToolOutput {
                returncode: self.code,
                stdout: vec![],
                stderr: vec![],
            })
        }
    }
    fn destination() -> Destination {
        Destination {
            repository: "repo".into(),
            backend: "s3".into(),
            credentials: serde_json::from_value(
                json!({"access_key_id":"ACCESS","secret_access_key":"SECRET"}),
            )
            .unwrap(),
        }
    }

    #[test]
    fn maps_reference_statuses_and_uses_stored_backend_values() {
        for (code, reason, reachable, exists) in [
            (0, "repo_exists", true, true),
            (10, "repo_missing", true, false),
            (12, "auth_failed", true, true),
            (11, "locked", true, true),
            (124, "timeout", false, false),
            (99, "unreachable", false, false),
        ] {
            let runner = Fixture {
                code,
                requests: RefCell::new(vec![]),
            };
            let status = validate_destination(
                &runner,
                &destination(),
                "PASSWORD",
                Path::new("/fixture/bin/restic"),
                None,
            )
            .unwrap();
            assert_eq!(
                (status.reason_code, status.reachable, status.repo_exists),
                (reason, reachable, exists)
            );
            let request = runner.requests.borrow_mut().pop().unwrap();
            assert_eq!(
                request
                    .env
                    .get(&std::ffi::OsString::from("AWS_ACCESS_KEY_ID"))
                    .unwrap(),
                "ACCESS"
            );
            assert_eq!(
                request
                    .env
                    .get(&std::ffi::OsString::from("AWS_SECRET_ACCESS_KEY"))
                    .unwrap(),
                "SECRET"
            );
        }
    }
}
