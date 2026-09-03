// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Removal of retired journal log aliases without following their targets.

use std::error::Error;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs::{self, Metadata};
use std::io;
use std::path::{Component, Path, PathBuf};

use chrono::NaiveDate;
#[cfg(windows)]
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
};

use crate::errors::PathError;
use crate::journal_root::JournalEntryKind;
use crate::paths::{DirEntry, DirEntryKind, is_day_key, list_dir_entries};
#[cfg(windows)]
use crate::windows_identity::{WindowsFileIdentity, file_identity};

/// The expected shape of one retired alias.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LegacyAliasTarget {
    /// A former day-local managed-process alias pointing to its run payload.
    ManagedDayLog { alias_name: OsString },
    /// A former root managed-process alias pointing into a valid day partition.
    ManagedRootLog { alias_name: OsString },
    /// A former talent-run alias pointing to a completed numeric use.
    TalentRun { talent_directory: OsString },
}

/// Why an alias candidate was deliberately preserved.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyAliasRefusal {
    /// The candidate is not a symbolic link.
    NotSymlink(JournalEntryKind),
    /// The caller's expected alias leaf did not match the observed leaf.
    WrongLeafName,
    /// A talent-run alias did not have the retired relative target shape.
    UnexpectedTarget,
    /// The entry is a link, but not a file symlink that the retired writers made.
    UnsupportedLinkKind,
    /// The observed link was replaced before it could be removed.
    ChangedBeforeRemoval,
}

/// The result of attempting to remove one observed alias.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyAliasDisposition {
    /// The alias leaf was removed.
    Removed,
    /// The alias was absent when observed or immediately before removal.
    AlreadyAbsent,
    /// The candidate was preserved for the stated reason.
    Refused(LegacyAliasRefusal),
}

/// The result of inspecting one candidate alias path.
#[derive(Debug, Eq, PartialEq)]
pub enum LegacyAliasObservationResult {
    /// The candidate did not exist.
    Absent,
    /// The candidate is a validated alias ready for revalidation and removal.
    Observed(LegacyAliasObservation),
    /// The candidate was not a removable retired alias.
    Refused(LegacyAliasRefusal),
}

/// A refusal recorded by a complete cleanup pass.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LegacyAliasRefused {
    /// Candidate path preserved by the cleanup pass.
    pub path: PathBuf,
    /// Reason the path was preserved.
    pub reason: LegacyAliasRefusal,
}

/// Summary of a complete retired-alias cleanup pass.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LegacyAliasCleanupReport {
    /// Number of retired aliases removed.
    pub removed: usize,
    /// Number of observed aliases that disappeared before removal.
    pub already_absent: usize,
    /// Candidates intentionally preserved, including their paths and reasons.
    pub refused: Vec<LegacyAliasRefused>,
}

/// Failure while inspecting or removing retired aliases.
#[derive(Debug)]
pub enum LegacyAliasCleanupError {
    /// A direct directory listing failed.
    List { path: PathBuf, source: PathError },
    /// A no-follow filesystem operation failed.
    Io {
        operation: &'static str,
        path: PathBuf,
        source: io::Error,
    },
}

impl fmt::Display for LegacyAliasCleanupError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::List { path, source } => write!(formatter, "{}: {source}", path.display()),
            Self::Io {
                operation,
                path,
                source,
            } => write!(formatter, "{operation} {}: {source}", path.display()),
        }
    }
}

impl Error for LegacyAliasCleanupError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::List { source, .. } => Some(source),
            Self::Io { source, .. } => Some(source),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum NoFollowIdentity {
    #[cfg(unix)]
    Unix {
        dev: u64,
        ino: u64,
        kind: JournalEntryKind,
    },
    #[cfg(windows)]
    Windows { identity: WindowsFileIdentity },
}

