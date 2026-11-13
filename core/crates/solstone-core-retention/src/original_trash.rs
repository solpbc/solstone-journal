// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Original recording files in the trash: what a release took from a segment
//! that stays, one item per segment.
//!
//! The door releases originals as it always has: it proves each file, stages
//! or unlinks it, and writes `original_deleted`. The trash keeps a second name
//! for each file first ([`link_before`]) and, once the door has run, keeps what
//! left ([`keep_after`]); erasing the item unlinks that last name, which is
//! when the space comes back. Its references:
//!
//! - **The segment's events**: `original_deleted` names a released file, as
//!   every reader already reads it, and `original_recovered` puts it back.
//!   Voice-only's owed read stays sticky on `original_deleted`, so an original
//!   recovered from the trash is never owed to voice-only again.
//! - **The day's stream marker**, which queues the day's follow-up.
//!
//! Its hold is the door's own for the segment. Recovering while the recording
//! itself waits in the trash is refused: the recording comes back first.

use std::path::Path;

use serde_json::{Map, Value};
use solstone_core_callosum::{CallosumEnvelope, DurableEvent, append_durable_event};
use solstone_core_journal_io::paths::contained_path;
use solstone_core_removals::{Action, Surface, Unit};
use solstone_core_trash::{Change, Hold, Item, Kind};

pub use solstone_core_trash::{Linked, erase_after_days};

use crate::door::SegmentHold;
use crate::original_deletion::{
    ORIGINAL_RECOVERED_EVENT, RawReleaseClass, recorded_original_deletions,
};
use crate::receipt::Target;

/// What the item keeps of the release, for its references.
const CLASS: &str = "class";

/// Refused: the recording these originals belong to is in the trash.
pub const RECORDING_IN_TRASH: &str = "recording_in_trash";

/// Refused: the recording these originals belong to is no longer in the
/// journal (erased, or removed for good).
pub const SEGMENT_GONE: &str = "segment_gone";

/// Refused: a different file is at an original's name now, which recovering
/// would put aside and erase.
pub const NAME_TAKEN: &str = "name_taken";

/// Where an originals item's segment stands.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SegmentState {
    /// In the journal as itself.
    Live,
    /// Waiting in the trash as its own item.
    Trashed,
    /// Erased, or removed for good.
    Gone,
}

impl SegmentState {
    #[must_use]
    pub fn tag(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Trashed => "trashed",
            Self::Gone => "gone",
        }
    }
}

/// Where `item`'s segment stands now.
#[must_use]
pub fn segment_state(journal: &Path, item: &Item) -> SegmentState {
    let Some(segment) = item.paths.first().and_then(|path| segment_of(path)) else {
        return SegmentState::Gone;
    };
    if segment_live(journal, segment) {
        return SegmentState::Live;
    }
    match solstone_core_trash::holders(journal, segment, &item.id) {
        Ok(holders) if !holders.is_empty() => SegmentState::Trashed,
        // A trash that can't be read is never taken for gone.
        Err(_) => SegmentState::Trashed,
        Ok(_) => SegmentState::Gone,
    }
}

/// What was released, by the transcripts page's words: `audio`, `screen`.
#[must_use]
pub fn modalities(item: &Item) -> Vec<&'static str> {
    let mut found = Vec::new();
    for path in &item.paths {
        let kind = path
            .rsplit('.')
            .next()
            .and_then(solstone_core_processing_record::media_kind)
            .map(|kind| match kind {
                solstone_core_processing_record::MediaKind::Audio => "audio",
                _ => "screen",
            });
        if let Some(kind) = kind
            && !found.contains(&kind)
        {
            found.push(kind);
        }
    }
    found.sort_unstable();
    found
}

/// Whether an originals item, waiting or on its way in, holds the original
/// `name` of the segment at `segment_rel`.
#[must_use]
pub fn in_trash(journal: &Path, segment_rel: &str, name: &str) -> bool {
    let path = format!("{segment_rel}/{name}");
    originals_holder(journal, &path).is_some()
}

/// The trash's kind for original files.
pub struct Originals;

/// The originals of one segment a release is about to take.
#[derive(Clone, Debug)]
pub struct Release {
    pub target: Target,
    /// Journal-relative paths of the files, inside the segment.
    pub paths: Vec<String>,
}

