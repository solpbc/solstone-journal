// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The only place in this tree that removes the owner's media.
//!
//! Every other module is barred from naming a removal primitive, and
//! `tests/architecture.rs` makes that a test failure rather than a convention.
//! Within this module every filesystem call goes through the journal-I/O
//! boundary's contained primitives, so containment is resolved immediately before
//! each syscall rather than once for a batch.
//!
//! # What this module does
//!
//! The raw-release verb and the whole-segment verb — with its staging rename,
//! its tombstone, its lock and its crash-recovery pass — both live here. A
//! caller that names a source (the observer-web location erase) resolves that
//! name to a set of segments and hands the set to [`remove_segments`]; this
//! module still resolves nothing.

#![allow(
    clippy::disallowed_methods,
    reason = "this is the door: the crate-wide ban exists so that only this module reaches a removal primitive"
)]

use std::path::Path;

use solstone_core_journal_io::atomic::atomic_replace;
use solstone_core_journal_io::entry::{Removed, remove_file, rename_within, sync_dir};
use solstone_core_journal_io::paths::{
    DirEntryKind, contained_path, list_dir_entries, path_lexists,
};
use solstone_core_journal_io::removal::remove_dir_all;
use solstone_core_journal_io::{
    AtomicWriteOptions, HealthMarkerKind, LockOptions, bump_stream_marker, health_marker_path,
    hold_lock, write_bytes_exclusive,
};

use crate::eligibility::{Evidence, ProvenRaw};
use crate::notify::{IndexNotify, NotifyError, PruneCounts};
use crate::receipt::{
    NotRemoved, Outcome, PostCommitFailure, RemovedPath, RunHalt, Target, TargetOutcome,
};
use crate::staging::staged_name;
use crate::tombstone::{RemovalReason, TOMBSTONE_NAME, TombstoneBody, tombstone_bytes};

/// How many files a run released on pre-record evidence rather than on a record.
///
/// Reported so a receipt can say which files rested on the weaker justification.
/// ⚠ The read-old rule requires honouring that evidence; it does not permit
/// honouring it silently.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EvidenceTally {
    pub on_record: usize,
    pub on_legacy_rows: usize,
}

/// Release proven raw originals, leaving every derived output in place.
///
/// ⛔ Takes proofs, never paths. A caller cannot ask this to remove a sidecar, a
/// derived output or journal-authored metadata, because it cannot construct the
/// value that would name one — the guarantee is that the request is
/// unrepresentable, not that a check refuses it.
///
/// ⛔ Returns [`Outcome`] and never `Result`: `?` is illegal in a function
/// returning it, so this cannot lose its own report by propagating an error out
/// from under the caller. That is the defect the reference implementation has —
/// an owner told an irreversible removal failed after it completed.
///
/// One [`TargetOutcome`] per segment, in first-seen order. A file that fails is a
/// row on its segment; ⛔ it never aborts the run and never becomes a run-level
/// halt, because one file cannot be allowed to discard a sibling's result.
pub fn release_raw(journal: &Path, proven: &[ProvenRaw]) -> (Outcome, EvidenceTally) {
    let mut outcome = Outcome {
        targets: Vec::new(),
        halted: None,
    };
    let mut tally = EvidenceTally::default();

    for item in proven {
        let target = Target {
            day: item.day().to_owned(),
            stream: item.stream().to_owned(),
            dir: item.dir().to_owned(),
        };
        // Group by segment: the row an owner reads is per-segment, and the proof
        // carries its own segment so no extra parameter is needed to find it.
        let index = match outcome
            .targets
            .iter()
            .position(|existing| existing.target == target)
        {
            Some(index) => index,
            None => {
                outcome.targets.push(TargetOutcome {
                    target,
                    removed: Vec::new(),
                    not_removed: Vec::new(),
                    post_commit_failure: None,
                });
                outcome.targets.len().saturating_sub(1)
            }
        };
        let Some(row) = outcome.targets.get_mut(index) else {
            continue;
        };

        let rel = item.rel();
        match remove_file(journal, &rel) {
            Ok(Removed::Unlinked) => {
                // The directory entry is gone; make that durable before anything
                // reports it. `fsync` on a file does not persist the entry naming
                // it, and a record saying a removal happened at a given moment
                // has to be true at that moment.
                if let Err(error) = sync_dir(journal, &item.segment_rel()) {
                    row.not_removed.push(NotRemoved {
                        entry: rel.clone(),
                        reason: format!(
                            "removed, but the change could not be flushed to disk: {error}"
                        ),
                        staged: None,
                    });
                    continue;
                }
                row.removed.push(RemovedPath::confirmed(rel));
                dirty_removed_day(journal, row);
                match item.evidence() {
                    Evidence::Record => tally.on_record = tally.on_record.saturating_add(1),
                    Evidence::LegacyRows => {
                        tally.on_legacy_rows = tally.on_legacy_rows.saturating_add(1);
                    }
                }
            }
            // ⛔ Not a removal this run performed, so no RemovedPath is minted.
            // The receipt describes THIS run; conflating the two would report a
            // deletion that did not happen here.
            Ok(Removed::AlreadyAbsent) => row.not_removed.push(NotRemoved {
                entry: rel,
                reason: "this was already gone before the run started".to_owned(),
                staged: None,
            }),
            Err(error) => row.not_removed.push(NotRemoved {
                entry: rel,
                reason: owner_reason(&error),
                staged: None,
            }),
        }
    }

    (outcome, tally)
}

