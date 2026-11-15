// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Remove older backups after an erase.
//!
//! The trash's erase unlinks a thing from the journal; older backups still
//! hold it. Once a backup has finished without it, the owner can choose to
//! remove every backup older than the newest, so no backup can bring it back.
//! An offloaded-media archive goes too when everything it holds is erased;
//! one that holds media the owner still has stays, because it is that media's
//! only copy. Backups kept from before a restore go only when the owner says.
//!
//! "Bring it back" means the product's restores. Nothing here rewrites a
//! backup or prunes: forgetting a backup makes what only it held unreachable
//! to every restore at once, while the bytes stay in storage, encrypted with
//! the owner's key, until a later nightly prune repacks them, which at
//! today's 5% tolerance may be never.
//!
//! Not in scope: removals recorded outside the trash (a faces wipe, voice-only
//! release, turning backup off). Older backups hold those too, and nothing
//! here offers to remove them.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Duration, FixedOffset, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use solstone_core_backup::archive_readback::OFFLOAD_ARCHIVE_TAG;
use solstone_core_backup::{OLDER_BACKUPS, get_backup_config, mutate_older_backups};
use solstone_core_backup_runtime::engine::KEPT_BEFORE_RESTORE_TAG;
use solstone_core_backup_runtime::older_backups::{
    self as repository, FORGET_BATCH, Repository, SnapshotFacts, storage_word,
};
use solstone_core_backup_runtime::{ARCHIVE_TAG, BackupServices, restic_tree_path};
use solstone_core_journal_io::JournalRoot;
use solstone_core_journal_io::locking::{DEFAULT_LOCK_POLL_INTERVAL, FileLock, LockOptions};
use solstone_core_journal_io::paths::SegmentLayout;
use solstone_core_removals::{Action, Entry, Removal, Surface, Unit};
use solstone_core_retention::archive_catalog::{
    ArchiveWitness, SegmentCatalog, read_complete_catalog,
};
use solstone_core_retention::tombstone::TOMBSTONE_NAME;
use solstone_core_segment::{OwnerDeletionState, owner_deletion_state};

/// How long "done" and "partly done" stay on the backup page.
const DONE_SHOWN_DAYS: i64 = 7;
/// With nothing settled and no listing, erases older than this aren't looked
/// for: rotation keeps no regular backup past a year.
const UNSURVEYED_LOOKBACK_DAYS: i64 = 400;
/// The run lock, a sidecar never unlinked, shared by the page and the CLI.
const RUN_LOCK: &str = "health/older-backups";

/// One backup, by id and the time restic recorded for it.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Dated {
    pub id: String,
    pub time: String,
}

impl Dated {
    fn of(facts: &SnapshotFacts) -> Self {
        Self {
            id: facts.id.clone(),
            time: facts.time.clone(),
        }
    }

    fn instant(&self) -> Option<DateTime<FixedOffset>> {
        DateTime::parse_from_rfc3339(&self.time).ok()
    }
}

/// What the destination held at the last listing, sorted for the owner.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Survey {
    /// When it was listed (unix seconds).
    pub time: i64,
    pub newest: Option<Dated>,
    /// Every regular backup but the newest, oldest first.
    pub older: Vec<Dated>,
    pub kept_before_restore: Vec<Dated>,
    /// Media archives that hold only erased things.
    pub archives_removable: Vec<Dated>,
    /// Media archives that stay and may still hold something erased: they
    /// also hold media the owner still has, or the offload record doesn't
    /// name what they hold.
    pub archives_mixed: Vec<Dated>,
    /// Media archives that hold nothing erased.
    pub archives_kept: usize,
    /// Whether the storage keeps what's deleted: `removes`, `versions`,
    /// `locks`, `hidden_files` or `unknown`.
    pub storage: String,
    pub operated: bool,
}

/// A removal under way, or one that stopped.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Run {
    pub started: i64,
    pub updated: i64,
    pub keep: String,
    pub keep_time: String,
    pub include_kept: bool,
    pub total: usize,
    pub removed: usize,
    pub archives_removed: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped: Option<String>,
}

/// The last finished removal.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Done {
    pub at: i64,
    /// When the backup that was kept started: the claim is about what was
    /// erased before this, never before `at`.
    pub kept_newest_time: String,
    pub removed: usize,
    pub archives_removed: usize,
    pub archives_mixed: usize,
    pub kept_before_restore_left: usize,
    pub storage: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct State {
    #[serde(default)]
    pub survey: Option<Survey>,
    /// The kept backup's start at the last finished removal: erases whose
    /// files were gone before it are settled.
    #[serde(default)]
    pub settled_through: Option<String>,
    #[serde(default)]
    pub run: Option<Run>,
    #[serde(default)]
    pub done: Option<Done>,
}

