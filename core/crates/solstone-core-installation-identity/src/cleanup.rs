// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only journal protection used by owner-scoped cleanup.

use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::{
    IdentityError, InstallationId, JournalToken, LifecycleState, NamespaceName, OwnerBase,
    PlatformTag, RootToken, enumerate_registry, namespace_name, open_provider,
};

#[cfg(any(windows, all(unix, not(target_os = "linux"))))]
use crate::lock_existing_owner;
#[cfg(target_os = "linux")]
use crate::lock_existing_owner_shared;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CleanupTargetKind {
    Shared,
    PerInstall,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CleanupSkip {
    ProtectedJournal,
    AnotherInstallation,
    RegistryUnreadable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CleanupTargetDecision {
    Remove,
    Skip(CleanupSkip),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProtectedJournals {
    paths: Vec<PathBuf>,
    platform: PlatformTag,
}

impl ProtectedJournals {
    #[must_use]
    pub fn new(platform: PlatformTag) -> Self {
        Self {
            paths: Vec::new(),
            platform,
        }
    }

    pub fn insert(&mut self, path: PathBuf) {
        if !self.paths.iter().any(|existing| {
            same_protected_place(existing, &path, self.platform == PlatformTag::Windows)
        }) {
            self.paths.push(path);
        }
    }

    #[must_use]
    pub fn paths(&self) -> &[PathBuf] {
        &self.paths
    }

    #[must_use]
    pub const fn platform(&self) -> PlatformTag {
        self.platform
    }

    #[must_use]
    pub fn overlaps(&self, target: &Path) -> bool {
        self.paths
            .iter()
            .any(|journal| cleanup_target_overlaps_journal(target, journal, self.platform))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstallationJournalRecord {
    pub lifecycle: LifecycleState,
    pub journal_token: JournalToken,
    pub namespace: NamespaceName,
    pub generation: crate::Generation,
    pub id: InstallationId,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstallationJournalCensus {
    pub registry_known: bool,
    pub records: Vec<InstallationJournalRecord>,
    pub root_record: Option<InstallationJournalRecord>,
}

/// Whether two paths overlap by component containment, including canonical aliases.
#[must_use]
pub fn cleanup_target_overlaps_journal(
    target: &Path,
    journal: &Path,
    platform: PlatformTag,
) -> bool {
    if !target.is_absolute() || !journal.is_absolute() {
        return false;
    }
    let case_insensitive = platform == PlatformTag::Windows;
    if component_overlap(target, journal, case_insensitive) {
        return true;
    }
    match (fs::canonicalize(target), fs::canonicalize(journal)) {
        (Ok(target), Ok(journal)) => component_overlap(&target, &journal, case_insensitive),
        _ => false,
    }
}

/// Whether two paths identify exactly the same place, not merely related paths.
#[must_use]
pub fn same_protected_place(a: &Path, b: &Path, case_insensitive: bool) -> bool {
    if !a.is_absolute() || !b.is_absolute() {
        return false;
    }
    if normalized_equal(a, b, case_insensitive) {
        return true;
    }
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => normalized_equal(&a, &b, case_insensitive),
        _ => false,
    }
}

/// Whether creating or rewriting identity storage could touch a protected journal.
#[must_use]
pub fn identity_writes_overlap(
    owner_base_path: &Path,
    journals: &ProtectedJournals,
    platform: PlatformTag,
) -> bool {
    journals
        .paths()
        .iter()
        .any(|journal| cleanup_target_overlaps_journal(owner_base_path, journal, platform))
}

/// Decide whether a cleanup target may be removed under the current census.
#[must_use]
pub fn may_remove_cleanup_target(
    target: &Path,
    kind: CleanupTargetKind,
    last_installation: bool,
    registry_known: bool,
    journals: &ProtectedJournals,
    platform: PlatformTag,
) -> CleanupTargetDecision {
    if journals
        .paths()
        .iter()
        .any(|journal| cleanup_target_overlaps_journal(target, journal, platform))
    {
        return CleanupTargetDecision::Skip(CleanupSkip::ProtectedJournal);
    }
    match kind {
        CleanupTargetKind::PerInstall => CleanupTargetDecision::Remove,
        CleanupTargetKind::Shared if !last_installation => {
            CleanupTargetDecision::Skip(CleanupSkip::AnotherInstallation)
        }
        CleanupTargetKind::Shared if !registry_known => {
            CleanupTargetDecision::Skip(CleanupSkip::RegistryUnreadable)
        }
        CleanupTargetKind::Shared => CleanupTargetDecision::Remove,
    }
}

/// Read the journal tokens for all identity records without creating storage.
///
/// With `lock` enabled, the existing owner lock is acquired without creating it.
/// The Unix service-uninstall child uses the unlocked form because its setup
/// parent retains the owner lock while the child runs.
pub fn read_installation_journal_census(
    owner: &OwnerBase,
    root: &RootToken,
    lock: bool,
) -> Result<InstallationJournalCensus, IdentityError> {
    let provider = match open_provider(owner, false) {
        Ok(provider) => provider,
        Err(IdentityError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::NotFound
                && matches!(
                    fs::symlink_metadata(owner.path()),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound
                ) =>
        {
            return Ok(InstallationJournalCensus {
                registry_known: true,
                records: Vec::new(),
                root_record: None,
            });
        }
        Err(error) => return Err(error),
    };

    #[cfg(target_os = "linux")]
    let _lock = if lock {
        Some(lock_existing_owner_shared(&provider)?)
    } else {
        None
    };
    #[cfg(all(unix, not(target_os = "linux")))]
    let _lock = if lock {
        Some(lock_existing_owner(&provider)?)
    } else {
        None
    };
    #[cfg(windows)]
    let _lock = if lock {
        Some(lock_existing_owner(&provider)?)
    } else {
        None
    };

    let root_namespace = namespace_name(owner.platform(), root);
    let registry = enumerate_registry(&provider)?;
    let records: Vec<_> = registry
        .values()
        .filter_map(|snapshot| {
            snapshot
                .record
                .as_ref()
                .map(|record| InstallationJournalRecord {
                    lifecycle: record.state,
                    journal_token: record.journal_token.clone(),
                    namespace: snapshot.namespace.clone(),
                    generation: record.generation,
                    id: record.id.clone(),
                })
        })
        .collect();
    let root_record = registry
        .get(&root_namespace)
        .and_then(|snapshot| snapshot.record.as_ref())
        .map(|record| InstallationJournalRecord {
            lifecycle: record.state,
            journal_token: record.journal_token.clone(),
            namespace: root_namespace,
            generation: record.generation,
            id: record.id.clone(),
        });

    Ok(InstallationJournalCensus {
        registry_known: true,
        records,
        root_record,
    })
}

fn component_overlap(a: &Path, b: &Path, case_insensitive: bool) -> bool {
    let a = normalized_components(a);
    let b = normalized_components(b);
    components_equal(&a, &b, case_insensitive)
        || is_component_prefix(&a, &b, case_insensitive)
        || is_component_prefix(&b, &a, case_insensitive)
}

fn normalized_equal(a: &Path, b: &Path, case_insensitive: bool) -> bool {
    let a = normalized_components(a);
    let b = normalized_components(b);
    components_equal(&a, &b, case_insensitive)
}

fn normalized_components(path: &Path) -> Vec<std::ffi::OsString> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if normalized
                    .components()
                    .next_back()
                    .is_some_and(|last| matches!(last, Component::Normal(_)))
                {
                    normalized.pop();
                }
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
        .components()
        .map(|component| component.as_os_str().to_os_string())
        .collect()
}

fn components_equal(a: &[std::ffi::OsString], b: &[std::ffi::OsString], insensitive: bool) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(a, b)| component_equal(a, b, insensitive))
}

fn is_component_prefix(
    prefix: &[std::ffi::OsString],
    path: &[std::ffi::OsString],
    insensitive: bool,
) -> bool {
    prefix.len() < path.len()
        && prefix
            .iter()
            .zip(path)
            .all(|(a, b)| component_equal(a, b, insensitive))
}

fn component_equal(a: &std::ffi::OsStr, b: &std::ffi::OsStr, insensitive: bool) -> bool {
    if insensitive {
        a.to_string_lossy()
            .eq_ignore_ascii_case(&b.to_string_lossy())
    } else {
        a == b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleanup_target_overlap_uses_components_in_both_directions() {
        let platform = PlatformTag::Linux;
        assert!(cleanup_target_overlaps_journal(
            Path::new("/journal"),
            Path::new("/journal"),
            platform
        ));
        assert!(cleanup_target_overlaps_journal(
            Path::new("/journal/child"),
            Path::new("/journal"),
            platform
        ));
        assert!(cleanup_target_overlaps_journal(
            Path::new("/journal"),
            Path::new("/journal/child"),
            platform
        ));
        assert!(!cleanup_target_overlaps_journal(
            Path::new("/journal-backup"),
            Path::new("/journal"),
            platform
        ));
        assert!(!cleanup_target_overlaps_journal(
            Path::new("/other"),
            Path::new("/journal"),
            platform
        ));
        assert!(cleanup_target_overlaps_journal(
            Path::new("/a/../journal/./child"),
            Path::new("/journal"),
            platform
        ));
    }

    #[test]
    fn cleanup_target_overlap_honors_windows_case_aliases() {
        assert!(cleanup_target_overlaps_journal(
            Path::new("/Users/Owner/Journal/child"),
            Path::new("/users/owner/journal"),
            PlatformTag::Windows
        ));
        assert!(!cleanup_target_overlaps_journal(
            Path::new("/Users/Owner/Journal-backup"),
            Path::new("/users/owner/journal"),
            PlatformTag::Windows
        ));
    }

    #[test]
    fn same_protected_place_requires_path_equality() {
        assert!(same_protected_place(
            Path::new("/journal/./data"),
            Path::new("/journal/data"),
            false
        ));
        assert!(same_protected_place(
            Path::new("/Owner/Journal"),
            Path::new("/owner/journal"),
            true
        ));
        assert!(!same_protected_place(
            Path::new("/journal"),
            Path::new("/journal/child"),
            false
        ));
    }

    #[test]
    fn identity_writes_overlap_covers_provider_ancestors_and_descendants() {
        let mut journals = ProtectedJournals::new(PlatformTag::Linux);
        journals.insert(PathBuf::from("/home/.local/share/solstone"));
        assert!(identity_writes_overlap(
            Path::new("/home/.local/share/solstone/installation-identity/v1"),
            &journals,
            PlatformTag::Linux
        ));

        let mut nested = ProtectedJournals::new(PlatformTag::Linux);
        nested.insert(PathBuf::from(
            "/home/.local/share/solstone/installation-identity/v1/namespaces/id/journal",
        ));
        assert!(identity_writes_overlap(
            Path::new("/home/.local/share/solstone/installation-identity/v1"),
            &nested,
            PlatformTag::Linux
        ));
    }

    #[test]
    fn cleanup_target_decision_applies_shared_and_per_install_rules() {
        let target = Path::new("/home/.cache/solstone/rclone");
        let empty = ProtectedJournals::new(PlatformTag::Linux);
        assert_eq!(
            may_remove_cleanup_target(
                target,
                CleanupTargetKind::Shared,
                false,
                true,
                &empty,
                PlatformTag::Linux,
            ),
            CleanupTargetDecision::Skip(CleanupSkip::AnotherInstallation)
        );
        assert_eq!(
            may_remove_cleanup_target(
                target,
                CleanupTargetKind::Shared,
                true,
                false,
                &empty,
                PlatformTag::Linux,
            ),
            CleanupTargetDecision::Skip(CleanupSkip::RegistryUnreadable)
        );
        let mut tombstoned = ProtectedJournals::new(PlatformTag::Linux);
        tombstoned.insert(target.join("journal-a"));
        assert_eq!(
            may_remove_cleanup_target(
                target,
                CleanupTargetKind::Shared,
                true,
                true,
                &tombstoned,
                PlatformTag::Linux,
            ),
            CleanupTargetDecision::Skip(CleanupSkip::ProtectedJournal)
        );
        assert_eq!(
            may_remove_cleanup_target(
                Path::new("/home/.config/solstone/config.toml"),
                CleanupTargetKind::Shared,
                true,
                true,
                &empty,
                PlatformTag::Linux,
            ),
            CleanupTargetDecision::Remove
        );
        assert_eq!(
            may_remove_cleanup_target(
                target,
                CleanupTargetKind::PerInstall,
                false,
                false,
                &empty,
                PlatformTag::Linux,
            ),
            CleanupTargetDecision::Remove
        );
        assert_eq!(
            may_remove_cleanup_target(
                target,
                CleanupTargetKind::PerInstall,
                true,
                true,
                &tombstoned,
                PlatformTag::Linux,
            ),
            CleanupTargetDecision::Skip(CleanupSkip::ProtectedJournal)
        );
    }
}
