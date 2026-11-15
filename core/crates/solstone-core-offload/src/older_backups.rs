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
    /// The boundary this run claims: erases before it are settled.
    pub keep_time: String,
    pub include_kept: bool,
    /// Only the kept-from-restore backups a finished removal left.
    #[serde(default)]
    pub kept_only: bool,
    /// Every backup this run chose, by id; what's gone is read back from a
    /// listing, never counted from exit codes.
    #[serde(default)]
    pub targets: Vec<String>,
    /// Which of them are media archives.
    #[serde(default)]
    pub archives: Vec<String>,
    pub total: usize,
    pub removed: usize,
    /// Whether the deletion record has this operation's entry.
    #[serde(default)]
    pub recorded: bool,
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
    /// The newest kept-from-restore backup left: "remove them" takes none newer.
    #[serde(default)]
    pub kept_before_restore_to: Option<String>,
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
        // A record this build can't read is left as it is, never reset.
        let Ok(mut state) = serde_json::from_value::<State>(Value::Object(object.clone())) else {
            return (false, Err("state_unreadable".to_owned()));
        };
        change(&mut state);
        let next = serde_json::to_value(&state)
            .ok()
            .and_then(|value| value.as_object().cloned())
            .unwrap_or_default();
        let changed = *object != next;
        *object = next;
        (changed, Ok(()))
    })
    .map_err(|_| "state_unwritable".to_owned())?
}

// ------------------------------------------------------------------ erases

/// An erase the journal recorded, and when its files were gone.
#[derive(Clone, Debug)]
pub struct Erase {
    pub at: DateTime<FixedOffset>,
    pub unit: Unit,
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
                unit: entry.unit,
            })
        })
        .collect()
}

// ---------------------------------------------------------------- archives

/// Media archives sorted by what they hold. Fail closed both ways: an
/// archive is removable only when the offload record is whole and names it,
/// and every file it holds belongs to a recording erased from the trash and
/// is gone from the journal. One the record doesn't name, or with any file
/// whose state can't be told, stays and counts as possibly holding something
/// erased, so "done" never claims more than is known.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ArchiveClasses {
    pub removable: BTreeSet<String>,
    /// Kept, and may still hold something erased.
    pub mixed: BTreeSet<String>,
}

/// One day's recordings, by (stream, key, folder), as restore resolves them.
type DayRecordings = Vec<(String, String, PathBuf)>;

/// What one archived file is, for this purpose.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Held {
    /// Its recording was erased from the trash and it's gone from the journal.
    Erased,
    /// The owner still has it, or it left some other way than a trash erase.
    Kept,
    /// Can't be told.
    Unknown,
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
    // Per archive: (erased, kept, unknown) files.
    let mut held: BTreeMap<&str, (usize, usize, usize)> = archives
        .iter()
        .map(|archive| (archive.id.as_str(), (0, 0, 0)))
        .collect();
    let mut discovered = BTreeMap::new();
    for day in &catalog.days {
        for segment in &day.segments {
            for manifest in &segment.history {
                for witness in &manifest.files {
                    let matching: Vec<&str> = archives
                        .iter()
                        .filter(|archive| {
                            // A short id could name several archives; one
                            // this short names none.
                            witness.snapshot_id.len() >= 8
                                && archive.id.starts_with(&witness.snapshot_id)
                        })
                        .map(|archive| archive.id.as_str())
                        .collect();
                    if matching.is_empty() {
                        continue;
                    }
                    let state = held_file(&root, &mut discovered, segment, witness);
                    for id in matching {
                        let counts = held.entry(id).or_default();
                        match state {
                            Held::Erased => counts.0 += 1,
                            Held::Kept => counts.1 += 1,
                            Held::Unknown => counts.2 += 1,
                        }
                    }
                }
            }
        }
    }
    let mut classes = ArchiveClasses::default();
    for (id, (erased, kept, unknown)) in held {
        if erased > 0 && kept == 0 && unknown == 0 {
            classes.removable.insert(id.to_owned());
        } else if erased > 0 || unknown > 0 || (erased, kept) == (0, 0) {
            classes.mixed.insert(id.to_owned());
        }
    }
    classes
}