pub fn load_state(backup: &Map<String, Value>) -> State {
    backup
        .get(OLDER_BACKUPS)
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default()
}

/// Every write goes through the backup config's locked read-modify-write.
fn store(journal: &Path, change: impl FnOnce(&mut State)) -> Result<(), String> {
    mutate_older_backups(journal, |object| {
        let mut state: State =
            serde_json::from_value(Value::Object(object.clone())).unwrap_or_default();
        change(&mut state);
        let next = serde_json::to_value(&state)
            .ok()
            .and_then(|value| value.as_object().cloned())
            .unwrap_or_default();
        let changed = *object != next;
        *object = next;
        (changed, ())
    })
    .map_err(|_| "state_unwritable".to_owned())
}

// ------------------------------------------------------------------ erases

/// An erase the journal recorded, and when its files were gone.
#[derive(Clone, Debug)]
pub struct Erase {
    pub at: DateTime<FixedOffset>,
    pub unit: Unit,
    /// The entry's id and operation: between them they name the trash item.
    names: String,
}

fn settled(state: &State) -> Option<DateTime<FixedOffset>> {
    state
        .settled_through
        .as_deref()
        .and_then(|time| DateTime::parse_from_rfc3339(time).ok())
}

/// Erases after the last finished removal (or, before any, after the oldest
/// backup the listing knows, or a year back). `whole` reads every entry, as a
/// removal must; a page's look skips names dated over a month earlier.
pub fn unsettled_erases(journal: &Path, state: &State, now: i64, whole: bool) -> Vec<Erase> {
    let since = settled(state)
        .map(|time| time.with_timezone(&Utc))
        .or_else(|| {
            let survey = state.survey.as_ref()?;
            survey
                .older
                .iter()
                .chain(&survey.kept_before_restore)
                .chain(&survey.newest)
                .filter_map(Dated::instant)
                .min()
                .map(|time| time.with_timezone(&Utc) - Duration::seconds(1))
        })
        .unwrap_or_else(|| {
            DateTime::from_timestamp(now, 0).unwrap_or_default()
                - Duration::days(UNSURVEYED_LOOKBACK_DAYS)
        });
    solstone_core_removals::erased_since(journal, since, !whole)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|entry: Entry| {
            Some(Erase {
                at: DateTime::parse_from_rfc3339(&entry.at).ok()?,
                names: alphanumeric(&format!("{}{}", entry.id, entry.operation)),
                unit: entry.unit,
            })
        })
        .collect()
}

fn alphanumeric(value: &str) -> String {
    value.chars().filter(char::is_ascii_alphanumeric).collect()
}

// ---------------------------------------------------------------- archives

/// Media archives sorted by what they hold. Fail closed: an archive is
/// removable only when the offload record is whole and names it, and every
/// file it holds belongs to a recording the owner erased (not one still
/// waiting in the trash) and is gone from the journal.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ArchiveClasses {
    pub removable: BTreeSet<String>,
    /// Kept, and may still hold something erased.
    pub mixed: BTreeSet<String>,
}

pub fn classify_archives(journal: &Path, archives: &[SnapshotFacts]) -> ArchiveClasses {
    let unknown = || ArchiveClasses {
        removable: BTreeSet::new(),
        mixed: archives.iter().map(|archive| archive.id.clone()).collect(),
    };
    if archives.is_empty() {
        return ArchiveClasses::default();
    }
    let Ok(root) = JournalRoot::open(journal).map(Arc::new) else {
        return unknown();
    };
    let Ok(catalog) = read_complete_catalog(&root) else {
        return unknown();
    };
    if catalog.degraded()
        || catalog
            .days
            .iter()
            .any(|day| day.degraded() || day.segments.iter().any(SegmentCatalog::degraded))
    {
        return unknown();
    }
    // Per archive: (erased files, kept files). An archive no record names
    // stays at (0, 0): kept, and it may hold anything.
    let mut held: BTreeMap<&str, (usize, usize)> = archives
        .iter()
        .map(|archive| (archive.id.as_str(), (0, 0)))
        .collect();
    let mut discovered = BTreeMap::new();
    for day in &catalog.days {
        for segment in &day.segments {
            for manifest in &segment.history {
                for witness in &manifest.files {
                    let matching: Vec<&str> = archives
                        .iter()
                        .filter(|archive| {
                            !witness.snapshot_id.is_empty()
                                && archive.id.starts_with(&witness.snapshot_id)
                        })
                        .map(|archive| archive.id.as_str())
                        .collect();
                    if matching.is_empty() {
                        continue;
                    }
                    let erased = erased_file(&root, &mut discovered, segment, witness);
                    for id in matching {
                        let counts = held.entry(id).or_default();
                        if erased {
                            counts.0 += 1;
                        } else {
                            counts.1 += 1;
                        }
                    }
                }
            }
        }
    }
    let mut classes = ArchiveClasses::default();
    for (id, counts) in held {
        match counts {
            (0, 0) => {
                classes.mixed.insert(id.to_owned());
            }
            (0, _) => {}
            (_, 0) => {
                classes.removable.insert(id.to_owned());
            }
            _ => {
                classes.mixed.insert(id.to_owned());
            }
        }
    }
    classes
}