/// A validated alias observation. Its fields stay private so the path, identity,
/// and raw target spelling cannot be separated by a caller.
#[derive(Debug, Eq, PartialEq)]
pub struct LegacyAliasObservation {
    path: PathBuf,
    parent: PathBuf,
    parent_identity: NoFollowIdentity,
    identity: NoFollowIdentity,
    target: PathBuf,
}

impl LegacyAliasObservation {
    /// Candidate path retained for diagnostics.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Inspect one exact alias path without following the leaf.
pub fn observe_legacy_alias_symlink(
    path: &Path,
    expected: &LegacyAliasTarget,
) -> Result<LegacyAliasObservationResult, LegacyAliasCleanupError> {
    let Some((parent, parent_identity)) = nofollow_parent_identity(path)? else {
        return Ok(LegacyAliasObservationResult::Absent);
    };
    let Some(metadata) = nofollow_metadata(path)? else {
        return Ok(LegacyAliasObservationResult::Absent);
    };
    let kind = journal_entry_kind(&metadata);
    if !is_symbolic_link(&metadata) {
        return Ok(LegacyAliasObservationResult::Refused(
            LegacyAliasRefusal::NotSymlink(kind),
        ));
    }
    if !is_file_symbolic_link(&metadata) {
        return Ok(LegacyAliasObservationResult::Refused(
            LegacyAliasRefusal::UnsupportedLinkKind,
        ));
    }
    if !leaf_matches(path, expected) {
        return Ok(LegacyAliasObservationResult::Refused(
            LegacyAliasRefusal::WrongLeafName,
        ));
    }
    let target = fs::read_link(path).map_err(|source| LegacyAliasCleanupError::Io {
        operation: "read retired alias target",
        path: path.to_path_buf(),
        source,
    })?;
    if !target_matches(&target, expected) {
        return Ok(LegacyAliasObservationResult::Refused(
            LegacyAliasRefusal::UnexpectedTarget,
        ));
    }
    let Some(identity) = nofollow_identity(path, &metadata)? else {
        return Ok(LegacyAliasObservationResult::Absent);
    };
    if !parent_identity_matches(&parent, &parent_identity)? {
        return Ok(LegacyAliasObservationResult::Refused(
            LegacyAliasRefusal::ChangedBeforeRemoval,
        ));
    }
    Ok(LegacyAliasObservationResult::Observed(
        LegacyAliasObservation {
            path: path.to_path_buf(),
            parent,
            parent_identity,
            identity,
            target,
        },
    ))
}

/// Revalidate then remove an alias leaf without resolving its target.
pub fn remove_observed_legacy_alias_symlink(
    observed: &LegacyAliasObservation,
) -> Result<LegacyAliasDisposition, LegacyAliasCleanupError> {
    if !parent_identity_matches(&observed.parent, &observed.parent_identity)? {
        return Ok(LegacyAliasDisposition::Refused(
            LegacyAliasRefusal::ChangedBeforeRemoval,
        ));
    }
    let Some(metadata) = nofollow_metadata(&observed.path)? else {
        return Ok(LegacyAliasDisposition::AlreadyAbsent);
    };
    if !is_file_symbolic_link(&metadata) {
        return Ok(LegacyAliasDisposition::Refused(
            LegacyAliasRefusal::ChangedBeforeRemoval,
        ));
    }
    let Some(identity) = nofollow_identity(&observed.path, &metadata)? else {
        return Ok(LegacyAliasDisposition::AlreadyAbsent);
    };
    if identity != observed.identity {
        return Ok(LegacyAliasDisposition::Refused(
            LegacyAliasRefusal::ChangedBeforeRemoval,
        ));
    }
    let target = match fs::read_link(&observed.path) {
        Ok(target) => target,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(LegacyAliasDisposition::AlreadyAbsent);
        }
        Err(source) => {
            return Err(LegacyAliasCleanupError::Io {
                operation: "re-read retired alias target",
                path: observed.path.clone(),
                source,
            });
        }
    };
    if target != observed.target {
        return Ok(LegacyAliasDisposition::Refused(
            LegacyAliasRefusal::ChangedBeforeRemoval,
        ));
    }
    if !parent_identity_matches(&observed.parent, &observed.parent_identity)? {
        return Ok(LegacyAliasDisposition::Refused(
            LegacyAliasRefusal::ChangedBeforeRemoval,
        ));
    }
    let Some(metadata) = nofollow_metadata(&observed.path)? else {
        return Ok(LegacyAliasDisposition::AlreadyAbsent);
    };
    let Some(identity) = nofollow_identity(&observed.path, &metadata)? else {
        return Ok(LegacyAliasDisposition::AlreadyAbsent);
    };
    if !is_file_symbolic_link(&metadata) || identity != observed.identity {
        return Ok(LegacyAliasDisposition::Refused(
            LegacyAliasRefusal::ChangedBeforeRemoval,
        ));
    }
    match fs::remove_file(&observed.path) {
        Ok(()) => Ok(LegacyAliasDisposition::Removed),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            Ok(LegacyAliasDisposition::AlreadyAbsent)
        }
        Err(source) => Err(LegacyAliasCleanupError::Io {
            operation: "remove retired alias",
            path: observed.path.clone(),
            source,
        }),
    }
}