/// What a witnessed file is: erased when its folder holds the owner's own
/// delete tombstone (the trash's, or an owner delete from before it), the
/// item that tombstone names is no longer in the trash, and the file is gone
/// from the folder.
fn held_file(
    root: &Arc<JournalRoot>,
    discovered: &mut BTreeMap<String, Option<DayRecordings>>,
    segment: &SegmentCatalog,
    witness: &ArchiveWitness,
) -> Held {
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
                _ => return Held::Unknown,
            }
        }
    };
    match owner_deletion_state(&path) {
        Ok(OwnerDeletionState::Deleted) => {}
        Ok(OwnerDeletionState::Live) if path.is_dir() => return Held::Kept,
        _ => return Held::Unknown,
    }
    match std::fs::symlink_metadata(path.join(&witness.file.name)) {
        Ok(_) => return Held::Kept,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Held::Unknown,
    }
    let Some(tombstone) = read_tombstone(&path) else {
        return Held::Unknown;
    };
    if tombstone.reason != "owner_segment_delete" {
        // Deleted some other way (a retention policy, an import's release):
        // not this feature's to remove.
        return Held::Kept;
    }
    // An item still in the trash (even one whose record can't be read) can
    // be recovered, so the owner still has it.
    if solstone_core_trash::left_since_copied(root.canonical_path(), &tombstone.cid) {
        Held::Erased
    } else {
        Held::Kept
    }
}

