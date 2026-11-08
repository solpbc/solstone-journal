// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Fresh coverage evidence for the retained teardown admission.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{Map, Value};

use solstone_core_backup::teardown_configuration::TeardownConfiguration;
use solstone_core_backup_runtime::TeardownReaderRefusal;
use solstone_core_journal_io::JournalRoot;
use solstone_core_journal_io::activity_obligations::{observe_obligations, observe_with_carrier};
use solstone_core_journal_io::media::RootCarrierObservation;
use solstone_core_journal_io::paths::{Segment, SegmentLayout};
use solstone_core_retention::archive_catalog::read_complete_catalog;
use solstone_core_segment::{OwnerDeletionState, owner_deletion_state};

use crate::measurement::measure_local_media;
use crate::restore::owned::discover;
use crate::status::{CatalogLocalPresence, CatalogMediaObservation, observe_catalog_media};

mod owned;
#[cfg(target_os = "ios")]
pub use owned::run_owned_teardown_with_root;
#[cfg(feature = "test-hooks")]
pub use owned::{
    OwnedTeardownInterrupt, OwnedTeardownInterruptArm, OwnedTeardownInterruptObservation,
    arm_owned_teardown_interrupt, owned_teardown_interrupt_observation,
};
pub use owned::{
    OwnedTeardownOutcome, SettledTeardownReport, TeardownAttemptJournal, TeardownRunPhase,
    TeardownRunRefusal, run_owned_teardown,
};

/// Add a finished "turn off & delete backup" to the journal's deletion record:
/// the copies its destination held are gone. A teardown that was held, refused
/// or ended unknown records nothing.
pub fn record_backup_off(
    journal: &std::path::Path,
    outcome: &OwnedTeardownOutcome,
    surface: solstone_core_removals::Surface,
) {
    let OwnedTeardownOutcome::Finished(settled) = outcome else {
        return;
    };
    // Only own storage reports whether its bucket keeps what was deleted.
    let storage = if settled.remote().storage_retention().is_some() {
        "own"
    } else {
        "operated"
    };
    solstone_core_removals::Removal::begin(
        journal,
        solstone_core_removals::Action::BackupOff,
        surface,
    )
    .record(solstone_core_removals::Unit::Backup {
        storage: storage.to_owned(),
    });
}

/// The plain sentence an owner gets after their own storage was emptied, when
/// that storage may still keep what was deleted.
pub fn storage_retention_sentence(
    retention: solstone_core_backup_runtime::teardown::owned::StorageRetention,
) -> Option<&'static str> {
    use solstone_core_backup_runtime::teardown::owned::{StorageKeeps, StorageRetention};
    match retention {
        StorageRetention::Removes => None,
        StorageRetention::Keeps(StorageKeeps::Versions) => Some(
            "your storage keeps earlier versions of deleted files, so copies of this backup stay in your bucket until you delete those versions or your provider's own rules remove them.",
        ),
        StorageRetention::Keeps(StorageKeeps::ObjectLock | StorageKeeps::FileLock) => Some(
            "your bucket can lock files against deletion, so copies of this backup stay there until any lock ends and you delete those versions or your provider's own rules remove them.",
        ),
        StorageRetention::Keeps(StorageKeeps::HiddenFiles) => Some(
            "your B2 bucket only hides deleted files unless a lifecycle rule for the whole bucket removes them, so copies of this backup stay there until you add one or delete them yourself.",
        ),
        StorageRetention::Unknown => Some(
            "your journal couldn't check whether your storage keeps deleted files, so check your bucket's versioning, lifecycle and lock settings to make sure this backup is gone.",
        ),
    }
}

