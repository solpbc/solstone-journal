// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Restores interrupted by a crash or restart. One that provably never
//! published is undone: its own unpublished stage, then its intent. One whose
//! published file matches the archive exactly is released the same way: the
//! file is local again, and a later restore of it records the event and retires
//! its mark as already present. Marks and the ledger are untouched either way,
//! so nothing is counted twice. Any other published state is held. A restore
//! that ended mid-download, before its intent, is recognised only by its
//! dedicated download leaf and loses that stage the same way, as does an
//! approval or offer readback at the journal root by its own leaf, and an
//! offload producer's copy at the journal root, whose original never moved.

use std::sync::Arc;

use solstone_core_journal_io::activity::ActivityLease;

#[cfg(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "windows",
    target_os = "ios",
    target_os = "android"
))]
use super::RestoreIntent;

/// Why an interrupted restore stays an unresolved obligation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AbandonedRestoreHold {
    /// The intent could not be read, decoded or matched to its name.
    Unreadable,
    /// The journal or destination no longer matches what the intent recorded.
    DestinationChanged,
    /// The recorded stage holds something other than its own unpublished copy.
    StageChanged,
    /// The recorded outcome needs its own restore: a published file is missing,
    /// or a recorded restore still owes its mark retirement.
    Recorded,
    /// A file with other content is in place. It is never touched.
    PublishedContentMismatch,
    /// Observation or removal failed.
    Native,
}

/// What ended an interrupted restore. Neither is a recorded restore.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AbandonedRestoreRelease {
    /// Nothing was published; the media stays offloaded and restorable.
    Undone,
    /// The exact file is local again; its restore is recorded when it is next
    /// restored, as already present.
    PublishedExact,
}

#[derive(Debug)]
pub struct AbandonedRestore {
    pub operation_id: Option<String>,
    pub outcome: Result<AbandonedRestoreRelease, AbandonedRestoreHold>,
}

/// End every interrupted restore that left nothing uncertain. Call only with a
/// freshly acquired lease, before creating anything. Desktop storage that is
/// not exclusive-local still returns an empty list. Android and iOS refuse
/// when the private container is not qualified, and never report that refusal
/// as an empty success.
pub fn recover_abandoned_restores(
    activity: &Arc<ActivityLease>,
) -> Result<Vec<AbandonedRestore>, std::io::Error> {
    #[cfg(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "windows",
        target_os = "ios",
        target_os = "android"
    ))]
    {
        local::recover(activity)
    }
    #[cfg(not(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "windows",
        target_os = "ios",
        target_os = "android"
    )))]
    {
        let _ = activity;
        Err(unsupported_storage())
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn unsupported_storage() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "the activity lease does not exclude every writer here",
    )
}

fn platform_miss<T>(error: Option<std::io::Error>) -> Result<Vec<T>, std::io::Error> {
    #[cfg(any(target_os = "ios", target_os = "android"))]
    {
        Err(error.unwrap_or_else(unsupported_storage))
    }
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        let _ = error;
        Ok(Vec::new())
    }
}

#[cfg(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "windows",
    target_os = "ios",
    target_os = "android"
))]
mod local {
    use std::collections::BTreeSet;
    use std::ffi::OsStr;
    use std::path::{Component, Path, PathBuf};

    use solstone_core_format::segment::segment_key;
    use solstone_core_journal_io::journal_root::JournalEntryKind;

    use solstone_core_journal_io::activity_obligations::{
        IntentRead, NativeObligation, intent_filename, is_stage_namespace_name, observe_obligations,
    };
    use solstone_core_journal_io::media::{
        Cleanup, Confirmation, Durability, MediaDirectory, MediaEntry, MediaStage, OriginalName,
        REMOVAL_ORIGINAL_LEAF, REMOVAL_RECORD_LEAF, RemovalEffect, RemovalPhase, digest_reader,
        exclusive_local_activity, retire_recorded_stage, retire_unrecorded_stage,
        retire_unrecorded_stage_where,
    };

    use super::super::{MarkRetirement, Obligation, RESTORE_DOWNLOAD_LEAF, selected_name};
    use super::*;
    use crate::archive_approval::READBACK_DOWNLOAD_LEAF;