/// Whether a witnessed file belongs to an erased recording: its folder holds
/// the owner's tombstone, the item that tombstone names is no longer in the
/// trash, and the file itself is gone from the folder.
fn erased_file(
    root: &Arc<JournalRoot>,
    discovered: &mut BTreeMap<String, Option<Vec<(String, String, PathBuf)>>>,
    segment: &SegmentCatalog,
    witness: &ArchiveWitness,
) -> bool {
    let day = root.canonical_path().join("chronicle").join(&segment.day);
    let path = match (witness.layout, &witness.segment_dir) {
        (Some(SegmentLayout::Direct), Some(dir)) => day.join(dir),
        (Some(SegmentLayout::Named), Some(dir)) => day.join(&segment.stream).join(dir),
        _ => {
            // A historical record names only stream and key: the one folder
            // with that identity that day, as restore resolves it.
            let day_segments = discovered.entry(segment.day.clone()).or_insert_with(|| {
                crate::restore::owned::discover(root, &segment.day)
                    .ok()
                    .map(|segments| {
                        segments
                            .iter()
                            .filter_map(|candidate| {
                                let identity = candidate.locator_identity().ok()?;
                                Some((
                                    identity.stream.to_string(),
                                    identity.key.to_string(),
                                    candidate.path().to_path_buf(),
                                ))
                            })
                            .collect()
                    })
            });
            let found: Option<Vec<PathBuf>> = day_segments.as_ref().map(|segments| {
                segments
                    .iter()
                    .filter(|(stream, key, _)| *stream == segment.stream && *key == segment.segment)
                    .map(|(_, _, path)| path.clone())
                    .collect()
            });
            match found.as_deref() {
                Some([only]) => only.clone(),
                _ => return false,
            }
        }
    };
    if !matches!(owner_deletion_state(&path), Ok(OwnerDeletionState::Deleted)) {
        return false;
    }
    let file_gone = std::fs::symlink_metadata(path.join(&witness.file.name))
        .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound);
    if !file_gone {
        return false;
    }
    // An item still in the trash (even one whose record can't be read) can
    // be recovered, so its media isn't erased.
    read_tombstone(&path).is_some_and(|tombstone| {
        std::fs::symlink_metadata(
            root.canonical_path()
                .join(solstone_core_journal::TRASH_DIR)
                .join(&tombstone.cid),
        )
        .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
    })
}

#[derive(Deserialize)]
struct Tombstone {
    cid: String,
    #[serde(default)]
    manifest: Vec<String>,
}

fn read_tombstone(segment_dir: &Path) -> Option<Tombstone> {
    let path = segment_dir.join(TOMBSTONE_NAME);
    let metadata = std::fs::symlink_metadata(&path).ok()?;
    if !metadata.is_file() {
        return None;
    }
    let tombstone: Tombstone = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    let safe = !tombstone.cid.is_empty()
        && !tombstone.cid.contains(['/', '\\'])
        && tombstone.cid != ".."
        && tombstone.cid != ".";
    safe.then_some(tombstone)
}

// ------------------------------------------------------------------ survey

fn is_archive(facts: &SnapshotFacts) -> bool {
    facts.has_tag(ARCHIVE_TAG) || facts.has_tag(OFFLOAD_ARCHIVE_TAG)
}