/// Turn a path error into something a person can act on.
///
/// ⛔ Never the raw error's `Display`: it carries absolute paths and errno prose,
/// and it is the text an owner would be shown about their own data.
fn owner_reason(error: &solstone_core_journal_io::errors::PathError) -> String {
    use solstone_core_journal_io::errors::PathError;
    match error {
        PathError::Escape(_) => "this is not inside your journal, so it was left alone".to_owned(),
        PathError::InvalidRelativePath { .. } => {
            "the location does not name a file inside your journal".to_owned()
        }
        PathError::Io { source, .. } => match source.kind() {
            std::io::ErrorKind::PermissionDenied => {
                "the file could not be read or removed, which usually means a permission problem"
                    .to_owned()
            }
            std::io::ErrorKind::IsADirectory => {
                "there is a folder where a recording was expected".to_owned()
            }
            _ => "the file could not be removed; the journal was left unchanged".to_owned(),
        },
    }
}

/// Remove whole segments, leaving each holding only its tombstone.
///
/// ⛔ **Not all-or-nothing.** An owner deleting forty segments must not lose the
/// thirty-nine that succeeded because the fortieth was unreadable, so each target
/// is independent and a failure is a row on that target. `halted` means the run
/// stopped before reaching every target, ⛔ never that one target failed.
///
/// ⛔ **This resolves nothing.** It receives targets the owner chose. There is no
/// query anywhere in this crate from a source name to a set of segments. The
/// observer-web source-delete surface performs that selection and hands the set
/// here.
///
/// Duplicate targets are collapsed at entry: the second occurrence would meet the
/// already-tombstoned guard and be reported as refused, telling an owner a segment
/// they asked to delete was declined when in fact it was removed.
pub fn remove_segments(
    journal: &Path,
    targets: &[Target],
    deleted_at: &str,
    reason: RemovalReason,
    cid: &str,
) -> Outcome {
    let mut outcome = Outcome {
        targets: Vec::new(),
        halted: None,
    };
    let mut seen: Vec<&Target> = Vec::new();
    for target in targets {
        if seen.contains(&target) {
            continue;
        }
        seen.push(target);
        let (mut row, mutated) = remove_one(journal, target, deleted_at, reason, cid);
        if mutated {
            dirty_removed_day(journal, &mut row);
        }
        outcome.targets.push(row);
    }
    outcome
}

/// Tell the index about everything a run removed — **after** the run.
///
/// Separate from the verbs on purpose. A verb's job is to remove and report; this
/// is the step that can only be correct if it happens second, and keeping it a
/// distinct call makes the ordering visible at every call site rather than buried
/// in a verb.
///
/// ⛔ Takes the outcome, so the paths it names are exactly the ones a verb
/// confirmed gone. A failure here is reported and does not undo anything: the
/// removal already happened, the index is a rebuildable cache, and the next scan
/// corrects it.
pub fn notify_index(
    index: &dyn IndexNotify,
    outcome: &Outcome,
) -> Result<PruneCounts, NotifyError> {
    let removed: Vec<RemovedPath> = outcome.removed_paths().cloned().collect();
    if removed.is_empty() {
        return Ok(PruneCounts::default());
    }
    index.paths_removed(&removed)
}

/// Remove operational log entries a plan judged prunable.
///
/// ⛔ Takes [`LogTarget`](crate::logs::LogTarget)s, never paths. Only the log planner
/// can mint one and only from a declared class root — which matters more here than for
/// media, because one class prunes `chronicle/<day>/health/`, a directory sitting
/// beside the owner's streams. The guarantee that this cannot reach a recording is
/// that it cannot be *asked* to.
///
/// ⛔ Returns [`Outcome`] and never `Result`, so it cannot lose its own report.
///
/// One [`TargetOutcome`] per class, in table order. A failed entry is a row on its
/// class and never halts the run: one unreadable log must not stop the rest.
pub fn remove_logs(journal: &Path, targets: &[crate::logs::LogTarget]) -> Outcome {
    use crate::logs::EntryKind;

    let mut by_class: Vec<TargetOutcome> = Vec::new();
    for target in targets {
        let class = target.class();
        if !by_class.iter().any(|done| done.target.dir == class) {
            by_class.push(TargetOutcome {
                // ⚠ A log class is not a segment. `Target` is reused for its shape;
                // the day and stream are empty because a class spans every day.
                target: Target {
                    day: String::new(),
                    stream: String::new(),
                    dir: class.to_owned(),
                },
                removed: Vec::new(),
                not_removed: Vec::new(),
                post_commit_failure: None,
            });
        }
        let Some(slot) = by_class.iter_mut().find(|done| done.target.dir == class) else {
            continue;
        };

        let outcome = match target.kind() {
            EntryKind::File => remove_file(journal, target.rel()),
            EntryKind::Directory => {
                remove_dir_all(journal, target.rel()).map(|()| Removed::Unlinked)
            }
        };
        match outcome {
            Ok(Removed::Unlinked | Removed::AlreadyAbsent) => {
                slot.removed
                    .push(RemovedPath::confirmed(target.rel().to_owned()));
            }
            Err(error) => slot.not_removed.push(NotRemoved {
                entry: target.rel().to_owned(),
                reason: format!("the log entry could not be removed: {error}"),
                staged: None,
            }),
        }
    }
    Outcome {
        targets: by_class,
        halted: None,
    }
}