/// Group journal-relative original paths by their segment.
#[must_use]
pub fn releases(paths: impl IntoIterator<Item = (Target, String)>) -> Vec<Release> {
    let mut grouped: Vec<Release> = Vec::new();
    for (target, path) in paths {
        match grouped.iter_mut().find(|release| release.target == target) {
            Some(release) => {
                if !release.paths.contains(&path) {
                    release.paths.push(path);
                }
            }
            None => grouped.push(Release {
                target,
                paths: vec![path],
            }),
        }
    }
    grouped
}

/// The originals a door release of `proven` would take, by segment.
#[must_use]
pub fn of_proven(proven: &[crate::eligibility::ProvenRaw]) -> Vec<Release> {
    releases(proven.iter().map(|item| {
        (
            Target {
                day: item.day().to_owned(),
                stream: item.stream().to_owned(),
                dir: item.dir().to_owned(),
            },
            item.rel(),
        )
    }))
}

/// The originals the policy marks among `marks` name, by segment. An offload
/// mark is media offload, which keeps a copy in the backup, and isn't trashed,
/// so a name an offload mark in the same selection also holds is left out.
#[must_use]
pub fn of_policy_marks<'m>(
    marks: impl IntoIterator<Item = &'m crate::marks::Mark>,
) -> Vec<Release> {
    let marks: Vec<&crate::marks::Mark> = marks.into_iter().collect();
    let offload = |target: &Target, name: &str| {
        marks.iter().any(|mark| {
            (mark.offload.is_some() || mark.class == crate::marks::RemovalClass::OffloadRawRelease)
                && mark.target == *target
                && mark.proposal.names.iter().any(|held| held == name)
        })
    };
    releases(
        marks
            .iter()
            .filter(|mark| {
                mark.class == crate::marks::RemovalClass::PolicyRawRelease && mark.offload.is_none()
            })
            .flat_map(|mark| {
                mark.proposal
                    .names
                    .iter()
                    .filter(|name| !offload(&mark.target, name))
                    .map(|name| {
                        (
                            mark.target.clone(),
                            crate::layout::content_rel(
                                &mark.target.day,
                                &mark.target.stream,
                                &mark.target.dir,
                                name,
                            ),
                        )
                    })
            }),
    )
}

/// The originals the policy marks among checked `marks` name, by segment.
#[must_use]
pub fn of_preflight(marks: &crate::marks::PreflightMarks) -> Vec<Release> {
    of_policy_marks(marks.as_slice().iter().map(|(_, mark)| mark))
}

/// What [`link_before`] kept a second name for, and the segments it couldn't
/// hold because something else is working on them. ⛔ The caller must not
/// release a `busy` segment's originals: nothing in the trash would hold them.
#[derive(Default)]
#[must_use]
pub struct Linking {
    pub linked: Vec<Linked>,
    pub busy: Vec<(Target, String)>,
}

/// Where a run over checked `marks` must stop: the first mark, in order,
/// whose segment [`link_before`] couldn't hold.
#[must_use]
pub fn first_busy(
    marks: &crate::marks::PreflightMarks,
    busy: &[(Target, String)],
) -> Option<usize> {
    marks
        .as_slice()
        .iter()
        .position(|(_, mark)| busy.iter().any(|(target, _)| *target == mark.target))
}