/// Sort a listing. Pure, so the decision has one place to be tested.
pub fn sort_listing(
    journal: &Path,
    snapshots: Vec<SnapshotFacts>,
    storage: &str,
    operated: bool,
    now: i64,
) -> Survey {
    let (mut archives, rest): (Vec<_>, Vec<_>) = snapshots.into_iter().partition(is_archive);
    let (mut kept, mut regular): (Vec<_>, Vec<_>) = rest
        .into_iter()
        .partition(|facts| facts.has_tag(KEPT_BEFORE_RESTORE_TAG));
    for list in [&mut archives, &mut kept, &mut regular] {
        list.sort_by_key(SnapshotFacts::instant);
    }
    let newest = regular.pop();
    let classes = classify_archives(journal, &archives);
    let mut survey = Survey {
        time: now,
        newest: newest.as_ref().map(Dated::of),
        older: regular.iter().map(Dated::of).collect(),
        kept_before_restore: kept.iter().map(Dated::of).collect(),
        storage: storage.to_owned(),
        operated,
        ..Survey::default()
    };
    for archive in &archives {
        if classes.removable.contains(&archive.id) {
            survey.archives_removable.push(Dated::of(archive));
        } else if classes.mixed.contains(&archive.id) {
            survey.archives_mixed.push(Dated::of(archive));
        } else {
            survey.archives_kept += 1;
        }
    }
    survey
}

fn list(journal: &Path, repository: &Repository<'_>, now: i64) -> Result<Survey, String> {
    let snapshots = repository.snapshots()?;
    Ok(listing(journal, repository, snapshots, now))
}

fn listing(
    journal: &Path,
    repository: &Repository<'_>,
    snapshots: Vec<SnapshotFacts>,
    now: i64,
) -> Survey {
    let storage = storage_word(repository.storage());
    sort_listing(journal, snapshots, storage, repository.operated(), now)
}

/// List the destination and keep what it holds, when an erase is unsettled.
/// Called after a backup or prune finishes, with the backup's own read
/// credential. A failure is quiet (the page then waits), and nothing is
/// listed while no erase needs it.
pub fn survey_if_pending(journal: &Path, services: &BackupServices<'_>) {
    if cfg!(target_os = "ios") {
        return;
    }
    let Ok(backup) = get_backup_config(journal) else {
        return;
    };
    let state = load_state(&backup);
    let now = services.clock.now_unix();
    if unsettled_erases(journal, &state, now, false).is_empty() {
        return;
    }
    let Ok(Some(repository)) = repository::open_read(journal, services) else {
        return;
    };
    if let Ok(survey) = list(journal, &repository, now) {
        let _ = store(journal, |state| state.survey = Some(survey));
    }
}

// -------------------------------------------------------------------- lock

fn lock_path(journal: &Path) -> PathBuf {
    journal.join(RUN_LOCK)
}

fn hold_run_lock(journal: &Path) -> Result<FileLock, Refusal> {
    let path = lock_path(journal);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|_| Refusal::Failed("state_unwritable".into()))?;
    }
    solstone_core_journal_io::locking::hold_lock(
        &path,
        LockOptions {
            timeout: std::time::Duration::ZERO,
            poll_interval: DEFAULT_LOCK_POLL_INTERVAL,
            mode: Some(0o600),
        },
    )
    .map_err(|_| Refusal::Running)
}

/// Whether a removal holds the run lock now, here or in another process.
pub fn run_lock_held(journal: &Path) -> bool {
    solstone_core_journal_io::locking::lock_is_held(lock_path(journal)).unwrap_or(false)
}

// ------------------------------------------------------------------- view

/// The last successful backup: when it was recorded, its id, and whether it
/// read everything it was meant to.
fn last_success(backup: &Map<String, Value>) -> Option<(i64, String, bool)> {
    let last = backup.get("last_backup")?;
    let ok = if last.get("status").and_then(Value::as_str) == Some("ok") {
        last
    } else {
        last.get("last_success").filter(|value| value.is_object())?
    };
    Some((
        ok.get("time").and_then(Value::as_i64)?,
        ok.get("snapshot_id").and_then(Value::as_str)?.to_owned(),
        ok.get("error_reason").is_none_or(Value::is_null),
    ))
}

fn same_snapshot(left: &str, right: &str) -> bool {
    !left.is_empty() && !right.is_empty() && (left.starts_with(right) || right.starts_with(left))
}

/// Whether something older than `at` can bring an erase back.
fn restorable(survey: &Survey, at: DateTime<FixedOffset>) -> bool {
    survey
        .older
        .iter()
        .chain(&survey.kept_before_restore)
        .any(|dated| dated.instant().is_some_and(|time| time < at))
        || !survey.archives_removable.is_empty()
        || !survey.archives_mixed.is_empty()
}