/// Perform a planned log compaction.
///
/// ⛔ Writes the bytes the plan carries, published atomically with the file's existing
/// mode, so a reader never sees a half-rewritten log and a crash leaves the original.
/// The mode is read from the file being replaced rather than defaulted: a log the
/// operator tightened must not be widened by being pruned.
pub fn compact_log(journal: &Path, planned: &crate::logs::Compaction) -> Outcome {
    let target = Target {
        day: String::new(),
        stream: String::new(),
        dir: planned.name().to_owned(),
    };
    let path = match contained_path(journal, planned.rel()) {
        Ok(path) => path,
        Err(error) => {
            return Outcome {
                targets: vec![refused(
                    &target,
                    planned.rel().to_owned(),
                    format!("the log is not inside the journal: {error}"),
                )],
                halted: None,
            };
        }
    };
    let mode = std::fs::metadata(&path)
        .ok()
        .map(|meta| std::os::unix::fs::PermissionsExt::mode(&meta.permissions()) & 0o777);
    match atomic_replace(&path, planned.contents(), AtomicWriteOptions { mode }) {
        Ok(()) => Outcome {
            targets: vec![TargetOutcome {
                target,
                // ⚠ A compaction removed lines, not a path. The receipt names the
                // file whose old end is gone.
                removed: vec![RemovedPath::confirmed(planned.rel().to_owned())],
                not_removed: Vec::new(),
                post_commit_failure: None,
            }],
            halted: None,
        },
        Err(error) => Outcome {
            targets: vec![refused(
                &target,
                planned.rel().to_owned(),
                format!("the log could not be rewritten: {error}"),
            )],
            halted: None,
        },
    }
}

/// The parent directory of a segment, journal-relative.
fn parent_rel(target: &Target) -> String {
    crate::layout::stream_rel(&target.day, &target.stream)
}

fn segment_rel(target: &Target) -> String {
    crate::layout::segment_rel(&target.day, &target.stream, &target.dir)
}

fn refused(target: &Target, entry: String, reason: String) -> TargetOutcome {
    TargetOutcome {
        target: target.clone(),
        removed: Vec::new(),
        not_removed: vec![NotRemoved {
            entry,
            reason,
            staged: None,
        }],
        post_commit_failure: None,
    }
}

fn refused_staged(target: &Target, staged: String, reason: String) -> TargetOutcome {
    TargetOutcome {
        target: target.clone(),
        removed: Vec::new(),
        not_removed: vec![NotRemoved {
            entry: staged.clone(),
            reason,
            staged: Some(staged),
        }],
        post_commit_failure: None,
    }
}

fn remove_one(
    journal: &Path,
    target: &Target,
    deleted_at: &str,
    reason: RemovalReason,
    cid: &str,
) -> (TargetOutcome, bool) {
    let live = segment_rel(target);
    let staged = format!("{}/{}", parent_rel(target), staged_name(&target.dir));

    // ⚠ A pre-lock existence filter, before step 0. Taking the lock creates the
    // sidecar's parent directories, so locking a target whose day or stream
    // directory does not exist would leave a phantom day in the chronicle behind a
    // request that was then REFUSED. The authoritative checks still run under the
    // lock below; this only avoids the side effect.
    match path_lexists(&journal.join(&live)) {
        Ok(true) => {}
        Ok(false) => {
            return (
                refused(
                    target,
                    live,
                    "there is no such segment in your journal".to_owned(),
                ),
                false,
            );
        }
        Err(error) => return (refused(target, live, owner_reason(&error)), false),
    }

    // Step 0: the lock, keyed on the segment's DIRECTORY NAME.
    //
    // ⛔ Both the door and the recovery pass must derive it from the same string.
    // The lock helper builds its sidecar from the path it is handed, so a door
    // keyed on the live name and a recovery pass keyed on the staged name would
    // take different sidecars and exclude nothing -- which is the one race this
    // lock exists to prevent.
    let _lock = match hold_lock(journal.join(&live), LockOptions::default()) {
        Ok(lock) => lock,
        Err(error) => {
            return (
                refused(
                    target,
                    live,
                    format!("another process is working on this segment ({error})"),
                ),
                false,
            );
        }
    };

    // Step 1: the guard, UNDER the lock.
    //
    // ⛔ Checking before locking lets two callers both pass and the second act on a
    // verdict computed before the first ran -- by which time the segment is
    // tombstone-only, so it would be staged again and step 4 would delete the
    // owner's deletion record.
    match path_lexists(&journal.join(&live).join(TOMBSTONE_NAME)) {
        Ok(true) => {
            return (
                refused(
                    target,
                    live,
                    "this segment has already been removed".to_owned(),
                ),
                false,
            );
        }
        Ok(false) => {}
        Err(error) => return (refused(target, live, owner_reason(&error)), false),
    }
    if !matches!(path_lexists(&journal.join(&live)), Ok(true)) {
        return (
            refused(
                target,
                live,
                "there is no such segment in your journal".to_owned(),
            ),
            false,
        );
    }
    // ⛔ Refuse rather than clobber: a rename onto an existing empty directory
    // succeeds and destroys it.
    if !matches!(path_lexists(&journal.join(&staged)), Ok(false)) {
        return (
            refused_staged(
                target,
                staged,
                "a previous removal of this segment did not finish; \
                 it needs looking at before this can be retried"
                    .to_owned(),
            ),
            false,
        );
    }

    // Step 2: stage, then make the move durable.
    if let Err(error) = rename_within(journal, &live, &staged) {
        return (refused(target, live, owner_reason(&error)), false);
    }
    if let Err(error) = sync_dir(journal, &parent_rel(target)) {
        return (refused_staged(target, staged, owner_reason(&error)), true);
    }

    (
        finish_staged(journal, target, &staged, deleted_at, reason, cid),
        true,
    )
}

fn dirty_removed_day(journal: &Path, row: &mut TargetOutcome) {
    if let Err(error) = bump_stream_marker(journal, &row.target.day) {
        let path = health_marker_path(journal, &row.target.day, HealthMarkerKind::Stream);
        let relative = path
            .strip_prefix(journal)
            .unwrap_or(&path)
            .display()
            .to_string();
        row.post_commit_failure = Some(PostCommitFailure {
            entry: relative,
            reason: format!(
                "the retention mutation completed, but the day could not be queued for follow-up processing: {error}"
            ),
        });
    }
}

