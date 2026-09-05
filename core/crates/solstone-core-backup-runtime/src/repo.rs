// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::fmt;
#[cfg(unix)]
use std::os::fd::{AsFd, AsRawFd};
use std::path::Path;
use std::time::Duration;

use solstone_core_backup::{BackupError, Destination, assemble_backend_env};

use crate::destination::validate_destination;
use crate::runner::{PassedHandle, ToolRunner, run_restic};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResticKeyError {
    pub returncode: i32,
}
impl fmt::Display for ResticKeyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "restic key operation failed with returncode {}",
            self.returncode
        )
    }
}
impl std::error::Error for ResticKeyError {}
#[derive(Debug)]
pub enum RepoError {
    Backup(BackupError),
    Key(ResticKeyError),
    Failed,
}
impl From<BackupError> for RepoError {
    fn from(error: BackupError) -> Self {
        Self::Backup(error)
    }
}

fn backend(destination: &Destination) -> Result<BTreeMap<String, Option<String>>, BackupError> {
    Ok(assemble_backend_env(destination)?
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
        .collect())
}
pub fn init_repository(
    runner: &dyn ToolRunner,
    destination: &Destination,
    daily_key: &str,
    recovery_key: &str,
    restic_path: &Path,
    timeout: Option<Duration>,
) -> Result<(), RepoError> {
    let daily = validate_destination(runner, destination, daily_key, restic_path, timeout)?;
    if daily.reason_code == "repo_missing" {
        run(
            runner,
            &["init".into()],
            destination,
            daily_key,
            restic_path,
            timeout,
            &[],
        )?;
        add_recovery_key(
            runner,
            destination,
            daily_key,
            recovery_key,
            restic_path,
            timeout,
        )?;
        return verify_recovery_key(runner, destination, recovery_key, restic_path, timeout);
    }
    if daily.reason_code == "repo_exists" {
        let recovery =
            validate_destination(runner, destination, recovery_key, restic_path, timeout)?;
        if recovery.repo_exists && recovery.reason_code == "repo_exists" {
            return Ok(());
        }
        if recovery.reason_code == "auth_failed" {
            add_recovery_key(
                runner,
                destination,
                daily_key,
                recovery_key,
                restic_path,
                timeout,
            )?;
            return verify_recovery_key(runner, destination, recovery_key, restic_path, timeout);
        }
    }
    Err(RepoError::Failed)
}
pub fn add_recovery_key(
    runner: &dyn ToolRunner,
    destination: &Destination,
    daily_key: &str,
    recovery_key: &str,
    restic_path: &Path,
    timeout: Option<Duration>,
) -> Result<(), RepoError> {
    #[cfg(unix)]
    {
        let (reader, writer) = nix::unistd::pipe().map_err(|_| RepoError::Failed)?;
        let mut writer = std::fs::File::from(writer);
        use std::io::Write;
        writer
            .write_all(format!("{recovery_key}\n").as_bytes())
            .map_err(|_| RepoError::Failed)?;
        drop(writer);
        let fd = reader.as_raw_fd();
        run(
            runner,
            &[
                "key".into(),
                "add".into(),
                "--new-password-file".into(),
                format!("/dev/fd/{fd}"),
            ],
            destination,
            daily_key,
            restic_path,
            timeout,
            &[reader.as_fd()],
        )?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = (
            runner,
            destination,
            daily_key,
            recovery_key,
            restic_path,
            timeout,
        );
        Err(RepoError::Failed)
    }
}
pub fn capture_current_key_id(
    runner: &dyn ToolRunner,
    destination: &Destination,
    password: &str,
    restic_path: &Path,
    timeout: Option<Duration>,
) -> Result<String, RepoError> {
    let result = run_restic(
        runner,
        &["key".into(), "list".into()],
        &destination.repository,
        password,
        restic_path,
        Some(&backend(destination)?),
        true,
        None,
        timeout,
        &[],
    )
    .map_err(|_| RepoError::Failed)?;
    if result.returncode != 0 {
        return Err(RepoError::Key(ResticKeyError {
            returncode: result.returncode,
        }));
    }
    let records = result
        .json
        .as_ref()
        .and_then(serde_json::Value::as_array)
        .ok_or(RepoError::Failed)?;
    let current = records
        .iter()
        .find(|item| item.get("current") == Some(&serde_json::Value::Bool(true)))
        .ok_or(RepoError::Failed)?;
    current
        .get("id")
        .and_then(serde_json::Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .ok_or(RepoError::Failed)
}
pub fn remove_key(
    runner: &dyn ToolRunner,
    destination: &Destination,
    password: &str,
    key_id: &str,
    restic_path: &Path,
    timeout: Option<Duration>,
) -> Result<(), RepoError> {
    run(
        runner,
        &["key".into(), "remove".into(), key_id.into()],
        destination,
        password,
        restic_path,
        timeout,
        &[],
    )
}
fn verify_recovery_key(
    runner: &dyn ToolRunner,
    destination: &Destination,
    recovery_key: &str,
    restic_path: &Path,
    timeout: Option<Duration>,
) -> Result<(), RepoError> {
    let status = validate_destination(runner, destination, recovery_key, restic_path, timeout)?;
    (status.repo_exists && status.reason_code == "repo_exists")
        .then_some(())
        .ok_or(RepoError::Failed)
}
fn run(
    runner: &dyn ToolRunner,
    args: &[String],
    destination: &Destination,
    password: &str,
    restic_path: &Path,
    timeout: Option<Duration>,
    pass_fds: &[PassedHandle<'_>],
) -> Result<(), RepoError> {
    let result = run_restic(
        runner,
        args,
        &destination.repository,
        password,
        restic_path,
        Some(&backend(destination)?),
        false,
        None,
        timeout,
        pass_fds,
    )
    .map_err(|_| RepoError::Failed)?;
    if result.returncode == 0 {
        Ok(())
    } else {
        Err(RepoError::Key(ResticKeyError {
            returncode: result.returncode,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::{ToolOutput, ToolRequest};
    use serde_json::json;
    use std::cell::RefCell;
    use std::io;

    struct Script {
        codes: RefCell<Vec<i32>>,
        commands: RefCell<Vec<Vec<String>>>,
    }
    impl ToolRunner for Script {
        fn run(&self, request: &ToolRequest<'_>) -> io::Result<ToolOutput> {
            self.commands.borrow_mut().push(
                request
                    .argv
                    .iter()
                    .map(|arg| arg.to_string_lossy().to_string())
                    .collect(),
            );
            Ok(ToolOutput {
                returncode: self.codes.borrow_mut().remove(0),
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
    fn missing_repository_initializes_adds_and_verifies_in_order() {
        let runner = Script {
            codes: RefCell::new(vec![10, 0, 0, 0]),
            commands: RefCell::new(vec![]),
        };
        init_repository(
            &runner,
            &destination(),
            "daily",
            "recovery",
            Path::new("/fixture/bin/restic"),
            None,
        )
        .unwrap();
        let commands = runner.commands.borrow();
        assert_eq!(
            commands
                .iter()
                .map(|args| args[0].as_str())
                .collect::<Vec<_>>(),
            vec!["cat", "init", "key", "cat"]
        );
        assert!(
            commands
                .iter()
                .all(|args| args[0] != "key" || args.get(1) != Some(&"remove".into()))
        );
    }

    #[test]
    fn existing_repository_only_adds_when_recovery_auth_fails() {
        let valid = Script {
            codes: RefCell::new(vec![0, 0]),
            commands: RefCell::new(vec![]),
        };
        init_repository(
            &valid,
            &destination(),
            "daily",
            "recovery",
            Path::new("/fixture/bin/restic"),
            None,
        )
        .unwrap();
        assert!(
            !valid
                .commands
                .borrow()
                .iter()
                .any(|args| args.get(1) == Some(&"add".into()))
        );
        let repair = Script {
            codes: RefCell::new(vec![0, 12, 0, 0]),
            commands: RefCell::new(vec![]),
        };
        init_repository(
            &repair,
            &destination(),
            "daily",
            "recovery",
            Path::new("/fixture/bin/restic"),
            None,
        )
        .unwrap();
        assert!(
            repair
                .commands
                .borrow()
                .iter()
                .any(|args| args.get(1) == Some(&"add".into()))
        );
        let refused = Script {
            codes: RefCell::new(vec![0, 11]),
            commands: RefCell::new(vec![]),
        };
        assert!(
            init_repository(
                &refused,
                &destination(),
                "daily",
                "recovery",
                Path::new("/fixture/bin/restic"),
                None
            )
            .is_err()
        );
        assert!(
            !refused
                .commands
                .borrow()
                .iter()
                .any(|args| args.get(1) == Some(&"add".into()))
        );
    }

    #[test]
    fn failures_keep_prior_remote_phases_and_never_roll_back() {
        let add_failure = Script {
            codes: RefCell::new(vec![10, 0, 12]),
            commands: RefCell::new(vec![]),
        };
        assert!(
            init_repository(
                &add_failure,
                &destination(),
                "daily",
                "recovery",
                Path::new("/fixture/bin/restic"),
                None
            )
            .is_err()
        );
        assert_eq!(
            add_failure
                .commands
                .borrow()
                .iter()
                .map(|args| args[0].as_str())
                .collect::<Vec<_>>(),
            vec!["cat", "init", "key"]
        );
        let verify_failure = Script {
            codes: RefCell::new(vec![10, 0, 0, 12]),
            commands: RefCell::new(vec![]),
        };
        assert!(
            init_repository(
                &verify_failure,
                &destination(),
                "daily",
                "recovery",
                Path::new("/fixture/bin/restic"),
                None
            )
            .is_err()
        );
        assert_eq!(
            verify_failure
                .commands
                .borrow()
                .iter()
                .map(|args| args[0].as_str())
                .collect::<Vec<_>>(),
            vec!["cat", "init", "key", "cat"]
        );
    }
}