/// What the backup page and the trash page show. Local reads only.
///
/// `running` is the page's own operation slot holding a removal.
pub fn view(journal: &Path, now: i64, running: bool) -> Value {
    if cfg!(target_os = "ios") {
        return json!({"state": "hidden"});
    }
    let Ok(backup) = get_backup_config(journal) else {
        return json!({"state": "hidden"});
    };
    if backup.get("enabled") != Some(&Value::Bool(true)) {
        return json!({"state": "hidden"});
    }
    let state = load_state(&backup);
    if running || run_lock_held(journal) {
        let (removed, total) = state
            .run
            .as_ref()
            .map_or((0, 0), |run| (run.removed, run.total));
        return json!({"state": "running", "removed": removed, "total": total});
    }
    if let Some(run) = &state.run {
        return json!({
            "state": "stopped",
            "removed": run.removed,
            "total": run.total,
            "include_kept": run.include_kept,
            "reason": run.stopped.clone().unwrap_or_else(|| "interrupted".into()),
        });
    }
    let erases = unsettled_erases(journal, &state, now, false);
    let last = last_success(&backup);
    if !erases.is_empty() {
        let survey = state.survey.as_ref();
        if let (Some(survey), Some((_, id, complete))) = (survey, &last)
            && let Some(newest) = survey.newest.as_ref()
            && same_snapshot(&newest.id, id)
        {
            let newest_time = newest.instant();
            let covered = erases.iter().any(|erase| {
                newest_time.is_some_and(|newest| erase.at < newest) && restorable(survey, erase.at)
            });
            if covered && *complete {
                return ready_view(survey);
            }
            if covered {
                // The newest backup couldn't read some files, so older ones
                // may hold their only copy: nothing older is offered.
                return json!({"state": "waiting", "newest_incomplete": true});
            }
        }
        let waiting = erases.iter().any(|erase| {
            last.as_ref()
                .is_some_and(|(time, _, _)| *time < erase.at.timestamp())
                || survey.is_some_and(|survey| restorable(survey, erase.at))
        });
        if waiting {
            return json!({"state": "waiting"});
        }
    }
    if let Some(done) = &state.done
        && now - done.at < DONE_SHOWN_DAYS * 24 * 60 * 60
    {
        let kept = state
            .survey
            .as_ref()
            .map_or(done.kept_before_restore_left, |survey| {
                survey.kept_before_restore.len()
            })
            .min(done.kept_before_restore_left);
        let partly = done.archives_mixed > 0
            || kept > 0
            || matches!(done.storage.as_str(), "versions" | "locks" | "hidden_files");
        return json!({
            "state": if partly { "partly_done" } else { "done" },
            "at": done.at,
            "before": done.kept_newest_time,
            "removed": done.removed,
            "archives_removed": done.archives_removed,
            "archives_mixed": done.archives_mixed,
            "kept_before_restore": kept,
            "storage": done.storage,
        });
    }
    json!({"state": "hidden"})
}

fn ready_view(survey: &Survey) -> Value {
    json!({
        "state": "ready",
        "newest": survey.newest.as_ref().map(|dated| dated.time.clone()),
        "older": survey.older.len(),
        "older_from": survey.older.first().map(|dated| dated.time.clone()),
        "older_to": survey.older.last().map(|dated| dated.time.clone()),
        "archives_removable": survey.archives_removable.len(),
        "archives_mixed": survey.archives_mixed.len(),
        "archives_kept": survey.archives_kept,
        "kept_before_restore": survey.kept_before_restore.len(),
        "kept_before_restore_to": survey.kept_before_restore.last().map(|dated| dated.time.clone()),
        "storage": survey.storage,
        "operated": survey.operated,
    })
}

// ---------------------------------------------------------------- removal

/// Why a removal didn't run, or stopped.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Refusal {
    /// Backup isn't set up here, or this platform can't do it.
    Unavailable,
    /// Another removal is running.
    Running,
    /// No erase is waiting on older backups, or none has a backup after it,
    /// or the newest backup isn't the last one this journal recorded.
    Waiting,
    /// The newest backup couldn't read some files, so it can't stand in for
    /// the older ones.
    NewestIncomplete,
    /// The newest backup still holds an erased thing at one of its paths.
    NewestHoldsErased,
    /// The look into the newest backup didn't see what it must hold, so its
    /// silence about erased things means nothing.
    CheckBlind,
    /// Nothing older is left to remove.
    Nothing,
    /// The destination couldn't be reached or read.
    Failed(String),
    /// Stopped part way: what was removed stays removed.
    Stopped {
        reason: String,
        removed: usize,
        total: usize,
    },
}

