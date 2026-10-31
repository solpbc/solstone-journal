// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Change an own-storage destination while backup stays on.
//!
//! The new destination is checked before anything is saved. New credentials
//! for the same repository must open the existing backup. A different
//! repository needs the owner's confirmation first, because the snapshots in
//! the current one stay there and this journal stops using them; then it must
//! be a bucket that already exists, so a mistyped bucket never becomes a new
//! one. Nothing is deleted on either path.

use std::path::Path;
use std::time::Duration;

use solstone_core_backup::{
    BackupError, Destination, destination_from_config, get_backup_config, get_keys, set_destination,
};

use crate::destination::{DestinationStatus, validate_destination};
use crate::repo::{RepoError, init_repository};
use crate::runner::{ResticInvocation, ToolRunner, reason_for_returncode};

const EDIT_CHECK: Duration = Duration::from_secs(60);

/// What an edit changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DestinationChange {
    /// New credentials for the repository already in use.
    Credentials,
    /// A different repository (or backend).
    Repository,
}

impl DestinationChange {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Credentials => "credentials",
            Self::Repository => "repository",
        }
    }
}

#[derive(Debug)]
pub enum DestinationEditRefusal {
    /// Backup isn't on with own storage, so there is nothing to edit.
    NotEditable,
    /// The new destination is the one already saved.
    Unchanged,
    /// A different repository needs the owner's confirmation first.
    NeedsConfirmation,
    /// The new destination can't be used. Nothing was saved.
    Unusable(DestinationStatus),
    /// Starting a backup at the new repository failed. Nothing was saved.
    SetupFailed(&'static str),
    Config(BackupError),
}

/// Classify an edit against the destination currently saved.
pub fn destination_change(current: &Destination, next: &Destination) -> DestinationChange {
    if current.repository == next.repository && current.backend == next.backend {
        DestinationChange::Credentials
    } else {
        DestinationChange::Repository
    }
}

/// Check and, only if usable, save a new destination for an own-storage
/// backup that is on. Returns the change and the destination's status.
pub fn edit_destination<'invocation>(
    runner: &dyn ToolRunner,
    journal: &Path,
    next: &Destination,
    restic: impl Into<ResticInvocation<'invocation>>,
    confirm_new_repository: bool,
) -> Result<(DestinationChange, DestinationStatus), DestinationEditRefusal> {
    let restic = restic.into();
    let config = get_backup_config(journal).map_err(DestinationEditRefusal::Config)?;
    let current = destination_from_config(&config);
    let keys = get_keys(journal).map_err(DestinationEditRefusal::Config)?;
    let (Some(current), Some(keys)) = (current, keys) else {
        return Err(DestinationEditRefusal::NotEditable);
    };
    if config.get("enabled") != Some(&serde_json::Value::Bool(true))
        || config.get("mode").and_then(serde_json::Value::as_str) != Some("byo")
    {
        return Err(DestinationEditRefusal::NotEditable);
    }
    if current == *next {
        return Err(DestinationEditRefusal::Unchanged);
    }
    let change = destination_change(&current, next);
    if change == DestinationChange::Repository && !confirm_new_repository {
        return Err(DestinationEditRefusal::NeedsConfirmation);
    }
    // restic retries a refused S3 signature for many minutes, so the check is
    // bounded and a refusal it can't name reads as a timeout. Nothing is saved.
    let status = validate_destination(runner, next, &keys.daily_key, restic, Some(EDIT_CHECK))
        .map_err(DestinationEditRefusal::Config)?;
    match (change, status.reason_code) {
        (DestinationChange::Credentials, "repo_exists") => {}
        (DestinationChange::Repository, "repo_exists" | "repo_missing") => {
            // Starts a backup there, or confirms the recovery key opens the one
            // already there. It never creates a bucket: a missing bucket was
            // refused above as bucket_not_found.
            init_repository(
                runner,
                next,
                &keys.daily_key,
                &keys.recovery_key,
                restic,
                None,
            )
            .map_err(|error| {
                DestinationEditRefusal::SetupFailed(match error {
                    RepoError::Key(key) => reason_for_returncode(key.returncode),
                    _ => "failed",
                })
            })?;
        }
        // A backup is already there that this journal's key can't open. It
        // belongs to someone or something else: never set up over it.
        (DestinationChange::Repository, "auth_failed") if status.repo_exists => {
            return Err(DestinationEditRefusal::Unusable(DestinationStatus {
                reachable: true,
                repo_exists: true,
                reason_code: "repository_key_refused",
                message: "a backup there doesn't open with this journal's key",
            }));
        }
        _ => return Err(DestinationEditRefusal::Unusable(status)),
    }
    set_destination(journal, next).map_err(DestinationEditRefusal::Config)?;
    Ok((change, status))
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::io;
    use std::path::Path;

    use serde_json::json;
    use solstone_core_backup::{
        generate_and_store_keys, get_destination, set_destination, set_enabled, set_mode,
    };

    use super::*;
    use crate::runner::{ToolOutput, ToolRequest};

    /// restic as seen by an edit: `cat config` answers `open` until `init` runs.
    struct Restic {
        open: i32,
        stderr: &'static str,
        initialized: Cell<bool>,
        commands: RefCell<Vec<String>>,
    }

    impl Restic {
        fn new(open: i32, stderr: &'static str) -> Self {
            Self {
                open,
                stderr,
                initialized: Cell::new(false),
                commands: RefCell::new(vec![]),
            }
        }
        fn ran(&self, command: &str) -> bool {
            self.commands.borrow().iter().any(|ran| ran == command)
        }
    }

    impl ToolRunner for Restic {
        fn run(&self, request: &ToolRequest<'_>) -> io::Result<ToolOutput> {
            let argv: Vec<String> = request
                .argv
                .iter()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect();
            let command = ["init", "cat", "key"]
                .into_iter()
                .find(|command| argv.iter().any(|arg| arg == command))
                .unwrap_or("other");
            self.commands.borrow_mut().push(command.to_owned());
            let returncode = match command {
                "init" => {
                    self.initialized.set(true);
                    0
                }
                "cat" if self.initialized.get() => 0,
                "cat" => self.open,
                _ => 0,
            };
            Ok(ToolOutput {
                returncode,
                stdout: vec![],
                stderr: self.stderr.as_bytes().to_vec(),
            })
        }
    }

    fn destination(repository: &str, key: &str) -> Destination {
        Destination {
            repository: repository.into(),
            backend: "s3".into(),
            credentials: serde_json::from_value(
                json!({"access_key_id": key, "secret_access_key": "SECRET"}),
            )
            .unwrap(),
        }
    }

    fn journal_with_backup_on() -> tempfile::TempDir {
        let journal = tempfile::tempdir().unwrap();
        generate_and_store_keys(journal.path()).unwrap();
        set_destination(journal.path(), &destination("s3:host/bucket/old", "OLD")).unwrap();
        set_mode(journal.path(), "byo").unwrap();
        set_enabled(journal.path(), true).unwrap();
        journal
    }

    fn edit(
        journal: &Path,
        restic: &Restic,
        next: &Destination,
        confirm: bool,
    ) -> Result<(DestinationChange, DestinationStatus), DestinationEditRefusal> {
        edit_destination(restic, journal, next, Path::new("/fixture/restic"), confirm)
    }

    #[test]
    fn a_key_rotation_saves_only_when_the_new_key_opens_the_same_backup() {
        let journal = journal_with_backup_on();
        let rotated = destination("s3:host/bucket/old", "NEW");

        let refused = Restic::new(12, "");
        assert!(matches!(
            edit(journal.path(), &refused, &rotated, false),
            Err(DestinationEditRefusal::Unusable(DestinationStatus {
                reason_code: "auth_failed",
                ..
            }))
        ));
        assert_eq!(
            get_destination(journal.path())
                .unwrap()
                .unwrap()
                .credentials["access_key_id"],
            "OLD"
        );

        let opens = Restic::new(0, "");
        let (change, _) = edit(journal.path(), &opens, &rotated, false).unwrap();
        assert_eq!(change, DestinationChange::Credentials);
        assert!(!opens.ran("init"));
        assert!(get_destination(journal.path()).unwrap().unwrap() == rotated);
    }

    #[test]
    fn a_different_repository_needs_confirmation_before_any_tool_runs() {
        let journal = journal_with_backup_on();
        let restic = Restic::new(10, "");
        assert!(matches!(
            edit(
                journal.path(),
                &restic,
                &destination("s3:host/bucket/new", "OLD"),
                false
            ),
            Err(DestinationEditRefusal::NeedsConfirmation)
        ));
        assert!(restic.commands.borrow().is_empty());
        assert_eq!(
            get_destination(journal.path()).unwrap().unwrap().repository,
            "s3:host/bucket/old"
        );
    }

    #[test]
    fn a_confirmed_repository_starts_a_backup_there_but_never_creates_a_bucket() {
        let journal = journal_with_backup_on();
        let typo = Destination {
            repository: "b2:typo:journal".into(),
            backend: "b2".into(),
            credentials: serde_json::from_value(json!({"account_id":"ID","account_key":"KEY"}))
                .unwrap(),
        };
        let missing_bucket = Restic::new(
            10,
            "Fatal: repository does not exist at b2:typo:journal: repository does not exist\n",
        );
        assert!(matches!(
            edit(journal.path(), &missing_bucket, &typo, true),
            Err(DestinationEditRefusal::Unusable(DestinationStatus {
                reason_code: "bucket_not_found",
                ..
            }))
        ));
        assert!(!missing_bucket.ran("init"));
        assert_eq!(
            get_destination(journal.path()).unwrap().unwrap().repository,
            "s3:host/bucket/old"
        );

        let empty_bucket = Restic::new(10, "");
        let moved = destination("s3:host/bucket/new", "OLD");
        let (change, _) = edit(journal.path(), &empty_bucket, &moved, true).unwrap();
        assert_eq!(change, DestinationChange::Repository);
        assert!(empty_bucket.ran("init"));
        assert!(get_destination(journal.path()).unwrap().unwrap() == moved);
    }

    #[test]
    fn nothing_is_edited_unless_own_storage_backup_is_on() {
        let journal = journal_with_backup_on();
        set_enabled(journal.path(), false).unwrap();
        let restic = Restic::new(0, "");
        assert!(matches!(
            edit(
                journal.path(),
                &restic,
                &destination("s3:host/bucket/old", "NEW"),
                false
            ),
            Err(DestinationEditRefusal::NotEditable)
        ));
        set_enabled(journal.path(), true).unwrap();
        set_mode(journal.path(), "operated").unwrap();
        assert!(matches!(
            edit(
                journal.path(),
                &restic,
                &destination("s3:host/bucket/old", "NEW"),
                false
            ),
            Err(DestinationEditRefusal::NotEditable)
        ));
        assert!(restic.commands.borrow().is_empty());
    }

    #[test]
    fn a_backup_this_key_cannot_open_is_never_set_up_over_and_a_no_op_runs_nothing() {
        let journal = journal_with_backup_on();
        let theirs = Restic::new(12, "");
        assert!(matches!(
            edit(
                journal.path(),
                &theirs,
                &destination("s3:host/bucket/theirs", "OLD"),
                true
            ),
            Err(DestinationEditRefusal::Unusable(DestinationStatus {
                reason_code: "repository_key_refused",
                ..
            }))
        ));
        assert!(!theirs.ran("init") && !theirs.ran("key"));
        let restic = Restic::new(0, "");
        assert!(matches!(
            edit(
                journal.path(),
                &restic,
                &destination("s3:host/bucket/old", "OLD"),
                false
            ),
            Err(DestinationEditRefusal::Unchanged)
        ));
        assert!(restic.commands.borrow().is_empty());
    }
}