#[derive(Deserialize)]
struct Tombstone {
    cid: String,
    #[serde(default)]
    reason: String,
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
            timeout: std::time::Duration::from_secs(2),
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

/// Whether removing older backups now takes something that can bring an
/// erase back: a regular backup older than it, or an archive of only erased
/// media. (Backups kept from before a restore, and archives that also hold
/// media the owner has, are never taken without being chosen.)
fn offerable(survey: &Survey, at: DateTime<FixedOffset>) -> bool {
    survey
        .older
        .iter()
        .any(|dated| dated.instant().is_some_and(|time| time < at))
        || !survey.archives_removable.is_empty()
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
                newest_time.is_some_and(|newest| erase.at < newest) && offerable(survey, erase.at)
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
        // Waiting means a backup after the erase is still to come and
        // something older can bring it back meanwhile. Once a current
        // listing shows a backup after it, nothing a backup does changes
        // what's left, so it isn't waiting.
        let fresh_newest = match (survey, &last) {
            (Some(survey), Some((_, id, _))) => survey
                .newest
                .as_ref()
                .filter(|newest| same_snapshot(&newest.id, id))
                .and_then(Dated::instant),
            _ => None,
        };
        let waiting = erases.iter().any(|erase| {
            let outstanding = fresh_newest.is_none_or(|newest| newest <= erase.at);
            let held = last
                .as_ref()
                .is_some_and(|(time, _, _)| *time < erase.at.timestamp())
                || survey.is_some_and(|survey| restorable(survey, erase.at));
            outstanding && held
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
            "kept_before_restore_to": done.kept_before_restore_to,
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
    /// The newest backup still holds something that has left the trash.
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

/// Which removal a press makes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    /// Every older backup and every archive of only erased media, and the
    /// kept-from-restore backups when chosen.
    Older,
    /// Only the kept-from-restore backups a finished removal left.
    KeptOnly,
    /// The rest of a removal that stopped, as it was chosen.
    Resume,
}

/// What a press would take, for the CLI's preview and the confirm. The
/// preview and the removal share this one decision.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Plan {
    pub kind: Kind,
    pub keep: Dated,
    keep_paths: Vec<String>,
    /// The boundary this removal claims: erases before it are settled.
    pub boundary: String,
    pub older: Vec<Dated>,
    pub archives_removable: Vec<Dated>,
    pub archives_mixed: usize,
    pub archives_kept: usize,
    /// Kept-from-restore backups this press takes.
    pub kept_before_restore: Vec<Dated>,
    /// Kept-from-restore backups it leaves.
    pub kept_before_restore_left: Vec<Dated>,
    pub storage: String,
    pub operated: bool,
    targets: Vec<String>,
    archive_targets: Vec<String>,
}

impl Plan {
    pub fn targets(&self) -> &[String] {
        &self.targets
    }
}

/// The paths an erased recording had where it was. (Its trash folder is
/// covered by the trash check, for every kind.)
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

fn current_state(journal: &Path) -> State {
    get_backup_config(journal)
        .map(|backup| load_state(&backup))
        .unwrap_or_default()
}

fn paths_of(snapshots: &[SnapshotFacts], id: &str) -> Vec<String> {
    snapshots
        .iter()
        .find(|facts| facts.id == id)
        .map(|facts| facts.paths.clone())
        .unwrap_or_default()
}

fn at_or_before(dated: &Dated, bound: Option<&str>) -> bool {
    match (
        dated.instant(),
        bound.and_then(|bound| DateTime::parse_from_rfc3339(bound).ok()),
    ) {
        (Some(time), Some(bound)) => time <= bound,
        _ => false,
    }
}

/// Look at the destination and say what a press would take. Nothing changes
/// but the kept listing.
pub fn plan(
    journal: &Path,
    services: &BackupServices<'_>,
    include_kept: bool,
    kept_bound: Option<&str>,
) -> Result<Plan, Refusal> {
    let repository = open(journal, services)?;
    decide(journal, services, &repository, include_kept, kept_bound).map(|(plan, _)| plan)
}

/// The one decision a press makes. A run that stopped is resumed as it was
/// chosen, whatever this press asks; otherwise kept-from-restore backups
/// alone after a finished removal, or an ordinary removal.
fn decide(
    journal: &Path,
    services: &BackupServices<'_>,
    repository: &Repository<'_>,
    include_kept: bool,
    kept_bound: Option<&str>,
) -> Result<(Plan, Vec<Erase>), Refusal> {
    let now = services.clock.now_unix();
    let snapshots = repository.snapshots().map_err(Refusal::Failed)?;
    let survey = listing(journal, repository, snapshots.clone(), now);
    let _ = store(journal, |state| state.survey = Some(survey.clone()));
    let backup = get_backup_config(journal).map_err(|_| Refusal::Unavailable)?;
    let state = load_state(&backup);
    let base = |kind, keep: Dated, boundary: String| Plan {
        kind,
        keep_paths: paths_of(&snapshots, &keep.id),
        keep,
        boundary,
        older: Vec::new(),
        archives_removable: Vec::new(),
        archives_mixed: survey.archives_mixed.len(),
        archives_kept: survey.archives_kept,
        kept_before_restore: Vec::new(),
        kept_before_restore_left: survey.kept_before_restore.clone(),
        storage: survey.storage.clone(),
        operated: survey.operated,
        targets: Vec::new(),
        archive_targets: Vec::new(),
    };

    if let Some(run) = &state.run {
        // What the stopped run chose and hasn't removed yet.
        let left: BTreeSet<&str> = snapshots.iter().map(|facts| facts.id.as_str()).collect();
        let keep = Dated {
            id: run.keep.clone(),
            time: snapshots
                .iter()
                .find(|facts| facts.id == run.keep)
                .map(|facts| facts.time.clone())
                .ok_or_else(|| Refusal::Failed("kept_backup_missing".into()))?,
        };
        let mut plan = base(Kind::Resume, keep, run.keep_time.clone());
        plan.targets = run
            .targets
            .iter()
            .filter(|id| left.contains(id.as_str()))
            .cloned()
            .collect();
        plan.archive_targets = run.archives.clone();
        let taking: BTreeSet<&str> = plan.targets.iter().map(String::as_str).collect();
        let take = |list: &[Dated]| -> Vec<Dated> {
            list.iter()
                .filter(|dated| taking.contains(dated.id.as_str()))
                .cloned()
                .collect()
        };
        plan.older = take(&survey.older);
        plan.archives_removable = take(&survey.archives_removable);
        plan.kept_before_restore = take(&survey.kept_before_restore);
        plan.kept_before_restore_left = survey
            .kept_before_restore
            .iter()
            .filter(|dated| !taking.contains(dated.id.as_str()))
            .cloned()
            .collect();
        return Ok((plan, Vec::new()));
    }

    let ordinary = (|| -> Result<(Plan, Vec<Erase>), Refusal> {
        let Some(keep) = survey.newest.clone() else {
            return Err(Refusal::Nothing);
        };
        // The backup kept must be this journal's last recorded success, and
        // whole.
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
        let before_keep: Vec<Erase> = unsettled_erases(journal, &state, now, true)
            .into_iter()
            .filter(|erase| erase.at < keep_time)
            .collect();
        if !before_keep.iter().any(|erase| offerable(&survey, erase.at)) {
            return Err(Refusal::Waiting);
        }
        let (chosen, left): (Vec<Dated>, Vec<Dated>) = survey
            .kept_before_restore
            .iter()
            .cloned()
            .partition(|dated| include_kept && at_or_before(dated, kept_bound));
        let mut plan = base(Kind::Older, keep.clone(), keep.time.clone());
        plan.older = survey.older.clone();
        plan.archives_removable = survey.archives_removable.clone();
        plan.kept_before_restore = chosen;
        plan.kept_before_restore_left = left;
        plan.targets = plan
            .older
            .iter()
            .chain(&plan.archives_removable)
            .chain(&plan.kept_before_restore)
            .map(|dated| dated.id.clone())
            .collect();
        plan.archive_targets = plan
            .archives_removable
            .iter()
            .map(|dated| dated.id.clone())
            .collect();
        Ok((plan, before_keep))
    })();
    let refusal = match ordinary {
        Ok(decided) => return Ok(decided),
        Err(refusal) => refusal,
    };

    // Partly done's "remove them": the kept-from-restore backups a finished
    // removal left, up to what the owner was shown. It settles nothing new,
    // so it needs no new backup.
    if let Some(done) = &state.done
        && done.kept_before_restore_left > 0
        && include_kept
        && let Some(keep) = survey.newest.clone()
    {
        let (chosen, left): (Vec<Dated>, Vec<Dated>) = survey
            .kept_before_restore
            .iter()
            .cloned()
            .partition(|dated| at_or_before(dated, kept_bound));
        if !chosen.is_empty() {
            let mut plan = base(Kind::KeptOnly, keep, done.kept_newest_time.clone());
            plan.targets = chosen.iter().map(|dated| dated.id.clone()).collect();
            plan.kept_before_restore = chosen;
            plan.kept_before_restore_left = left;
            return Ok((plan, Vec::new()));
        }
    }
    Err(refusal)
}

/// The kept backup must hold nothing that has left the trash: no folder in
/// its trash that isn't in the journal's trash now, and none of an erased
/// recording's files where they were. The same look must see the journal's
/// own `config` folder, or it proves nothing.
fn check_newest(
    journal: &Path,
    repository: &Repository<'_>,
    plan: &Plan,
    erases: &[Erase],
) -> Result<(), Refusal> {
    let [base] = plan.keep_paths.as_slice() else {
        return Err(Refusal::CheckBlind);
    };
    let base = restic_tree_path(Path::new(base));
    let base = base.trim_end_matches('/');
    let control = format!("{base}/config");
    let trash = format!("{base}/{}", solstone_core_trash::copy_rel());
    let mut wanted = vec![control.clone(), trash.clone()];
    wanted.extend(
        erases
            .iter()
            .flat_map(|erase| erased_paths(journal, erase))
            .map(|rel| format!("{base}/{rel}")),
    );
    let found = repository
        .present(&plan.keep.id, &wanted)
        .map_err(Refusal::Failed)?;
    if !found.contains(&control) {
        return Err(Refusal::CheckBlind);
    }
    let trash_prefix = format!("{trash}/");
    for path in &found {
        if path == &control || path.starts_with(&format!("{control}/")) || path == &trash {
            continue;
        }
        if let Some(child) = path.strip_prefix(&trash_prefix) {
            let child = child.split('/').next().unwrap_or_default();
            // Still in the trash here: recoverable, so the owner has it.
            if !solstone_core_trash::left_since_copied(journal, child) {
                continue;
            }
            return Err(Refusal::NewestHoldsErased);
        }
        return Err(Refusal::NewestHoldsErased);
    }
    Ok(())
}

/// Remove what one press takes (see [`decide`]). `kept_bound` is the newest
/// kept-from-restore backup the owner was shown; none newer is taken.
/// `progress` hears (removed, total) after each batch.
///
/// The premise, as prune's: a repository holds one journal.
pub fn remove(
    journal: &Path,
    services: &BackupServices<'_>,
    include_kept: bool,
    kept_bound: Option<&str>,
    surface: Surface,
    progress: &dyn Fn(usize, usize),
) -> Result<Done, Refusal> {
    let _lock = hold_run_lock(journal)?;
    let repository = open(journal, services)?;
    let (plan, erases) = decide(journal, services, &repository, include_kept, kept_bound)?;
    let mut run = match current_state(journal).run {
        Some(run) => run,
        None => {
            if plan.targets.is_empty() {
                return Err(Refusal::Nothing);
            }
            if plan.kind == Kind::Older {
                check_newest(journal, &repository, &plan, &erases)?;
            }
            Run {
                started: services.clock.now_unix(),
                keep: plan.keep.id.clone(),
                keep_time: plan.boundary.clone(),
                include_kept: !plan.kept_before_restore.is_empty(),
                kept_only: plan.kind == Kind::KeptOnly,
                total: plan.targets.len(),
                targets: plan.targets.clone(),
                archives: plan.archive_targets.clone(),
                ..Run::default()
            }
        }
    };
    run.stopped = None;
    write_run(journal, services, &run);
    progress(run.removed, run.total);
    for batch in plan.targets.chunks(FORGET_BATCH) {
        let forgot = repository.forget(batch);
        // The listing decides what's gone: a target someone else already
        // forgot counts, and an exit code alone never stops a run.
        let left = match repository.snapshots() {
            Ok(after) => after
                .into_iter()
                .map(|facts| facts.id)
                .collect::<BTreeSet<_>>(),
            Err(listing) => {
                // A forget may have happened: the record says so, once.
                record_once(journal, &mut run, plan.operated, surface);
                return Err(stop(
                    journal,
                    services,
                    &mut run,
                    forgot.err().unwrap_or(listing),
                ));
            }
        };
        run.removed = run.targets.iter().filter(|id| !left.contains(*id)).count();
        if run.removed > 0 {
            record_once(journal, &mut run, plan.operated, surface);
        }
        if batch.iter().any(|id| left.contains(id)) || !left.contains(&run.keep) {
            let reason = forgot.err().unwrap_or_else(|| "residual".into());
            return Err(stop(journal, services, &mut run, reason));
        }
        write_run(journal, services, &run);
        progress(run.removed, run.total);
    }
    finish(
        journal,
        services,
        &repository,
        &mut run,
        plan.operated,
        surface,
    )
}

/// The deletion record's one entry for this operation.
fn record_once(journal: &Path, run: &mut Run, operated: bool, surface: Surface) {
    if run.recorded {
        return;
    }
    Removal::begin(journal, Action::OlderBackupsRemove, surface).record(Unit::Backup {
        storage: if operated { "operated" } else { "own" }.to_owned(),
    });
    run.recorded = true;
}

fn write_run(journal: &Path, services: &BackupServices<'_>, run: &Run) {
    let mut run = run.clone();
    run.updated = services.clock.now_unix();
    let _ = store(journal, |state| state.run = Some(run));
}

fn stop(journal: &Path, services: &BackupServices<'_>, run: &mut Run, reason: String) -> Refusal {
    run.stopped = Some(reason.clone());
    write_run(journal, services, run);
    Refusal::Stopped {
        reason,
        removed: run.removed,
        total: run.total,
    }
}

/// Write the result, claiming only what a fresh listing shows: nothing the
/// run chose is left, and no regular backup older than its boundary. A run
/// that took regular backups also needs no archive of only erased media
/// left. Anything else is a stop, never a "done".
fn finish(
    journal: &Path,
    services: &BackupServices<'_>,
    repository: &Repository<'_>,
    run: &mut Run,
    operated: bool,
    surface: Surface,
) -> Result<Done, Refusal> {
    record_once(journal, run, operated, surface);
    let now = services.clock.now_unix();
    let snapshots = match repository.snapshots() {
        Ok(snapshots) => snapshots,
        Err(reason) => return Err(stop(journal, services, run, reason)),
    };
    let survey = listing(journal, repository, snapshots.clone(), now);
    let present: BTreeSet<&str> = snapshots.iter().map(|facts| facts.id.as_str()).collect();
    let boundary = DateTime::parse_from_rfc3339(&run.keep_time).ok();
    let older_left = survey.older.iter().any(|dated| {
        dated.id != run.keep
            && match (dated.instant(), boundary) {
                (Some(time), Some(boundary)) => time < boundary,
                _ => true,
            }
    });
    let chosen_left = run.targets.iter().any(|id| present.contains(id.as_str()));
    let archives_left = !run.kept_only && !survey.archives_removable.is_empty();
    if chosen_left || older_left || archives_left || !present.contains(run.keep.as_str()) {
        return Err(stop(journal, services, run, "residual".into()));
    }
    run.removed = run.total;
    let done = Done {
        at: now,
        kept_newest_time: run.keep_time.clone(),
        removed: run.total,
        archives_removed: run.archives.len(),
        archives_mixed: survey.archives_mixed.len(),
        kept_before_restore_left: survey.kept_before_restore.len(),
        kept_before_restore_to: survey
            .kept_before_restore
            .last()
            .map(|dated| dated.time.clone()),
        storage: survey.storage.clone(),
    };
    let settle = (!run.kept_only).then(|| run.keep_time.clone());
    store(journal, |state| {
        state.run = None;
        state.done = Some(done.clone());
        if let Some(settle) = settle {
            state.settled_through = Some(settle);
        }
        state.survey = Some(survey);
    })
    .map_err(Refusal::Failed)?;
    Ok(done)
}

#[cfg(test)]
mod tests;