impl Refusal {
    pub fn code(&self) -> &str {
        match self {
            Self::Unavailable => "unavailable",
            Self::Running => "running",
            Self::Waiting => "waiting",
            Self::NewestIncomplete => "newest_incomplete",
            Self::NewestHoldsErased => "newest_holds_erased",
            Self::CheckBlind => "check_blind",
            Self::Nothing => "nothing_to_remove",
            Self::Failed(reason) => reason,
            Self::Stopped { reason, .. } => reason,
        }
    }
}

/// What a removal would take, for the CLI's plan and the confirm.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Plan {
    pub keep: Dated,
    keep_paths: Vec<String>,
    pub older: Vec<Dated>,
    pub archives_removable: Vec<Dated>,
    pub archives_mixed: usize,
    pub archives_kept: usize,
    pub kept_before_restore: Vec<Dated>,
    pub storage: String,
    pub operated: bool,
}

impl Plan {
    pub fn targets(&self, include_kept: bool) -> Vec<String> {
        let mut ids: Vec<String> = self
            .older
            .iter()
            .chain(&self.archives_removable)
            .map(|dated| dated.id.clone())
            .collect();
        if include_kept {
            ids.extend(
                self.kept_before_restore
                    .iter()
                    .map(|dated| dated.id.clone()),
            );
        }
        ids
    }
}

/// The journal-relative paths an erased recording had: its files where they
/// were. (Its trash folder is checked for every kind, by name.)
fn erased_paths(journal: &Path, erase: &Erase) -> Vec<String> {
    let Unit::Segment {
        day,
        stream,
        segment,
    } = &erase.unit
    else {
        return Vec::new();
    };
    let day_dir = journal.join("chronicle").join(day);
    let candidates = [day_dir.join(stream).join(segment), day_dir.join(segment)];
    let Some(tombstone) = candidates.iter().find_map(|dir| read_tombstone(dir)) else {
        return Vec::new();
    };
    tombstone
        .manifest
        .into_iter()
        .filter(|rel| !rel.starts_with('/') && !rel.split('/').any(|part| part == ".."))
        .collect()
}

fn open<'a>(journal: &Path, services: &'a BackupServices<'a>) -> Result<Repository<'a>, Refusal> {
    if cfg!(target_os = "ios") {
        return Err(Refusal::Unavailable);
    }
    match repository::open(journal, services) {
        Ok(Some(repository)) => Ok(repository),
        Ok(None) => Err(Refusal::Unavailable),
        Err(reason) => Err(Refusal::Failed(reason)),
    }
}

/// Look at the destination and say what a removal would take. Nothing
/// changes but the kept listing.
pub fn plan(journal: &Path, services: &BackupServices<'_>) -> Result<Plan, Refusal> {
    let repository = open(journal, services)?;
    plan_with(journal, services, &repository).map(|(plan, _)| plan)
}

fn plan_with(
    journal: &Path,
    services: &BackupServices<'_>,
    repository: &Repository<'_>,
) -> Result<(Plan, Vec<Erase>), Refusal> {
    let now = services.clock.now_unix();
    let snapshots = repository.snapshots().map_err(Refusal::Failed)?;
    let survey = listing(journal, repository, snapshots.clone(), now);
    let _ = store(journal, |state| state.survey = Some(survey.clone()));
    let backup = get_backup_config(journal).map_err(|_| Refusal::Unavailable)?;
    let state = load_state(&backup);
    let Some(keep) = survey.newest.clone() else {
        return Err(Refusal::Nothing);
    };
    // The backup kept must be this journal's last recorded success, and whole.
    let Some((_, last_id, complete)) = last_success(&backup) else {
        return Err(Refusal::Waiting);
    };
    if !same_snapshot(&keep.id, &last_id) {
        return Err(Refusal::Waiting);
    }
    if !complete {
        return Err(Refusal::NewestIncomplete);
    }
    let keep_time = keep.instant().ok_or(Refusal::Nothing)?;
    let covered: Vec<Erase> = unsettled_erases(journal, &state, now, true)
        .into_iter()
        .filter(|erase| erase.at < keep_time && restorable(&survey, erase.at))
        .collect();
    if covered.is_empty() {
        return Err(Refusal::Waiting);
    }
    let keep_paths = snapshots
        .iter()
        .find(|facts| facts.id == keep.id)
        .map(|facts| facts.paths.clone())
        .unwrap_or_default();
    let plan = Plan {
        keep,
        keep_paths,
        older: survey.older,
        archives_removable: survey.archives_removable,
        archives_mixed: survey.archives_mixed.len(),
        archives_kept: survey.archives_kept,
        kept_before_restore: survey.kept_before_restore,
        storage: survey.storage,
        operated: survey.operated,
    };
    Ok((plan, covered))
}