/// Steps 3 to 6, from a staged directory. Shared with the recovery pass.
fn finish_staged(
    journal: &Path,
    target: &Target,
    staged: &str,
    deleted_at: &str,
    reason: RemovalReason,
    cid: &str,
) -> TargetOutcome {
    let live = segment_rel(target);

    // Step 3: the manifest and the tombstone, written INTO the staged directory.
    //
    // ⚠ The manifest has to be captured here, before anything is removed: the
    // tombstone is the only artifact that survives, so it is the only place a later
    // pass can learn what went.
    let entries = match list_dir_entries(&journal.join(staged)) {
        Ok(entries) => entries,
        Err(error) => return refused_staged(target, staged.to_owned(), owner_reason(&error)),
    };
    let mut manifest = Vec::new();
    let mut to_remove = Vec::new();
    for entry in &entries {
        let name = entry.name.to_string_lossy().into_owned();
        if name == TOMBSTONE_NAME {
            continue;
        }
        manifest.push(format!("{live}/{name}"));
        to_remove.push((name, entry.kind));
    }
    manifest.sort();

    let tombstone_path = journal.join(staged).join(TOMBSTONE_NAME);
    if !matches!(path_lexists(&tombstone_path), Ok(true)) {
        let body = TombstoneBody {
            deleted_at: deleted_at.to_owned(),
            cid: cid.to_owned(),
            reason,
            manifest: manifest.clone(),
        };
        let bytes = match tombstone_bytes(&body) {
            Ok(bytes) => bytes,
            Err(_) => {
                return refused_staged(
                    target,
                    staged.to_owned(),
                    "the record of this removal could not be prepared".to_owned(),
                );
            }
        };
        if write_bytes_exclusive(&tombstone_path, &bytes, AtomicWriteOptions::default()).is_err() {
            return refused_staged(
                target,
                staged.to_owned(),
                "the record of this removal could not be written".to_owned(),
            );
        }
    }

    // Step 4: empty the staged directory, keeping the tombstone.
    //
    // ⚠ Directories and files need different primitives; a file-only removal would
    // fail on a `talents/` subdirectory and never reach the rest.
    let mut failures = Vec::new();
    for (name, kind) in &to_remove {
        let rel = format!("{staged}/{name}");
        let result = match kind {
            DirEntryKind::Directory => remove_dir_all(journal, &rel).map(|()| Removed::Unlinked),
            _ => remove_file(journal, &rel),
        };
        if let Err(error) = result {
            failures.push(NotRemoved {
                entry: format!("{live}/{name}"),
                reason: owner_reason(&error),
                staged: None,
            });
        }
    }
    if !failures.is_empty() {
        // ⛔ Leave it staged and say where it went. Escalate rather than retry: the
        // segment is not enumerable under its real name, so the reason is the only
        // way anyone learns it is there.
        failures.push(NotRemoved {
            entry: staged.to_owned(),
            reason: "this segment is part-way through removal and is set aside; \
                     it is not listed with your other segments until it finishes"
                .to_owned(),
            staged: Some(staged.to_owned()),
        });
        return TargetOutcome {
            target: target.clone(),
            removed: Vec::new(),
            not_removed: failures,
            post_commit_failure: None,
        };
    }
    if let Err(error) = sync_dir(journal, staged) {
        return refused_staged(target, staged.to_owned(), owner_reason(&error));
    }

    // Step 5: restore, refusing a name something else has taken.
    if !matches!(path_lexists(&journal.join(&live)), Ok(false)) {
        return refused_staged(
            target,
            staged.to_owned(),
            "something new was written where this segment was, so it has been \
             left set aside rather than overwritten"
                .to_owned(),
        );
    }
    if let Err(error) = rename_within(journal, staged, &live) {
        return refused_staged(target, staged.to_owned(), owner_reason(&error));
    }
    if let Err(error) = sync_dir(journal, &parent_rel(target)) {
        return refused(target, live, owner_reason(&error));
    }

    // Step 6: mint, against the RESTORED path.
    //
    // ⛔ Not earlier. Staging makes every path under the live name absent the
    // instant the rename lands, because the directory moved rather than because
    // anything was removed -- so minting then would claim files nothing touched.
    let mut removed = Vec::new();
    let mut unverified = Vec::new();
    for rel in &manifest {
        match path_lexists(&journal.join(rel)) {
            Ok(false) => removed.push(RemovedPath::confirmed(rel.clone())),
            _ => unverified.push(NotRemoved {
                entry: rel.clone(),
                reason: "this could not be confirmed gone, so it is not being \
                         reported as removed"
                    .to_owned(),
                staged: None,
            }),
        }
    }
    TargetOutcome {
        target: target.clone(),
        removed,
        not_removed: unverified,
        post_commit_failure: None,
    }
}