/// Keep a second name for every original `releases` names, one item per
/// segment, before the door releases them. A file already gone is left out:
/// the door can't release it either. A segment something else holds is left
/// out and named in [`Linking::busy`]. A file that can't be kept refuses the
/// whole release, and the caller then releases nothing.
pub fn link_before(
    journal: &Path,
    releases: &[Release],
    class: RawReleaseClass,
    action: Action,
    surface: Surface,
    stop_at_busy: bool,
    at: chrono::DateTime<chrono::Utc>,
) -> Result<Linking, String> {
    // One owner act: the history shows it as one line however many
    // recordings it took originals from.
    let operation = solstone_core_removals::OperationId::fresh(at);
    let mut linking = Linking::default();
    for release in releases {
        // Which are there is decided under the door's hold, after its
        // recovery has put back anything a stop left part way.
        let paths = release.paths.clone();
        let files = paths
            .iter()
            .filter_map(|path| path.rsplit_once('/').map(|(_, name)| name.to_owned()))
            .collect();
        let unit = Unit::Originals {
            day: release.target.day.clone(),
            stream: release.target.stream.clone(),
            segment: release.target.dir.clone(),
            files,
        };
        match solstone_core_trash::detach_linked(
            journal,
            &Originals,
            unit,
            action,
            surface,
            paths,
            &[(CLASS, class.tag().as_bytes())],
            Some(operation.as_str().to_owned()),
        ) {
            Ok(item) => linking.linked.push(item),
            // Nothing of this segment is there, under the door's hold, to release.
            Err(solstone_core_trash::TrashError::Missing(_)) => {}
            Err(solstone_core_trash::TrashError::Held(reason)) => {
                linking.busy.push((release.target.clone(), reason));
                // A caller that releases all or nothing need not wait out the rest.
                if stop_at_busy {
                    return Ok(linking);
                }
            }
            Err(error) => {
                // Nothing has been released yet: let go of what was kept.
                let _ignored = keep_after(journal, linking.linked);
                return Err(error.to_string());
            }
        }
    }
    Ok(linking)
}

/// What [`keep_after`] settled: the items now in the trash, and what the
/// trash's sweep still has to settle.
#[derive(Debug, Default)]
pub struct Kept {
    pub items: Vec<Item>,
    pub unsettled: Vec<String>,
}

/// Once the door has run: keep what it released, drop what it left.
pub fn keep_after(journal: &Path, linked: Vec<Linked>) -> Kept {
    let mut kept = Kept::default();
    for item in linked {
        let id = item.id().to_owned();
        match solstone_core_trash::finish_linked(journal, &Originals, item) {
            Ok(Some(item)) => kept.items.push(item),
            Ok(None) => {}
            Err(error) => {
                log::warn!("trash item {id}: originals not settled yet: {error}");
                kept.unsettled.push(format!("{id}: {error}"));
            }
        }
    }
    kept
}

/// What the command line answers of what [`keep_after`] settled: each item
/// with the owner's day it is erased on (`erase_day`, `YYYYMMDD`), what the
/// sweep still owes, and one plain sentence.
#[must_use]
pub fn cli_answer(journal: &Path, kept: &Kept, now: chrono::DateTime<chrono::Utc>) -> Value {
    let day_of = |item: &Item| {
        item.erase_at()
            .map(|at| solstone_core_journal_config::owner_day(journal, at))
    };
    let items: Vec<Value> = kept
        .items
        .iter()
        .map(|item| {
            let files = match &item.unit {
                Unit::Originals { files, .. } => files.clone(),
                _ => Vec::new(),
            };
            serde_json::json!({
                "id": item.id,
                "files": files,
                "erase_at": item.erase_at,
                "erase_day": day_of(item),
            })
        })
        .collect();
    let count: usize = kept.items.iter().map(|item| item.paths.len()).sum();
    let earliest = kept.items.iter().filter_map(day_of).min();
    let today = solstone_core_journal_config::owner_day(journal, now);
    let said = earliest
        .as_deref()
        .and_then(|day| spoken_day(day, &today))
        .filter(|_| count > 0)
        .map(|day| {
            if count == 1 {
                format!(
                    "moved 1 original to the trash. the trash erases it on {day}, and you can recover it until then."
                )
            } else {
                format!(
                    "moved {count} originals to the trash. the trash erases them on {day}, and you can recover them until then."
                )
            }
        });
    serde_json::json!({ "items": items, "unsettled": kept.unsettled, "said": said })
}

/// A `YYYYMMDD` day as the trash page writes it: lowercase weekday, month and
/// day, with the year when it isn't this year.
fn spoken_day(day: &str, today: &str) -> Option<String> {
    let date = chrono::NaiveDate::parse_from_str(day, "%Y%m%d").ok()?;
    let mut text = date.format("%a %b %-d").to_string().to_lowercase();
    if today.get(..4) != day.get(..4) {
        text.push_str(&date.format(" '%y").to_string());
    }
    Some(text)
}