fn kept_left_by_last_run(journal: &Path) -> bool {
    get_backup_config(journal)
        .map(|backup| load_state(&backup))
        .ok()
        .and_then(|state| state.done)
        .is_some_and(|done| done.kept_before_restore_left > 0)
}

/// A plan that takes only the backups kept from before a restore.
fn kept_only_plan(
    journal: &Path,
    services: &BackupServices<'_>,
    repository: &Repository<'_>,
) -> Result<Plan, Refusal> {
    let snapshots = repository.snapshots().map_err(Refusal::Failed)?;
    let survey = listing(
        journal,
        repository,
        snapshots.clone(),
        services.clock.now_unix(),
    );
    let keep = survey.newest.clone().ok_or(Refusal::Nothing)?;
    if survey.kept_before_restore.is_empty() {
        return Err(Refusal::Nothing);
    }
    let keep_paths = snapshots
        .iter()
        .find(|facts| facts.id == keep.id)
        .map(|facts| facts.paths.clone())
        .unwrap_or_default();
    Ok(Plan {
        keep,
        keep_paths,
        older: Vec::new(),
        archives_removable: Vec::new(),
        archives_mixed: survey.archives_mixed.len(),
        archives_kept: survey.archives_kept,
        kept_before_restore: survey.kept_before_restore,
        storage: survey.storage,
        operated: survey.operated,
    })
}

/// The kept backup must hold none of the covered erases: not a recording's
/// files where they were, and no erased thing's folder in the trash. The same
/// look must see the journal's own `config` folder, or it proves nothing.
fn check_newest(
    journal: &Path,
    repository: &Repository<'_>,
    plan: &Plan,
    covered: &[Erase],
) -> Result<(), Refusal> {
    let [base] = plan.keep_paths.as_slice() else {
        return Err(Refusal::CheckBlind);
    };
    let base = restic_tree_path(Path::new(base));
    let base = base.trim_end_matches('/');
    let control = format!("{base}/config");
    let trash = format!("{base}/{}", solstone_core_journal::TRASH_DIR);
    let mut wanted = vec![control.clone(), trash.clone()];
    wanted.extend(
        covered
            .iter()
            .flat_map(|erase| erased_paths(journal, erase))
            .map(|rel| format!("{base}/{rel}")),
    );
    let found = repository
        .present(&plan.keep.id, &wanted)
        .map_err(Refusal::Failed)?;
    if !found.iter().any(|path| *path == control) {
        return Err(Refusal::CheckBlind);
    }
    let trash_prefix = format!("{trash}/");
    for path in &found {
        if path == &control || path.starts_with(&format!("{control}/")) || path == &trash {
            continue;
        }
        if let Some(item) = path.strip_prefix(&trash_prefix) {
            let item = alphanumeric(item.split('/').next().unwrap_or_default());
            if !item.is_empty() && covered.iter().any(|erase| erase.names.contains(&item)) {
                return Err(Refusal::NewestHoldsErased);
            }
            continue;
        }
        return Err(Refusal::NewestHoldsErased);
    }
    Ok(())
}