/// Finish removals a previous run left staged.
///
/// 🔴 Enumerates staged directories and **never inspects a live segment.** That is
/// the whole contract: a live segment carries no evidence that a removal was ever
/// requested, so a pass that acted on one would remove segments nobody asked about
/// — including any segment where a *raw release* was merely interrupted, which by
/// definition must keep its derived output.
pub fn recover(journal: &Path, deleted_at: &str, reason: RemovalReason, cid: &str) -> Outcome {
    let mut outcome = Outcome {
        targets: Vec::new(),
        halted: None,
    };
    let chronicle = journal.join("chronicle");
    let days = match list_dir_entries(&chronicle) {
        Ok(days) => days,
        Err(error) => {
            outcome.halted = Some(RunHalt {
                reason: owner_reason(&error),
            });
            return outcome;
        }
    };
    for day in days.iter().filter(|d| d.kind == DirEntryKind::Directory) {
        let day_name = day.name.to_string_lossy().into_owned();
        let Ok(streams) = list_dir_entries(&day.path) else {
            continue;
        };
        for stream in streams.iter().filter(|s| s.kind == DirEntryKind::Directory) {
            let stream_name = stream.name.to_string_lossy().into_owned();
            let Ok(entries) = list_dir_entries(&stream.path) else {
                continue;
            };
            for entry in entries.iter().filter(|e| e.kind == DirEntryKind::Directory) {
                let staged_dir = entry.name.to_string_lossy().into_owned();
                // ⛔ A name alone is weak provenance, so the recovered original
                // must be a name this crate could have staged.
                let Some(original) = crate::staging::original_name(&staged_dir) else {
                    continue;
                };
                let target = Target {
                    day: day_name.clone(),
                    stream: stream_name.clone(),
                    dir: original.to_owned(),
                };
                let live = segment_rel(&target);
                let staged = format!("{}/{}", parent_rel(&target), staged_dir);
                // Same key as the door's, derived from the LIVE name.
                let Ok(_lock) = hold_lock(journal.join(&live), LockOptions::default()) else {
                    continue;
                };
                let mut row = finish_staged(journal, &target, &staged, deleted_at, reason, cid);
                dirty_removed_day(journal, &mut row);
                outcome.targets.push(row);
            }
        }
    }
    outcome
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    reason = "test code; the crate-level denials exist to constrain the verbs"
)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::*;
    use crate::content::{ContentName, MediaClassifier};

    struct Bed {
        root: PathBuf,
    }

    impl Bed {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "retention-door-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            fs::create_dir_all(&root).unwrap();
            Self { root }
        }

        fn segment(&self, day: &str, stream: &str, dir: &str) -> PathBuf {
            let path = self.root.join(format!("chronicle/{day}/{stream}/{dir}"));
            fs::create_dir_all(&path).unwrap();
            path
        }
    }

    impl Drop for Bed {
        fn drop(&mut self) {
            let _ignored = fs::remove_dir_all(&self.root);
        }
    }

    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    fn media_only(name: &ContentName) -> bool {
        matches!(name.extension().as_deref(), Some("flac" | "mp4" | "wav"))
    }

    fn proof(day: &str, stream: &str, dir: &str, name: &str, size: u64) -> ProvenRaw {
        let classifier: &dyn MediaClassifier = &media_only;
        ProvenRaw::for_test(classifier, day, stream, dir, name, size).unwrap()
    }

    #[test]
    fn releases_only_the_proven_files_and_leaves_everything_else() {
        let bed = Bed::new();
        let segment = bed.segment("20260805", "field.audio", "070000_17");
        for name in ["a.flac", "a.jsonl", "stream.json"] {
            fs::write(segment.join(name), b"bytes").unwrap();
        }
        fs::create_dir_all(segment.join("talents")).unwrap();
        fs::write(segment.join("talents/sense.json"), b"{}").unwrap();

        let (outcome, tally) = release_raw(
            &bed.root,
            &[proof("20260805", "field.audio", "070000_17", "a.flac", 5)],
        );

        assert!(outcome.halted.is_none());
        assert_eq!(outcome.targets.len(), 1);
        assert_eq!(outcome.targets[0].removed.len(), 1);
        assert!(outcome.targets[0].not_removed.is_empty());
        assert_eq!(tally.on_record, 1);

        assert!(!segment.join("a.flac").exists(), "the proven raw is gone");
        for survivor in ["a.jsonl", "stream.json", "talents/sense.json"] {
            assert!(
                segment.join(survivor).exists(),
                "{survivor} must survive a raw release"
            );
        }
        assert!(
            !segment.join("tombstone.json").exists(),
            "a released segment is not a removed segment"
        );
    }

    /// Two files in the SAME segment failing for different reasons are both
    /// reported, and the run does not abort.
    ///
    /// This is the shape a single failure slot could not represent, one level
    /// below the run: the verb groups N files into one row.
    #[test]
    fn two_files_in_one_segment_failing_differently_are_both_reported() {
        let bed = Bed::new();
        let segment = bed.segment("20260805", "field.audio", "070000_17");
        fs::write(segment.join("kept.flac"), b"gone").unwrap();
        // A directory where a recording was expected: unlink refuses it.
        fs::create_dir_all(segment.join("dir.flac")).unwrap();
        // And one that is simply not there.

        let (outcome, _) = release_raw(
            &bed.root,
            &[
                proof("20260805", "field.audio", "070000_17", "kept.flac", 4),
                proof("20260805", "field.audio", "070000_17", "dir.flac", 4),
                proof("20260805", "field.audio", "070000_17", "absent.flac", 4),
            ],
        );

        assert_eq!(outcome.targets.len(), 1, "one segment, one row");
        let row = &outcome.targets[0];
        assert_eq!(row.removed.len(), 1);
        assert_eq!(row.not_removed.len(), 2, "both failures are reported");
        assert_ne!(
            row.not_removed[0].reason, row.not_removed[1].reason,
            "the two reasons are distinct"
        );
        assert!(outcome.halted.is_none(), "a file failure is not a run halt");
    }

    /// An absent file is disclosed, and mints no removal.
    #[test]
    fn an_already_absent_file_mints_no_removed_path() {
        let bed = Bed::new();
        bed.segment("20260805", "field.audio", "070000_17");
        let (outcome, tally) = release_raw(
            &bed.root,
            &[proof(
                "20260805",
                "field.audio",
                "070000_17",
                "gone.flac",
                4,
            )],
        );
        assert!(outcome.targets[0].removed.is_empty());
        assert_eq!(outcome.targets[0].not_removed.len(), 1);
        assert_eq!(tally.on_record, 0);
    }

    #[test]
    fn files_from_two_segments_produce_two_rows_in_first_seen_order() {
        let bed = Bed::new();
        for dir in ["070000_17", "070100_17"] {
            let segment = bed.segment("20260805", "field.audio", dir);
            fs::write(segment.join("a.flac"), b"x").unwrap();
        }
        let (outcome, _) = release_raw(
            &bed.root,
            &[
                proof("20260805", "field.audio", "070100_17", "a.flac", 1),
                proof("20260805", "field.audio", "070000_17", "a.flac", 1),
            ],
        );
        assert_eq!(outcome.targets.len(), 2);
        assert_eq!(outcome.targets[0].target.dir, "070100_17");
        assert_eq!(outcome.targets[1].target.dir, "070000_17");
    }

    #[test]
    fn a_second_run_over_the_same_set_removes_nothing() {
        let bed = Bed::new();
        let segment = bed.segment("20260805", "field.audio", "070000_17");
        fs::write(segment.join("a.flac"), b"x").unwrap();
        let set = [proof("20260805", "field.audio", "070000_17", "a.flac", 1)];

        let (first, _) = release_raw(&bed.root, &set);
        assert_eq!(first.targets[0].removed.len(), 1);

        let (second, tally) = release_raw(&bed.root, &set);
        assert!(second.targets[0].removed.is_empty(), "idempotent");
        assert_eq!(second.targets[0].not_removed.len(), 1);
        assert_eq!(tally.on_record, 0);
    }

    #[test]
    fn every_minted_path_is_absent_when_the_verb_returns() {
        let bed = Bed::new();
        let segment = bed.segment("20260805", "field.audio", "070000_17");
        for name in ["a.flac", "b.wav"] {
            fs::write(segment.join(name), b"x").unwrap();
        }
        let (outcome, _) = release_raw(
            &bed.root,
            &[
                proof("20260805", "field.audio", "070000_17", "a.flac", 1),
                proof("20260805", "field.audio", "070000_17", "b.wav", 1),
            ],
        );
        let minted: Vec<&str> = outcome.removed_paths().map(RemovedPath::as_str).collect();
        assert_eq!(minted.len(), 2);
        for rel in minted {
            assert!(!rel.starts_with('/'), "{rel} must be journal-relative");
            assert!(!rel.contains(".."), "{rel} must not contain ..");
            assert!(
                !bed.root.join(rel).exists(),
                "{rel} was minted but still exists"
            );
        }
    }

    /// A reason must be readable by a person, not an errno dump.
    #[test]
    fn a_failure_reason_is_owner_readable() {
        let bed = Bed::new();
        let segment = bed.segment("20260805", "field.audio", "070000_17");
        fs::create_dir_all(segment.join("dir.flac")).unwrap();
        let (outcome, _) = release_raw(
            &bed.root,
            &[proof("20260805", "field.audio", "070000_17", "dir.flac", 1)],
        );
        let reason = &outcome.targets[0].not_removed[0].reason;
        assert!(!reason.contains(&bed.root.display().to_string()));
        assert!(
            [
                "there is a folder where a recording was expected",
                "the file could not be read or removed, which usually means a permission problem",
            ]
            .contains(&reason.as_str()),
            "got {reason}"
        );
    }

    // ---- remove_segments -------------------------------------------------

    fn target(day: &str, stream: &str, dir: &str) -> Target {
        Target {
            day: day.to_owned(),
            stream: stream.to_owned(),
            dir: dir.to_owned(),
        }
    }

    fn populated(bed: &Bed, dir: &str) -> PathBuf {
        populated_on(bed, "20260805", dir)
    }

    fn populated_on(bed: &Bed, day: &str, dir: &str) -> PathBuf {
        let segment = bed.segment(day, "field.audio", dir);
        fs::write(segment.join("audio.flac"), b"raw").unwrap();
        fs::write(segment.join("audio.jsonl"), b"{}").unwrap();
        fs::write(segment.join("stream.json"), b"{}").unwrap();
        fs::create_dir_all(segment.join("talents")).unwrap();
        fs::write(segment.join("talents/sense.json"), b"{}").unwrap();
        segment
    }

    fn remove(bed: &Bed, targets: &[Target]) -> Outcome {
        remove_segments(
            &bed.root,
            targets,
            "2026-08-05T21:00:00Z",
            RemovalReason::OwnerSegmentDelete,
            "sha256:abc",
        )
    }

    #[test]
    fn a_removed_segment_holds_only_its_tombstone() {
        let bed = Bed::new();
        let segment = populated(&bed, "070000_17");
        let outcome = remove(&bed, &[target("20260805", "field.audio", "070000_17")]);

        assert!(outcome.halted.is_none());
        assert!(outcome.targets[0].not_removed.is_empty());
        assert_eq!(outcome.targets[0].removed.len(), 4, "four entries removed");

        let names: Vec<String> = fs::read_dir(&segment)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec![TOMBSTONE_NAME.to_owned()]);
    }

    #[test]
    fn segment_removal_dirties_each_mutated_day_and_no_untouched_day() {
        let bed = Bed::new();
        populated_on(&bed, "20260805", "070000_17");
        populated_on(&bed, "20260806", "080000_17");
        populated_on(&bed, "20260807", "090000_17");

        let outcome = remove(
            &bed,
            &[
                target("20260805", "field.audio", "070000_17"),
                target("20260806", "field.audio", "080000_17"),
            ],
        );

        assert!(!outcome.has_failures());
        for day in ["20260805", "20260806"] {
            let value: serde_json::Value = serde_json::from_slice(
                &fs::read(
                    bed.root
                        .join("chronicle")
                        .join(day)
                        .join("health/stream.updated"),
                )
                .unwrap(),
            )
            .unwrap();
            assert_eq!(value["generation"], 1, "{day}");
        }
        assert!(
            !bed.root
                .join("chronicle/20260807/health/stream.updated")
                .exists()
        );
    }

    #[test]
    fn segment_marker_failure_is_terminal_without_claiming_the_removal_rolled_back() {
        let bed = Bed::new();
        let segment = populated(&bed, "070000_17");
        let marker = bed.root.join("chronicle/20260805/health/stream.updated");
        fs::create_dir_all(&marker).unwrap();

        let outcome = remove(&bed, &[target("20260805", "field.audio", "070000_17")]);

        assert!(outcome.has_failures());
        let row = &outcome.targets[0];
        assert!(row.not_removed.is_empty());
        assert_eq!(row.removed.len(), 4);
        let failure = row.post_commit_failure.as_ref().unwrap();
        assert_eq!(failure.entry, "chronicle/20260805/health/stream.updated");
        assert!(failure.reason.contains("retention mutation completed"));
        assert_eq!(listing(&segment), vec![TOMBSTONE_NAME]);
    }

    /// The tombstone names every path that went.
    #[test]
    fn the_tombstone_carries_the_manifest() {
        let bed = Bed::new();
        let segment = populated(&bed, "070000_17");
        let setup = remove(&bed, &[target("20260805", "field.audio", "070000_17")]);
        assert!(
            setup.targets[0].not_removed.is_empty(),
            "setup removal failed"
        );
        let json: serde_json::Value =
            serde_json::from_slice(&fs::read(segment.join(TOMBSTONE_NAME)).unwrap()).unwrap();
        let manifest: Vec<&str> = json["manifest"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(manifest.len(), 4);
        assert!(manifest.iter().any(|p| p.ends_with("talents")));
        assert!(
            manifest
                .iter()
                .all(|p| p.starts_with("chronicle/20260805/"))
        );
    }

    /// Two targets, the second missing: both reported, the first still removed.
    #[test]
    fn one_failing_target_does_not_abort_the_run() {
        let bed = Bed::new();
        populated(&bed, "070000_17");
        bed.segment("20260805", "field.audio", "070100_17");
        let outcome = remove(
            &bed,
            &[
                target("20260805", "field.audio", "070000_17"),
                target("20260805", "field.audio", "nosuch_99"),
                target("20260805", "field.audio", "070100_17"),
            ],
        );
        assert_eq!(outcome.targets.len(), 3);
        assert!(!outcome.targets[0].removed.is_empty());
        assert_eq!(outcome.targets[1].not_removed.len(), 1);
        assert!(outcome.targets[2].not_removed.is_empty());
        assert!(
            outcome.halted.is_none(),
            "a target failure is not a run halt"
        );
    }

    #[test]
    fn a_duplicate_target_is_collapsed_rather_than_reported_refused() {
        let bed = Bed::new();
        populated(&bed, "070000_17");
        let t = target("20260805", "field.audio", "070000_17");
        let outcome = remove(&bed, &[t.clone(), t]);
        assert_eq!(outcome.targets.len(), 1, "collapsed at entry");
        assert!(outcome.targets[0].not_removed.is_empty());
    }

    /// A second removal is refused, and the first tombstone is untouched.
    #[test]
    fn a_second_removal_is_refused_and_the_deletion_record_survives() {
        let bed = Bed::new();
        let segment = populated(&bed, "070000_17");
        let t = target("20260805", "field.audio", "070000_17");
        let setup = remove(&bed, std::slice::from_ref(&t));
        assert!(
            setup.targets[0].not_removed.is_empty(),
            "setup removal failed"
        );
        let first = fs::read(segment.join(TOMBSTONE_NAME)).unwrap();

        let second = remove(&bed, std::slice::from_ref(&t));
        assert!(second.targets[0].removed.is_empty());
        assert_eq!(second.targets[0].not_removed.len(), 1);
        assert_eq!(
            fs::read(segment.join(TOMBSTONE_NAME)).unwrap(),
            first,
            "the owner's deletion record must be byte-identical"
        );
    }

    /// A refused request must not leave a phantom day behind.
    ///
    /// The lock helper creates its sidecar's parent directories, so locking before
    /// filtering would create the day and stream for a target that does not exist.
    #[test]
    fn a_refused_request_creates_nothing() {
        let bed = Bed::new();
        let outcome = remove(&bed, &[target("20260806", "field.audio", "080000_1")]);
        assert_eq!(outcome.targets[0].not_removed.len(), 1);
        assert!(
            !bed.root.join("chronicle/20260806").exists(),
            "a refused request must not create a day directory"
        );
    }

    #[test]
    fn the_segment_is_not_listed_under_its_real_name_while_staged() {
        let bed = Bed::new();
        populated(&bed, "070000_17");
        // After a completed removal the name is back, holding the tombstone.
        let setup = remove(&bed, &[target("20260805", "field.audio", "070000_17")]);
        assert!(
            setup.targets[0].not_removed.is_empty(),
            "setup removal failed"
        );
        let stream = bed.root.join("chronicle/20260805/field.audio");
        let names: Vec<String> = fs::read_dir(&stream)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| !n.ends_with(".lock"))
            .collect();
        assert_eq!(names, vec!["070000_17".to_owned()], "no staged leftovers");
    }

    /// A directory name whose key differs from it round-trips exactly.
    #[test]
    fn a_suffixed_segment_name_is_restored_exactly() {
        let bed = Bed::new();
        populated(&bed, "093000_300_summary");
        let outcome = remove(
            &bed,
            &[target("20260805", "field.audio", "093000_300_summary")],
        );
        assert!(outcome.targets[0].not_removed.is_empty());
        assert!(
            bed.root
                .join("chronicle/20260805/field.audio/093000_300_summary")
                .join(TOMBSTONE_NAME)
                .exists(),
            "the original directory name must come back, not the key"
        );
    }

    // ---- recover ---------------------------------------------------------

    fn recover_all(bed: &Bed) -> Outcome {
        recover(
            &bed.root,
            "2026-08-05T22:00:00Z",
            RemovalReason::OwnerSegmentDelete,
            "sha256:abc",
        )
    }

    /// 🔴 The negative twin, and the most important test here.
    ///
    /// A recovery pass must act only on positive evidence. Case (ii) is the one
    /// that matters: an interrupted raw release leaves exactly what a
    /// "nothing started" recovery row would act on, and acting on it would turn a
    /// lifecycle release into a whole-segment deletion.
    #[test]
    fn recover_touches_nothing_without_a_staged_directory() {
        let bed = Bed::new();
        // (i) an ordinary untouched segment
        let untouched = populated(&bed, "070000_17");
        // (ii) a segment where a raw release was interrupted: raw gone, derived intact
        let released = populated(&bed, "070100_17");
        fs::remove_file(released.join("audio.flac")).unwrap();
        // (iii) a completed removal
        populated(&bed, "070200_17");
        let setup = remove(&bed, &[target("20260805", "field.audio", "070200_17")]);
        assert!(
            setup.targets[0].not_removed.is_empty(),
            "setup removal failed"
        );
        let completed = bed.root.join("chronicle/20260805/field.audio/070200_17");
        let tombstone_before = fs::read(completed.join(TOMBSTONE_NAME)).unwrap();

        let before: Vec<Vec<String>> = [&untouched, &released, &completed]
            .iter()
            .map(|dir| listing(dir))
            .collect();

        // (iv) a POSITIVE CONTROL in the same journal, so a pass that scanned
        // nothing cannot satisfy this test.
        populated(&bed, "070300_17");
        let stream = bed.root.join("chronicle/20260805/field.audio");
        fs::rename(stream.join("070300_17"), stream.join(".removing_070300_17")).unwrap();

        let outcome = recover_all(&bed);

        assert_eq!(
            outcome.targets.len(),
            1,
            "exactly the staged segment was acted on"
        );
        assert_eq!(outcome.targets[0].target.dir, "070300_17");
        assert!(outcome.targets[0].not_removed.is_empty());

        let after: Vec<Vec<String>> = [&untouched, &released, &completed]
            .iter()
            .map(|dir| listing(dir))
            .collect();
        assert_eq!(before, after, "no live segment may be touched");
        assert_eq!(
            fs::read(completed.join(TOMBSTONE_NAME)).unwrap(),
            tombstone_before
        );
    }

    fn listing(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// A live segment holding a tombstone beside media is NOT a recovery trigger.
    ///
    /// Ordinary operation can produce that state, so a pass keyed on it would
    /// re-stage and destroy newly captured media.
    #[test]
    fn a_tombstone_beside_media_is_not_a_recovery_trigger() {
        let bed = Bed::new();
        let segment = populated(&bed, "070000_17");
        fs::write(segment.join(TOMBSTONE_NAME), b"{}").unwrap();
        let before = listing(&segment);
        let outcome = recover_all(&bed);
        assert!(outcome.targets.is_empty());
        assert_eq!(listing(&segment), before);
    }

    /// Recovery resumes from each interrupted staged state.
    #[test]
    fn recover_finishes_a_staged_directory_at_every_stage() {
        // media present, no tombstone -> tombstone then empty then restore
        let bed = Bed::new();
        populated(&bed, "070000_17");
        let stream = bed.root.join("chronicle/20260805/field.audio");
        fs::rename(stream.join("070000_17"), stream.join(".removing_070000_17")).unwrap();
        let outcome = recover_all(&bed);
        assert_eq!(outcome.targets.len(), 1);
        assert_eq!(listing(&stream.join("070000_17")), vec![TOMBSTONE_NAME]);

        // only the tombstone -> restore
        let bed = Bed::new();
        let staged = bed.segment("20260805", "field.audio", ".removing_070100_17");
        fs::write(staged.join(TOMBSTONE_NAME), b"{}").unwrap();
        let outcome = recover_all(&bed);
        assert_eq!(outcome.targets.len(), 1);
        let stream = bed.root.join("chronicle/20260805/field.audio");
        assert!(stream.join("070100_17").join(TOMBSTONE_NAME).exists());
        assert!(!stream.join(".removing_070100_17").exists());

        // empty staged -> tombstone then restore
        let bed = Bed::new();
        bed.segment("20260805", "field.audio", ".removing_070200_17");
        let outcome = recover_all(&bed);
        assert_eq!(outcome.targets.len(), 1);
        let stream = bed.root.join("chronicle/20260805/field.audio");
        assert!(stream.join("070200_17").join(TOMBSTONE_NAME).exists());
    }

    /// Recovery refuses a staged directory whose real name is occupied again.
    #[test]
    fn recover_refuses_when_the_live_name_was_taken() {
        let bed = Bed::new();
        let staged = bed.segment("20260805", "field.audio", ".removing_070000_17");
        fs::write(staged.join(TOMBSTONE_NAME), b"{}").unwrap();
        // Something new appeared at the original name.
        let fresh = bed.segment("20260805", "field.audio", "070000_17");
        fs::write(fresh.join("audio.flac"), b"new recording").unwrap();

        let outcome = recover_all(&bed);
        assert_eq!(outcome.targets.len(), 1);
        assert!(outcome.targets[0].removed.is_empty());
        assert_eq!(outcome.targets[0].not_removed.len(), 1);
        assert_eq!(
            fs::read(fresh.join("audio.flac")).unwrap(),
            b"new recording",
            "the new recording must survive untouched"
        );
        assert!(
            staged.join(TOMBSTONE_NAME).exists(),
            "the staged directory is left for inspection, not merged"
        );
    }

    /// A directory this crate could not have staged is left alone.
    #[test]
    fn recover_ignores_a_directory_it_could_not_have_staged() {
        let bed = Bed::new();
        let stray = bed.segment("20260805", "field.audio", ".removing_");
        fs::write(stray.join("something.flac"), b"not ours").unwrap();
        let outcome = recover_all(&bed);
        assert!(outcome.targets.is_empty());
        assert!(stray.join("something.flac").exists());
    }
}