/// Split a release report's `outcome.targets[].removed` into what waits in
/// the trash, `trashed`, and what is gone, `removed`; and add `trash` with
/// `erase_at`, the earliest erase of the items holding them, for the owner's
/// result. Paths are journal-relative.
pub fn annotate_report(journal: &Path, report: &mut Value) {
    let Some(targets) = report
        .pointer_mut("/outcome/targets")
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    let mut earliest: Option<String> = None;
    let mut count = 0_usize;
    for target in targets {
        let Some(fields) = target.as_object_mut() else {
            continue;
        };
        let Some(removed) = fields.get("removed").and_then(Value::as_array).cloned() else {
            continue;
        };
        let (mut trashed, mut gone) = (Vec::new(), Vec::new());
        for entry in removed {
            let holder = entry
                .as_str()
                .and_then(|path| originals_holder(journal, path));
            match holder {
                Some(item) => {
                    if earliest.as_ref().is_none_or(|at| item.erase_at < *at) {
                        earliest = Some(item.erase_at.clone());
                    }
                    trashed.push(entry);
                }
                None => gone.push(entry),
            }
        }
        count = count.saturating_add(trashed.len());
        fields.insert("removed".to_owned(), Value::Array(gone));
        fields.insert("trashed".to_owned(), Value::Array(trashed));
    }
    if let Some(fields) = report.as_object_mut() {
        fields.insert(
            "trash".to_owned(),
            serde_json::json!({ "count": count, "erase_at": earliest }),
        );
    }
}

/// The originals item that holds journal-relative `path`, waiting or on its
/// way in.
fn originals_holder(journal: &Path, path: &str) -> Option<Item> {
    solstone_core_trash::holders(journal, path, "")
        .ok()?
        .iter()
        .filter_map(|id| solstone_core_trash::item(journal, id).ok().flatten())
        .find(|item| {
            matches!(item.unit, Unit::Originals { .. })
                && item.paths.iter().any(|held| held == path)
        })
}

/// The segment an item's originals belong to.
fn target_of(item: &Item) -> Option<Target> {
    match &item.unit {
        Unit::Originals {
            day,
            stream,
            segment,
            ..
        } => Some(Target {
            day: day.clone(),
            stream: stream.clone(),
            dir: segment.clone(),
        }),
        _ => None,
    }
}

/// Where the segment of `path` is, from the file's own path.
fn segment_of(path: &str) -> Option<&str> {
    path.rsplit_once('/').map(|(segment, _)| segment)
}

/// Whether the segment at `segment_rel` is in the journal as itself, not a
/// tombstone or a removal part way.
fn segment_live(journal: &Path, segment_rel: &str) -> bool {
    let place = journal.join(segment_rel);
    place.is_dir()
        && matches!(
            solstone_core_segment::owner_deletion_state(&place),
            Ok(solstone_core_segment::OwnerDeletionState::Live)
        )
}

impl Kind for Originals {
    fn handles(&self, unit: &Unit) -> bool {
        matches!(unit, Unit::Originals { .. })
    }