    type Hold = AbandonedRestoreHold;

    pub(super) fn recover(
        activity: &Arc<ActivityLease>,
    ) -> Result<Vec<AbandonedRestore>, std::io::Error> {
        if let Some(miss) = storage_miss(exclusive_local_activity(activity)) {
            return miss;
        }
        let census = match observe_obligations(activity.journal_root()) {
            Ok(census) => census,
            Err(error) => return census_miss(error),
        };
        let mut recovered: Vec<_> = census
            .entries()
            .iter()
            .filter_map(|obligation| match obligation {
                NativeObligation::Intent {
                    entry, contents, ..
                } => Some((entry, contents)),
                NativeObligation::Stage { .. } => None,
            })
            .map(|(entry, contents)| match contents {
                IntentRead::Complete {
                    operation_id,
                    bytes,
                } => AbandonedRestore {
                    operation_id: Some(operation_id.clone()),
                    outcome: recover_one(activity, entry, operation_id, bytes),
                },
                _ => AbandonedRestore {
                    operation_id: None,
                    outcome: Err(Hold::Unreadable),
                },
            })
            .collect();
        recovered.extend(retire_unrecorded_downloads(activity)?);
        recovered.extend(retire_empty_metadata_stages(activity)?);
        Ok(recovered)
    }

    fn storage_miss<T>(qualified: std::io::Result<bool>) -> Option<Result<Vec<T>, std::io::Error>> {
        match qualified {
            Ok(true) => None,
            Ok(false) => Some(super::platform_miss(None)),
            Err(error) => Some(super::platform_miss(Some(error))),
        }
    }

    fn census_miss<T>(error: impl std::fmt::Display) -> Result<Vec<T>, std::io::Error> {
        super::platform_miss(Some(std::io::Error::other(error.to_string())))
    }

    /// A restore, legacy probe or approval readback that ended mid-download
    /// leaves a stage holding only its dedicated download leaf, or nothing if it
    /// ended before creating it. An offload producer that ended mid-run leaves
    /// a root stage holding one copy of an original that never moved. Only such
    /// a stage, where that producer stages and named by no intent, is removed.
    fn retire_unrecorded_downloads(
        activity: &Arc<ActivityLease>,
    ) -> Result<Vec<AbandonedRestore>, std::io::Error> {
        let census = match observe_obligations(activity.journal_root()) {
            Ok(census) => census,
            Err(error) => return census_miss(error),
        };
        let mut referenced = BTreeSet::new();
        for obligation in census.entries() {
            let NativeObligation::Intent { contents, .. } = obligation else {
                continue;
            };
            let IntentRead::Complete { bytes, .. } = contents else {
                return census_miss("unreadable restore intent");
            };
            match RestoreIntent::decode(Ok(bytes)) {
                Ok(intent) => {
                    referenced.insert(intent.stage.stage_name);
                    if let Some(binding) = intent.recovery_binding {
                        referenced.insert(binding.reservation_name().to_owned());
                    }
                }
                Err(_) => return census_miss("unreadable restore intent"),
            };
        }
        Ok(census
            .entries()
            .iter()
            .filter_map(|obligation| match obligation {
                NativeObligation::Stage { relative, entry }
                    if entry.kind == JournalEntryKind::Directory
                        && entry
                            .name
                            .to_str()
                            .is_some_and(|name| !referenced.contains(name)) =>
                {
                    Some((download_stage_location(relative)?, entry))
                }
                _ => None,
            })
            .filter_map(|((parent, leaf), entry)| {
                let parent =
                    MediaDirectory::open(Arc::clone(activity.journal_root()), &parent).ok()?;
                match leaf {
                    Leaf::Download(leaf) => retire_unrecorded_stage(
                        activity,
                        &parent,
                        &entry.name,
                        entry.identity,
                        OsStr::new(leaf),
                    ),
                    Leaf::RootCopy => retire_unrecorded_stage_where(
                        activity,
                        &parent,
                        &entry.name,
                        entry.identity,
                        &root_copy,
                    ),
                }
                .ok()
                .filter(|retired| *retired)
                .map(|_| AbandonedRestore {
                    operation_id: None,
                    outcome: Ok(AbandonedRestoreRelease::Undone),
                })
            })
            .collect())
    }

