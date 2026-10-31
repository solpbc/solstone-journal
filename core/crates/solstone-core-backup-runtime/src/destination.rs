// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
#[cfg(test)]
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
pub fn validate_destination<'invocation>(
    runner: &dyn ToolRunner,
    destination: &Destination,
    password: &str,
    restic_path: impl Into<crate::runner::ResticInvocation<'invocation>>,
    timeout: Option<Duration>,
) -> Result<DestinationStatus, BackupError> {
    let restic_path = restic_path.into();
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
        // restic reports a B2 bucket that doesn't exist as no repository too,
        // and `init` would then create a bucket named after a typo. Only a
        // missing config file inside a bucket that exists means needs setup.
        10 if destination.backend == "b2"
            && !result.stderr.contains("unable to open config file") =>
        {
            DestinationStatus {
                reachable: true,
                repo_exists: false,
                reason_code: "bucket_not_found",
                message: "these credentials can't open a bucket by that name",
            }
        }
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
            open_failure_status(&result.stderr)
        }
    })
}

// restic exits 1 for every repository it can't open, so only its own words
// tell a destination it reached and was refused by from one it never reached.
// A B2 key scoped to one bucket is refused at list_buckets when the repository
// names any other bucket, which is how a key's name typed as the bucket's
// shows up. A key that can list every bucket never gets here: restic reports
// a missing bucket as no repository (10), handled above. Rejected B2
// credentials are refused one step earlier.
fn open_failure_status(stderr: &str) -> DestinationStatus {
    if stderr.contains("b2_authorize_account: 401") {
        return DestinationStatus {
            reachable: true,
            repo_exists: false,
            reason_code: "auth_failed",
            message: "the destination rejected the credentials",
        };
    }
    if stderr.contains("b2_list_buckets: 401") {
        return DestinationStatus {
            reachable: true,
            repo_exists: false,
            reason_code: "bucket_not_found",
            message: "these credentials can't open a bucket by that name",
        };
    }
    DestinationStatus {
        reachable: false,
        repo_exists: false,
        reason_code: "unreachable",
        message: "could not reach the backup destination",
    }
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
        stderr: &'static str,
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
                stdin: request.stdin.clone(),
                #[cfg(windows)]
                resources: request.resources.clone(),
            });
            Ok(ToolOutput {
                returncode: self.code,
                stdout: vec![],
                stderr: self.stderr.as_bytes().to_vec(),
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
                stderr: "",
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

    // Captured from restic 0.19.0 against Backblaze B2: a key scoped to one
    // bucket, with the repository naming another bucket (the key's own name);
    // then with credentials B2 doesn't know; then an S3 host that doesn't
    // resolve.
    #[test]
    fn a_wrong_bucket_and_rejected_credentials_are_not_unreachable() {
        for (stderr, reason, reachable) in [
            (
                "Fatal: unable to open repository at b2:solstone-journal-key:journal: Bucket: b2_list_buckets: 401: \n",
                "bucket_not_found",
                true,
            ),
            (
                "Fatal: unable to open repository at b2:no-such-bucket:journal: b2.NewClient: b2_authorize_account: 401:\n",
                "auth_failed",
                true,
            ),
            (
                "Stat(<config/>) returned error, retrying after 588.80128ms: Stat: Get \"https://s3.example.invalid/bucket/?location=\": dial tcp: lookup s3.example.invalid on 127.0.0.53:53: no such host\n",
                "unreachable",
                false,
            ),
        ] {
            let runner = Fixture {
                code: 1,
                stderr,
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
                (reason, reachable, false)
            );
        }
    }

    // Captured from restic 0.19.0 against Backblaze B2 with a key that can
    // reach every bucket, read-only `cat config`, 2026-10-31. Both exit 10;
    // only the second is a bucket that exists with nothing in it yet, and only
    // that one may be set up. `init` on the first creates the mistyped bucket.
    #[test]
    fn a_missing_b2_bucket_is_never_read_as_needs_setup() {
        let b2 = Destination {
            repository: "b2:typo:journal".into(),
            backend: "b2".into(),
            credentials: serde_json::from_value(json!({"account_id":"ID","account_key":"KEY"}))
                .unwrap(),
        };
        for (destination, stderr, reason) in [
            (
                &b2,
                "Fatal: repository does not exist at b2:solstone-nonexistent-probe-7f3a91:journal: repository does not exist\n",
                "bucket_not_found",
            ),
            (
                &b2,
                "Fatal: repository does not exist: unable to open config file: Stat: b2_download_file_by_name: 404: \nIs there a repository at the following location?\nb2:extro-backup-bd4a90de:nonexistent-probe-path\n",
                "repo_missing",
            ),
            (
                &destination(),
                "Fatal: repository does not exist: unable to open config file: Stat: The specified key does not exist.\nIs there a repository at the following location?\ns3:s3.us-west-004.backblazeb2.com/extro-backup-bd4a90de/nonexistent-probe-path\n",
                "repo_missing",
            ),
        ] {
            let runner = Fixture {
                code: 10,
                stderr,
                requests: RefCell::new(vec![]),
            };
            let status = validate_destination(
                &runner,
                destination,
                "PASSWORD",
                Path::new("/fixture/bin/restic"),
                None,
            )
            .unwrap();
            assert_eq!((status.reason_code, status.repo_exists), (reason, false));
        }
    }
}