/// Remove every backup older than the newest, the media archives that hold
/// only erased things, and, when `include_kept`, the backups kept from before
/// a restore. `progress` hears (removed, total) after each batch.
pub fn remove(
    journal: &Path,
    services: &BackupServices<'_>,
    include_kept: bool,
    surface: Surface,
    progress: &dyn Fn(usize, usize),
) -> Result<Done, Refusal> {
    let _lock = hold_run_lock(journal)?;
    let repository = open(journal, services)?;
    let prior = get_backup_config(journal)
        .map(|backup| load_state(&backup))
        .unwrap_or_default()
        .run;
    let (plan, covered) = match plan_with(journal, services, &repository) {
        // A run that removed everything and stopped before it wrote its
        // result: finish it now, once.
        Err(Refusal::Waiting | Refusal::Nothing) if prior.is_some() => {
            return finish_stopped_run(journal, services, &repository, prior, surface);
        }
        // Partly done's "remove them": the backups kept from before a
        // restore that the last run left, with no new erase needed.
        Err(Refusal::Waiting) if include_kept && kept_left_by_last_run(journal) => {
            (kept_only_plan(journal, services, &repository)?, Vec::new())
        }
        other => other?,
    };
    check_newest(journal, &repository, &plan, &covered)?;

    let targets = plan.targets(include_kept);
    if targets.is_empty() {
        return Err(Refusal::Nothing);
    }
    let total = targets.len();
    let started = services.clock.now_unix();
    let archives: BTreeSet<&str> = plan
        .archives_removable
        .iter()
        .map(|dated| dated.id.as_str())
        .collect();
    let mut run = Run {
        started,
        updated: started,
        keep: plan.keep.id.clone(),
        keep_time: plan.keep.time.clone(),
        include_kept,
        total,
        ..Run::default()
    };
    let record = |run: &Run| {
        let mut run = run.clone();
        run.updated = services.clock.now_unix();
        let _ = store(journal, |state| state.run = Some(run));
    };
    record(&run);
    progress(0, total);
    for batch in targets.chunks(FORGET_BATCH) {
        let forgot = repository.forget(batch);
        // The listing decides: a target someone else already forgot counts
        // as gone, and an exit code alone never stops a run.
        let left = match (&forgot, repository.snapshots()) {
            (_, Ok(after)) => after
                .into_iter()
                .map(|facts| facts.id)
                .collect::<BTreeSet<_>>(),
            (forgot, Err(listing)) => {
                let reason = forgot.clone().err().unwrap_or(listing);
                run.stopped = Some(reason.clone());
                record(&run);
                return Err(Refusal::Stopped {
                    reason: reason.clone(),
                    removed: run.removed,
                    total,
                });
            }
        };
        let gone = batch.iter().filter(|id| !left.contains(*id)).count();
        run.removed += gone;
        run.archives_removed += batch
            .iter()
            .filter(|id| !left.contains(*id) && archives.contains(id.as_str()))
            .count();
        if gone < batch.len() || !left.contains(&plan.keep.id) {
            let reason = forgot.err().unwrap_or_else(|| "residual".into());
            run.stopped = Some(reason.clone());
            record(&run);
            return Err(Refusal::Stopped {
                reason,
                removed: run.removed,
                total,
            });
        }
        record(&run);
        progress(run.removed, total);
    }
    let kept_left = if include_kept {
        0
    } else {
        plan.kept_before_restore.len()
    };
    finish(
        journal,
        services,
        &repository,
        &run,
        plan.operated,
        &plan.storage,
        plan.archives_mixed,
        kept_left,
        surface,
    )
}

#[allow(clippy::too_many_arguments)] // The one place a run's result is written.
fn finish(
    journal: &Path,
    services: &BackupServices<'_>,
    repository: &Repository<'_>,
    run: &Run,
    operated: bool,
    storage: &str,
    archives_mixed: usize,
    kept_before_restore_left: usize,
    surface: Surface,
) -> Result<Done, Refusal> {
    Removal::begin(journal, Action::OlderBackupsRemove, surface).record(Unit::Backup {
        storage: if operated { "operated" } else { "own" }.to_owned(),
    });
    let now = services.clock.now_unix();
    let done = Done {
        at: now,
        kept_newest_time: run.keep_time.clone(),
        removed: run.removed,
        archives_removed: run.archives_removed,
        archives_mixed,
        kept_before_restore_left,
        storage: storage.to_owned(),
    };
    let survey = list(journal, repository, now).ok();
    store(journal, |state| {
        state.run = None;
        state.done = Some(done.clone());
        state.settled_through = Some(run.keep_time.clone());
        if let Some(survey) = survey {
            state.survey = Some(survey);
        }
    })
    .map_err(Refusal::Failed)?;
    Ok(done)
}

/// A run whose every target is gone but whose result was never written.
fn finish_stopped_run(
    journal: &Path,
    services: &BackupServices<'_>,
    repository: &Repository<'_>,
    prior: Option<Run>,
    surface: Surface,
) -> Result<Done, Refusal> {
    let Some(mut run) = prior else {
        return Err(Refusal::Nothing);
    };
    let survey = list(journal, repository, services.clock.now_unix()).map_err(Refusal::Failed)?;
    let older_left = survey
        .older
        .iter()
        .chain(&survey.archives_removable)
        .count()
        + if run.include_kept {
            survey.kept_before_restore.len()
        } else {
            0
        };
    if older_left > 0 {
        // What's left holds nothing an unsettled erase needs gone: the old
        // run's record only says it stopped, so let it go.
        let _ = store(journal, |state| state.run = None);
        return Err(Refusal::Waiting);
    }
    run.removed = run.total;
    let kept_left = if run.include_kept {
        0
    } else {
        survey.kept_before_restore.len()
    };
    finish(
        journal,
        services,
        repository,
        &run,
        survey.operated,
        &survey.storage,
        survey.archives_mixed.len(),
        kept_left,
        surface,
    )
}

#[cfg(test)]
mod tests;