/// Remove all retired aliases in the four declared journal namespaces.
pub fn cleanup_legacy_log_aliases(
    journal: &Path,
) -> Result<LegacyAliasCleanupReport, LegacyAliasCleanupError> {
    let mut report = LegacyAliasCleanupReport::default();
    cleanup_managed_log_aliases(&journal.join("health"), true, &mut report)?;

    for day in nofollow_directory_entries(&journal.join("chronicle"))? {
        if day.kind != DirEntryKind::Directory || !is_valid_day(&day.name) {
            continue;
        }
        let health = nofollow_directory_entries(&day.path)?
            .into_iter()
            .find(|entry| {
                entry.name == OsStr::new("health") && entry.kind == DirEntryKind::Directory
            });
        if let Some(health) = health {
            cleanup_managed_log_aliases(&health.path, false, &mut report)?;
        }
    }

    cleanup_talent_run_aliases(&journal.join("talents"), &mut report)?;
    cleanup_talent_run_aliases(&journal.join("agents"), &mut report)?;
    Ok(report)
}

fn cleanup_managed_log_aliases(
    directory: &Path,
    root_alias: bool,
    report: &mut LegacyAliasCleanupReport,
) -> Result<(), LegacyAliasCleanupError> {
    for entry in nofollow_directory_entries(directory)? {
        if Path::new(&entry.name).extension() != Some(OsStr::new("log")) {
            continue;
        }
        let expected = if root_alias {
            LegacyAliasTarget::ManagedRootLog {
                alias_name: entry.name,
            }
        } else {
            LegacyAliasTarget::ManagedDayLog {
                alias_name: entry.name,
            }
        };
        apply_candidate(&entry.path, &expected, report)?;
    }
    Ok(())
}

fn cleanup_talent_run_aliases(
    directory: &Path,
    report: &mut LegacyAliasCleanupReport,
) -> Result<(), LegacyAliasCleanupError> {
    for entry in nofollow_directory_entries(directory)? {
        let Some(talent_directory) = talent_directory_from_alias(&entry.name) else {
            continue;
        };
        apply_candidate(
            &entry.path,
            &LegacyAliasTarget::TalentRun { talent_directory },
            report,
        )?;
    }
    Ok(())
}

fn apply_candidate(
    path: &Path,
    expected: &LegacyAliasTarget,
    report: &mut LegacyAliasCleanupReport,
) -> Result<(), LegacyAliasCleanupError> {
    match observe_legacy_alias_symlink(path, expected)? {
        LegacyAliasObservationResult::Absent => report.already_absent += 1,
        LegacyAliasObservationResult::Observed(observed) => {
            record_disposition(
                path,
                remove_observed_legacy_alias_symlink(&observed)?,
                report,
            );
        }
        LegacyAliasObservationResult::Refused(reason) => record_refusal(path, reason, report),
    }
    Ok(())
}