    enum Leaf {
        Download(&'static str),
        RootCopy,
    }

    /// At the root: a readback download, or an offload producer's copy under
    /// its original's name. A name any other stage producer uses stays held,
    /// including `payload`, which an earlier version's readback used.
    fn root_copy(name: &OsStr) -> bool {
        name == READBACK_DOWNLOAD_LEAF
            || ![
                "payload",
                RESTORE_DOWNLOAD_LEAF,
                REMOVAL_ORIGINAL_LEAF,
                REMOVAL_RECORD_LEAF,
            ]
            .iter()
            .any(|held| name == *held)
    }

    /// Where a download stage may sit, with what it may hold there. Restores
    /// stage in a segment's parent: a day directory, or a stream directory
    /// under it. Approval and offer readback, and the offload producer's
    /// copies, stage at the journal root. Never a segment or anywhere else.
    fn download_stage_location(relative: &Path) -> Option<(PathBuf, Leaf)> {
        let parts = relative
            .components()
            .map(|part| match part {
                Component::Normal(name) => name.to_str(),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()?;
        let day = |name: &str| name.len() == 8 && name.bytes().all(|byte| byte.is_ascii_digit());
        let stream = |name: &str| !name.starts_with('.') && segment_key(name).is_none();
        match parts.as_slice() {
            [_] => Some((PathBuf::new(), Leaf::RootCopy)),
            ["chronicle", day_name, _] if day(day_name) => Some((
                Path::new("chronicle").join(day_name),
                Leaf::Download(RESTORE_DOWNLOAD_LEAF),
            )),
            ["chronicle", day_name, stream_name, _] if day(day_name) && stream(stream_name) => {
                Some((
                    Path::new("chronicle").join(day_name).join(stream_name),
                    Leaf::Download(RESTORE_DOWNLOAD_LEAF),
                ))
            }
            _ => None,
        }
    }

    fn recover_one(
        activity: &Arc<ActivityLease>,
        entry: &MediaEntry,
        operation_id: &str,
        bytes: &[u8],
    ) -> Result<AbandonedRestoreRelease, Hold> {
        let intent = RestoreIntent::decode(Ok(bytes)).map_err(|_| Hold::Unreadable)?;
        if intent.event.operation_id != operation_id {
            return Err(Hold::Unreadable);
        }
        let root = activity.journal_root();
        if intent.destination.root != root.identity().observation_id() {
            return Err(Hold::DestinationChanged);
        }
        let segment = intent.locator.relative_segment();
        let (Some(parent), Some(name)) = (segment.parent(), segment.file_name()) else {
            return Err(Hold::DestinationChanged);
        };
        let parent =
            MediaDirectory::open(Arc::clone(root), parent).map_err(|_| Hold::DestinationChanged)?;
        let live = parent
            .open_child(name)
            .map_err(|_| Hold::DestinationChanged)?
            .ok_or(Hold::DestinationChanged)?;
        if parent.identity() != intent.destination.parent
            || parent.identity() != intent.stage.parent
            || live.identity() != intent.destination.segment
        {
            return Err(Hold::DestinationChanged);
        }
        let release = match live.observe_file(selected_name(&intent)) {
            // Publication records its outcome before anything later; none, or
            // one with no effect, means no file was placed.
            Ok(None) if intent.outcome.is_none() || intent.no_effect_outcome() => {
                AbandonedRestoreRelease::Undone
            }
            Ok(None) => return Err(Hold::Recorded),
            // A recorded restore already retired its witness; only its own read
            // may retire the mark it still owes.
            Ok(Some(present)) if present.digest == intent.expected_digest() => {
                if intent.event_recorded == Obligation::Complete
                    && matches!(intent.retirement, MarkRetirement::Exact { .. })
                    && intent.register != Obligation::Complete
                {
                    return Err(Hold::Recorded);
                }
                AbandonedRestoreRelease::PublishedExact
            }
            Ok(Some(_)) => return Err(Hold::PublishedContentMismatch),
            Err(_) => return Err(Hold::Native),
        };
        // A published stage leaf shares the published inode; unlinking the
        // stage name leaves the published file in place.
        retire_recorded_stage(
            activity,
            &parent,
            OsStr::new(&intent.stage.stage_name),
            intent.stage.directory,
            OsStr::new(&intent.stage.leaf_name),
            intent.stage.leaf,
        )
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::InvalidData => Hold::StageChanged,
            _ => Hold::Native,
        })?;
        remove_intent(activity, entry, operation_id, bytes)?;
        Ok(release)
    }