/// Read both owning sources afresh inside `TeardownAdmission::check`.
/// A successful observation grants no deletion, cleanup or crash-safety authority.
/// Call again at each destructive boundary; config and lease checks stay in admission.
///
/// Refuses while any active archive witness lacks a same-size local file, so the
/// backup is never deleted while it may hold the only copy of owner media. The one
/// exception is media in a segment the owner deleted: restore never brings it back.
pub fn read_teardown_coverage(
    root: &Arc<JournalRoot>,
    configuration: &TeardownConfiguration,
    carrier: Option<RootCarrierObservation<'_>>,
) -> Result<(), TeardownReaderRefusal> {
    let catalog =
        read_complete_catalog(root).map_err(|_| TeardownReaderRefusal::CatalogUnavailable)?;
    let held = match carrier {
        Some(carrier) => observe_with_carrier(root, carrier)
            .map_err(|_| TeardownReaderRefusal::RecoveryUnavailable)?
            .unresolved_entries()
            .next()
            .is_some(),
        None => !observe_obligations(root)
            .map_err(|_| TeardownReaderRefusal::RecoveryUnavailable)?
            .entries()
            .is_empty(),
    };
    if held {
        return Err(TeardownReaderRefusal::RecoveryHeld);
    }
    // A missing ledger reads as an empty catalog. Once offload has been on, an
    // empty catalog may mean the record was lost rather than that nothing moved.
    if catalog.days.is_empty() && offload_recorded(configuration.backup()) {
        return Err(TeardownReaderRefusal::CatalogUnavailable);
    }
    let observed = observe_catalog_media(&catalog, &measure_local_media(root.canonical_path()));
    // A degraded catalog may omit witnesses, so its absence of backup-only media proves nothing.
    if observed.catalog_degraded {
        return Err(TeardownReaderRefusal::CatalogUnavailable);
    }
    let mut days = BTreeMap::new();
    let mut restorable = false;
    let mut unplaced = false;
    for file in &observed.files {
        match coverage(root, &mut days, file) {
            Coverage::Covered => {}
            Coverage::Restorable => restorable = true,
            Coverage::Unplaced => unplaced = true,
        }
    }
    // The scan above is by path; it only describes this root if the path still names it.
    root.revalidate_canonical_binding()
        .map_err(|_| TeardownReaderRefusal::CatalogUnavailable)?;
    // Restoring what can be restored comes first; what is left then names itself.
    if restorable {
        Err(TeardownReaderRefusal::BackupOnlyHeld)
    } else if unplaced {
        Err(TeardownReaderRefusal::BackupOnlyUnplaced)
    } else {
        Ok(())
    }
}

/// Local evidence that offload was ever on or ever ran on this journal.
fn offload_recorded(backup: &Map<String, Value>) -> bool {
    let enabled = backup
        .get("offload")
        .and_then(|offload| offload.get("enabled"))
        .and_then(Value::as_bool)
        == Some(true);
    let ran = backup.get("last_offload").is_some_and(|last| {
        !last.get("time").unwrap_or(&Value::Null).is_null()
            || !last.get("last_ok_time").unwrap_or(&Value::Null).is_null()
            || last
                .get("files_marked")
                .and_then(Value::as_u64)
                .is_some_and(|files| files > 0)
    });
    enabled || ran
}

enum Coverage {
    Covered,
    /// Only in the backup, with a local recording a restore can return it to.
    Restorable,
    /// Only in the backup, with no single local recording to return it to.
    Unplaced,
}

/// Unknown presence and every size mismatch stay uncovered.
fn coverage(
    root: &Arc<JournalRoot>,
    days: &mut BTreeMap<String, Result<Vec<Segment>, String>>,
    file: &CatalogMediaObservation,
) -> Coverage {
    match file.presence {
        CatalogLocalPresence::Present { local_bytes } if local_bytes == file.witness.file.bytes => {
            Coverage::Covered
        }
        CatalogLocalPresence::Present { .. } | CatalogLocalPresence::Unknown { .. } => {
            Coverage::Restorable
        }
        CatalogLocalPresence::Absent => {
            let (Some(layout), Some(segment_dir)) =
                (file.witness.layout, &file.witness.segment_dir)
            else {
                return Coverage::Restorable;
            };
            let day = root.canonical_path().join("chronicle").join(&file.day);
            let path = match layout {
                SegmentLayout::Direct => day.join(segment_dir),
                SegmentLayout::Named => day.join(&file.stream).join(segment_dir),
            };
            match owner_deletion_state(&path) {
                Ok(OwnerDeletionState::Deleted) => Coverage::Covered,
                Ok(OwnerDeletionState::Live)
                    if std::fs::symlink_metadata(&path)
                        .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
                {
                    Coverage::Unplaced
                }
                _ => Coverage::Restorable,
            }
        }
        // A historical record names only stream and segment key. Resolve it the way
        // legacy restore does: the one directory with that identity on that day.
        CatalogLocalPresence::Unbound => {
            let Ok(segments) = days
                .entry(file.day.clone())
                .or_insert_with(|| discover(root, &file.day))
            else {
                return Coverage::Unplaced;
            };
            let mut matching = segments.iter().filter(|segment| {
                segment.locator_identity().is_ok_and(|identity| {
                    identity.stream == file.stream && identity.key == file.segment
                })
            });
            let (Some(segment), None) = (matching.next(), matching.next()) else {
                return Coverage::Unplaced;
            };
            match owner_deletion_state(segment.path()) {
                Ok(OwnerDeletionState::Deleted) => Coverage::Covered,
                _ => Coverage::Restorable,
            }
        }
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod tests;