    fn hold<'h>(&'h self, journal: &Path, item: &Item) -> Result<Box<dyn Hold + 'h>, String> {
        let target = target_of(item)
            .ok_or_else(|| "this isn't a recording your journal can place".to_owned())?;
        // A recover needs its segment here as itself: waiting in the trash,
        // the segment comes back first; gone for good, this can't come back,
        // and the item stays on its timer.
        if item.state == solstone_core_trash::State::Trashed {
            match segment_state(journal, item) {
                SegmentState::Live => {}
                SegmentState::Trashed => {
                    return Err(serde_json::json!({ "code": RECORDING_IN_TRASH }).to_string());
                }
                SegmentState::Gone => {
                    return Err(serde_json::json!({ "code": SEGMENT_GONE }).to_string());
                }
            }
        }
        // Taken first: the door's admission puts back anything a stop left
        // part way, so what is at each name is read after that.
        let hold = SegmentHold::acquire(journal, &target)?;
        // A different file at a name would be put aside and erased by the
        // recover: the owner's newer file is never the cost of a recover. It
        // stays refused, and the item on its timer.
        if item.state == solstone_core_trash::State::Trashed {
            for path in &item.paths {
                let live = journal.join(path);
                if std::fs::symlink_metadata(&live).is_err() {
                    continue;
                }
                let ours = solstone_core_trash::held_file(journal, &item.id, path)
                    .map_err(|error| error.to_string())?;
                let same =
                    solstone_core_journal_io::entry::same_file(&live, &ours).unwrap_or(false);
                if !same {
                    return Err(serde_json::json!({ "code": NAME_TAKEN }).to_string());
                }
            }
        }
        Ok(Box::new(hold))
    }

    fn present(&self, journal: &Path, path: &str) -> bool {
        let Some(segment) = segment_of(path) else {
            return false;
        };
        segment_live(journal, segment)
            && std::fs::symlink_metadata(journal.join(path))
                .is_ok_and(|metadata| metadata.file_type().is_file())
    }

    fn returns(&self, journal: &Path, _item: &Item, path: &str) -> bool {
        // A recording erased since, or removed for good, has no place for it.
        segment_of(path).is_some_and(|segment| segment_live(journal, segment))
    }

    fn narrowed(&self, unit: &Unit, paths: &[String]) -> Unit {
        match unit {
            Unit::Originals {
                day,
                stream,
                segment,
                ..
            } => Unit::Originals {
                day: day.clone(),
                stream: stream.clone(),
                segment: segment.clone(),
                files: paths
                    .iter()
                    .filter_map(|path| path.rsplit_once('/').map(|(_, name)| name.to_owned()))
                    .collect(),
            },
            other => other.clone(),
        }
    }

    fn references(
        &self,
        journal: &Path,
        item: &Item,
        change: Change,
        files: &[String],
    ) -> Result<(), String> {
        let target = target_of(item)
            .ok_or_else(|| "this isn't a recording your journal can place".to_owned())?;
        // When it happened: the owner's recover, or the trash's own time for it.
        let at = match change {
            Change::Attached => item
                .pending
                .as_ref()
                .and_then(|pending| chrono::DateTime::parse_from_rfc3339(&pending.at).ok())
                .map(|at| at.with_timezone(&chrono::Utc)),
            Change::Detached => None,
        }
        .or_else(|| item.trashed_at())
        .unwrap_or_default();
        let mut wrote = false;
        for path in files {
            let (Some(segment), Some((_, name))) = (segment_of(path), path.rsplit_once('/')) else {
                continue;
            };
            // Gone with its recording since: that recording carries its events.
            if !segment_live(journal, segment) {
                continue;
            }
            let dir = contained_path(journal, segment).map_err(|error| error.to_string())?;
            let event = match change {
                Change::Detached => {
                    // The door wrote it unless a stop came between its release
                    // and its record. A file at the name now is not this one,
                    // and its name is not deleted.
                    if recorded_original_deletions(&dir).contains_key(name)
                        || std::fs::symlink_metadata(journal.join(path)).is_ok()
                    {
                        continue;
                    }
                    "original_deleted"
                }
                Change::Attached => ORIGINAL_RECOVERED_EVENT,
            };
            let mut extra = Map::new();
            extra.insert("name".to_owned(), Value::String(name.to_owned()));
            if change == Change::Detached {
                let class = solstone_core_trash::kept(journal, &item.id, CLASS)
                    .ok()
                    .flatten()
                    .and_then(|bytes| String::from_utf8(bytes).ok())
                    .unwrap_or_else(|| RawReleaseClass::Owner.tag().to_owned());
                extra.insert("class".to_owned(), Value::String(class));
            }
            extra.insert("item".to_owned(), Value::String(item.id.clone()));
            let envelope = CallosumEnvelope {
                tract: "retention".to_owned(),
                event: event.to_owned(),
                ts: Some(at.timestamp_millis()),
                extra,
            };
            append_durable_event(&dir, &DurableEvent::Callosum(envelope))
                .map_err(|error| format!("the recording's events weren't written: {error}"))?;
            wrote = true;
        }
        if wrote || change == Change::Attached {
            solstone_core_journal_io::bump_stream_marker(journal, &target.day)
                .map(|_| ())
                .map_err(|error| format!("the day was not queued for follow-up: {error}"))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