fn record_disposition(
    path: &Path,
    disposition: LegacyAliasDisposition,
    report: &mut LegacyAliasCleanupReport,
) {
    match disposition {
        LegacyAliasDisposition::Removed => report.removed += 1,
        LegacyAliasDisposition::AlreadyAbsent => report.already_absent += 1,
        LegacyAliasDisposition::Refused(reason) => record_refusal(path, reason, report),
    }
}

fn record_refusal(path: &Path, reason: LegacyAliasRefusal, report: &mut LegacyAliasCleanupReport) {
    report.refused.push(LegacyAliasRefused {
        path: path.to_path_buf(),
        reason,
    });
}

fn nofollow_directory_entries(directory: &Path) -> Result<Vec<DirEntry>, LegacyAliasCleanupError> {
    let Some(metadata) = nofollow_metadata(directory)? else {
        return Ok(Vec::new());
    };
    if journal_entry_kind(&metadata) != JournalEntryKind::Directory {
        return Ok(Vec::new());
    }
    list_dir_entries(directory).map_err(|source| LegacyAliasCleanupError::List {
        path: directory.to_path_buf(),
        source,
    })
}

fn nofollow_metadata(path: &Path) -> Result<Option<Metadata>, LegacyAliasCleanupError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(LegacyAliasCleanupError::Io {
            operation: "inspect retired alias",
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn nofollow_parent_identity(
    path: &Path,
) -> Result<Option<(PathBuf, NoFollowIdentity)>, LegacyAliasCleanupError> {
    let Some(parent) = path.parent() else {
        return Ok(None);
    };
    let Some(metadata) = nofollow_metadata(parent)? else {
        return Ok(None);
    };
    if journal_entry_kind(&metadata) != JournalEntryKind::Directory {
        return Ok(None);
    }
    let Some(identity) = nofollow_identity(parent, &metadata)? else {
        return Ok(None);
    };
    Ok(Some((parent.to_path_buf(), identity)))
}

fn parent_identity_matches(
    parent: &Path,
    expected: &NoFollowIdentity,
) -> Result<bool, LegacyAliasCleanupError> {
    let Some(metadata) = nofollow_metadata(parent)? else {
        return Ok(false);
    };
    if journal_entry_kind(&metadata) != JournalEntryKind::Directory {
        return Ok(false);
    }
    Ok(nofollow_identity(parent, &metadata)?.as_ref() == Some(expected))
}

fn is_valid_day(name: &OsStr) -> bool {
    let Some(day) = name.to_str() else {
        return false;
    };
    is_day_key(day) && NaiveDate::parse_from_str(day, "%Y%m%d").is_ok()
}

fn talent_directory_from_alias(name: &OsStr) -> Option<OsString> {
    let path = Path::new(name);
    (path.extension() == Some(OsStr::new("log")))
        .then(|| path.file_stem())
        .flatten()
        .filter(|stem| !stem.is_empty())
        .map(OsStr::to_os_string)
}

fn leaf_matches(path: &Path, expected: &LegacyAliasTarget) -> bool {
    match expected {
        LegacyAliasTarget::ManagedDayLog { alias_name }
        | LegacyAliasTarget::ManagedRootLog { alias_name } => {
            path.file_name() == Some(alias_name.as_os_str())
        }
        LegacyAliasTarget::TalentRun { talent_directory } => {
            let mut alias = talent_directory.clone();
            alias.push(".log");
            path.file_name() == Some(alias.as_os_str())
        }
    }
}

fn target_matches(target: &Path, expected: &LegacyAliasTarget) -> bool {
    match expected {
        LegacyAliasTarget::ManagedDayLog { alias_name } => {
            let mut components = target.components();
            let (Some(Component::Normal(payload)), None) = (components.next(), components.next())
            else {
                return false;
            };
            managed_payload_matches(alias_name, payload)
        }
        LegacyAliasTarget::ManagedRootLog { alias_name } => {
            let mut components = target.components();
            let (
                Some(Component::ParentDir),
                Some(Component::Normal(chronicle)),
                Some(Component::Normal(day)),
                Some(Component::Normal(health)),
                Some(Component::Normal(payload)),
                None,
            ) = (
                components.next(),
                components.next(),
                components.next(),
                components.next(),
                components.next(),
                components.next(),
            )
            else {
                return false;
            };
            chronicle == OsStr::new("chronicle")
                && is_valid_day(day)
                && health == OsStr::new("health")
                && managed_payload_matches(alias_name, payload)
        }
        LegacyAliasTarget::TalentRun { talent_directory } => {
            let mut components = target.components();
            let (Some(Component::Normal(directory)), Some(Component::Normal(run)), None) =
                (components.next(), components.next(), components.next())
            else {
                return false;
            };
            directory == talent_directory.as_os_str() && numeric_jsonl_name(run)
        }
    }
}

fn managed_payload_matches(alias: &OsStr, payload: &OsStr) -> bool {
    let (Some(alias), Some(payload)) = (alias.to_str(), payload.to_str()) else {
        return false;
    };
    payload
        .strip_suffix(alias)
        .is_some_and(|reference_prefix| reference_prefix.ends_with('_'))
}

fn numeric_jsonl_name(name: &OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    let Some(id) = name.strip_suffix(".jsonl") else {
        return false;
    };
    !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit())
}

fn is_symbolic_link(metadata: &Metadata) -> bool {
    #[cfg(unix)]
    {
        metadata.file_type().is_symlink()
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileTypeExt;

        let file_type = metadata.file_type();
        file_type.is_symlink_file() || file_type.is_symlink_dir()
    }
}

fn is_file_symbolic_link(metadata: &Metadata) -> bool {
    #[cfg(unix)]
    {
        metadata.file_type().is_symlink()
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileTypeExt;

        metadata.file_type().is_symlink_file()
    }
}

fn journal_entry_kind(metadata: &Metadata) -> JournalEntryKind {
    let file_type = metadata.file_type();
    if is_symbolic_link(metadata) {
        JournalEntryKind::Symlink
    } else if file_type.is_file() {
        JournalEntryKind::RegularFile
    } else if file_type.is_dir() {
        JournalEntryKind::Directory
    } else {
        JournalEntryKind::Other
    }
}

fn nofollow_identity(
    path: &Path,
    metadata: &Metadata,
) -> Result<Option<NoFollowIdentity>, LegacyAliasCleanupError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        let _ = path;
        Ok(Some(NoFollowIdentity::Unix {
            dev: metadata.dev(),
            ino: metadata.ino(),
            kind: journal_entry_kind(metadata),
        }))
    }
    #[cfg(windows)]
    {
        use std::fs::OpenOptions;
        use std::os::windows::fs::OpenOptionsExt;
        use std::os::windows::io::AsRawHandle;

        let _ = metadata;
        let file = match OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(LegacyAliasCleanupError::Io {
                    operation: "open retired alias without following",
                    path: path.to_path_buf(),
                    source,
                });
            }
        };
        let identity =
            file_identity(file.as_raw_handle()).map_err(|source| LegacyAliasCleanupError::Io {
                operation: "identify retired alias without following",
                path: path.to_path_buf(),
                source,
            })?;
        Ok(Some(NoFollowIdentity::Windows { identity }))
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    use tempfile::tempdir;

    #[test]
    fn managed_day_alias_requires_exact_leaf_and_target_shape() {
        let directory = tempdir().unwrap();
        let alias = directory.path().join("convey.log");
        let expected = LegacyAliasTarget::ManagedDayLog {
            alias_name: OsString::from("convey.log"),
        };
        symlink("launch-1_convey.log", &alias).unwrap();
        let observed = observe_legacy_alias_symlink(&alias, &expected).unwrap();
        let LegacyAliasObservationResult::Observed(observed) = observed else {
            panic!("dangling managed alias must be observed");
        };
        assert_eq!(
            remove_observed_legacy_alias_symlink(&observed).unwrap(),
            LegacyAliasDisposition::Removed
        );
        assert!(fs::symlink_metadata(&alias).is_err());

        fs::write(&alias, "regular log").unwrap();
        assert_eq!(
            observe_legacy_alias_symlink(&alias, &expected).unwrap(),
            LegacyAliasObservationResult::Refused(LegacyAliasRefusal::NotSymlink(
                JournalEntryKind::RegularFile
            ))
        );

        fs::remove_file(&alias).unwrap();
        symlink("other.log", &alias).unwrap();
        assert_eq!(
            observe_legacy_alias_symlink(&alias, &expected).unwrap(),
            LegacyAliasObservationResult::Refused(LegacyAliasRefusal::UnexpectedTarget)
        );
    }

    #[test]
    fn talent_alias_requires_the_retired_relative_target_shape() {
        let directory = tempdir().unwrap();
        let alias = directory.path().join("chat.log");
        symlink("other/1700000000000.jsonl", &alias).unwrap();
        let expected = LegacyAliasTarget::TalentRun {
            talent_directory: OsString::from("chat"),
        };
        assert_eq!(
            observe_legacy_alias_symlink(&alias, &expected).unwrap(),
            LegacyAliasObservationResult::Refused(LegacyAliasRefusal::UnexpectedTarget)
        );
        assert!(fs::symlink_metadata(alias).is_ok());
    }

    #[test]
    fn replacement_after_observation_is_preserved() {
        let directory = tempdir().unwrap();
        let alias = directory.path().join("chat.log");
        symlink("chat/1700000000000.jsonl", &alias).unwrap();
        let expected = LegacyAliasTarget::TalentRun {
            talent_directory: OsString::from("chat"),
        };
        let observed = observe_legacy_alias_symlink(&alias, &expected).unwrap();
        let LegacyAliasObservationResult::Observed(observed) = observed else {
            panic!("valid alias must be observed");
        };
        fs::remove_file(&alias).unwrap();
        symlink("chat/1700000000001.jsonl", &alias).unwrap();
        assert_eq!(
            remove_observed_legacy_alias_symlink(&observed).unwrap(),
            LegacyAliasDisposition::Refused(LegacyAliasRefusal::ChangedBeforeRemoval)
        );
        assert_eq!(
            fs::read_link(alias).unwrap(),
            PathBuf::from("chat/1700000000001.jsonl")
        );
    }

    #[test]
    fn parent_replacement_after_observation_preserves_both_namespaces() {
        let directory = tempdir().unwrap();
        let health = directory.path().join("health");
        let retired = directory.path().join("health-retired");
        fs::create_dir(&health).unwrap();
        let alias = health.join("convey.log");
        symlink("launch-1_convey.log", &alias).unwrap();
        let expected = LegacyAliasTarget::ManagedDayLog {
            alias_name: OsString::from("convey.log"),
        };
        let observed = observe_legacy_alias_symlink(&alias, &expected).unwrap();
        let LegacyAliasObservationResult::Observed(observed) = observed else {
            panic!("valid alias must be observed");
        };

        fs::rename(&health, &retired).unwrap();
        fs::create_dir(&health).unwrap();
        symlink("launch-1_convey.log", &alias).unwrap();

        assert_eq!(
            remove_observed_legacy_alias_symlink(&observed).unwrap(),
            LegacyAliasDisposition::Refused(LegacyAliasRefusal::ChangedBeforeRemoval)
        );
        assert!(fs::symlink_metadata(&alias).is_ok());
        assert!(fs::symlink_metadata(retired.join("convey.log")).is_ok());
    }

    #[test]
    fn cleanup_reports_regular_files_and_directories_without_removing_them() {
        let directory = tempdir().unwrap();
        let health = directory.path().join("health");
        fs::create_dir_all(&health).unwrap();
        fs::create_dir(health.join("folder.log")).unwrap();
        fs::write(health.join("service.log"), "ordinary log").unwrap();

        let report = cleanup_legacy_log_aliases(directory.path()).unwrap();
        assert_eq!(report.removed, 0);
        assert_eq!(
            report.refused,
            vec![
                LegacyAliasRefused {
                    path: health.join("folder.log"),
                    reason: LegacyAliasRefusal::NotSymlink(JournalEntryKind::Directory),
                },
                LegacyAliasRefused {
                    path: health.join("service.log"),
                    reason: LegacyAliasRefusal::NotSymlink(JournalEntryKind::RegularFile),
                },
            ]
        );
        assert!(health.join("folder.log").is_dir());
        assert_eq!(
            fs::read_to_string(health.join("service.log")).unwrap(),
            "ordinary log"
        );
    }

    #[test]
    fn cleanup_visits_only_declared_roots_and_valid_day_health_directories() {
        let directory = tempdir().unwrap();
        let journal = directory.path();
        let root_health = journal.join("health");
        let valid_health = journal.join("chronicle/20240101/health");
        let invalid_health = journal.join("chronicle/not-a-day/health");
        let invalid_calendar_health = journal.join("chronicle/20240230/health");
        let outside = journal.join("outside");
        fs::create_dir_all(&root_health).unwrap();
        fs::create_dir_all(&valid_health).unwrap();
        fs::create_dir_all(&invalid_health).unwrap();
        fs::create_dir_all(&invalid_calendar_health).unwrap();
        fs::create_dir_all(journal.join("talents")).unwrap();
        fs::create_dir_all(journal.join("agents")).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::create_dir(root_health.join("pruning-runs")).unwrap();
        let payload = valid_health.join("launch-1_convey.log");
        fs::write(&payload, "managed payload").unwrap();
        symlink(
            "../chronicle/20240101/health/launch-1_convey.log",
            root_health.join("convey.log"),
        )
        .unwrap();
        symlink("launch-1_convey.log", valid_health.join("convey.log")).unwrap();
        symlink("missing", root_health.join("heartbeat.log")).unwrap();
        symlink("launch-1_convey.log", invalid_health.join("convey.log")).unwrap();
        symlink(
            "launch-1_convey.log",
            invalid_calendar_health.join("convey.log"),
        )
        .unwrap();
        symlink("launch-1_convey.log", outside.join("convey.log")).unwrap();
        symlink("chat/1700000000000.jsonl", journal.join("talents/chat.log")).unwrap();
        symlink(
            "agent/1700000000001.jsonl",
            journal.join("agents/agent.log"),
        )
        .unwrap();

        let report = cleanup_legacy_log_aliases(journal).unwrap();
        assert_eq!(report.removed, 4);
        assert_eq!(
            report.refused,
            vec![
                LegacyAliasRefused {
                    path: root_health.join("heartbeat.log"),
                    reason: LegacyAliasRefusal::UnexpectedTarget,
                },
                LegacyAliasRefused {
                    path: payload.clone(),
                    reason: LegacyAliasRefusal::NotSymlink(JournalEntryKind::RegularFile),
                },
            ]
        );
        for removed in [
            root_health.join("convey.log"),
            valid_health.join("convey.log"),
            journal.join("talents/chat.log"),
            journal.join("agents/agent.log"),
        ] {
            assert!(fs::symlink_metadata(removed).is_err());
        }
        assert_eq!(fs::read_to_string(payload).unwrap(), "managed payload");
        assert!(fs::symlink_metadata(root_health.join("heartbeat.log")).is_ok());
        assert!(fs::symlink_metadata(invalid_health.join("convey.log")).is_ok());
        assert!(fs::symlink_metadata(invalid_calendar_health.join("convey.log")).is_ok());
        assert!(fs::symlink_metadata(outside.join("convey.log")).is_ok());

        let second = cleanup_legacy_log_aliases(journal).unwrap();
        assert_eq!(second.removed, 0);
        assert_eq!(second.refused, report.refused);
    }
}