    fn remove_intent(
        activity: &Arc<ActivityLease>,
        entry: &MediaEntry,
        operation_id: &str,
        bytes: &[u8],
    ) -> Result<(), Hold> {
        let name = intent_filename(operation_id).ok_or(Hold::Unreadable)?;
        let health = MediaDirectory::open(
            Arc::clone(activity.journal_root()),
            Path::new("health/offload"),
        )
        .map_err(|_| Hold::Native)?;
        let intents = health
            .open_child(OsStr::new("restore-intents"))
            .map_err(|_| Hold::Native)?
            .ok_or(Hold::Native)?;
        let observed = intents
            .observe_file(&name)
            .map_err(|_| Hold::Native)?
            .ok_or(Hold::Unreadable)?;
        let read = digest_reader(&mut &bytes[..]).map_err(|_| Hold::Unreadable)?;
        if observed.identity != entry.identity || observed.digest != read {
            return Err(Hold::Unreadable);
        }
        let claim = MediaStage::create(Arc::clone(&health), Arc::clone(activity))
            .map_err(|_| Hold::Native)?;
        let mut removal = intents.remove_observed(&name, &observed, claim);
        if removal.receipt().cleanup != Cleanup::Complete {
            removal.cleanup();
        }
        let receipt = removal.receipt();
        if receipt.effect == RemovalEffect::Deleted
            && receipt.original_name == OriginalName::Absent
            && receipt.claim_confirmation == Confirmation::Confirmed
            && receipt.cleanup == Cleanup::Complete
            && (cfg!(windows)
                || (receipt.source_durability == Durability::Synced
                    && receipt.claim_durability == Durability::Synced))
            && !receipt
                .failures
                .iter()
                .any(|f| f.phase != RemovalPhase::Cleanup)
        {
            Ok(())
        } else {
            Err(Hold::Native)
        }
    }

    fn retire_empty_metadata_stages(
        activity: &Arc<ActivityLease>,
    ) -> Result<Vec<AbandonedRestore>, std::io::Error> {
        let census = match observe_obligations(activity.journal_root()) {
            Ok(census) => census,
            Err(error) => return census_miss(error),
        };
        let health = match MediaDirectory::open(
            Arc::clone(activity.journal_root()),
            Path::new("health/offload"),
        ) {
            Ok(health) => health,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return census_miss(error),
        };
        let mut retired = Vec::new();
        for obligation in census.entries() {
            let NativeObligation::Stage { relative, entry } = obligation else {
                continue;
            };
            if entry.kind != JournalEntryKind::Directory
                || !is_stage_namespace_name(&entry.name)
                || relative.parent() != Some(Path::new("health/offload"))
            {
                continue;
            }
            if let Ok(true) = retire_unrecorded_stage_where(
                activity,
                &health,
                &entry.name,
                entry.identity,
                &|_| false,
            ) {
                retired.push(AbandonedRestore {
                    operation_id: None,
                    outcome: Ok(AbandonedRestoreRelease::Undone),
                });
            }
        }
        Ok(retired)
    }
}

#[cfg(all(
    test,
    feature = "full-tests",
    any(target_os = "linux", target_os = "macos")
))]
#[allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::disallowed_methods,
    reason = "native fixture assertions"
)]
mod tests;

#[cfg(all(test, feature = "full-tests", windows))]
#[allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::disallowed_methods,
    reason = "native fixture assertions"
)]
mod windows_tests;
