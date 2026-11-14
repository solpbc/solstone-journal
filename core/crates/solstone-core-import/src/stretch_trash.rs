// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! A stretch of time, deleted as one thing: the owner picks a from and a to,
//! and everything the journal holds that was recorded then goes to the trash
//! together.
//!
//! - **What it takes is what is in the journal now.** Every segment whose
//!   capture time overlaps the stretch, from any source: each device's audio,
//!   screen and location, what imports made, and body windows. Something sent
//!   later for that time arrives as new data; nothing is remembered to refuse
//!   it.
//! - **Time is capture time in the owner's zone**, read from each segment's
//!   day and key, and whole segments are taken: the bounds the owner is shown
//!   are the taken segments' own ([`Plan::from`], [`Plan::to`]).
//! - **Imports go as [`segment_trash::choose`] decides.** A stretch holding
//!   every remaining segment of an import takes it whole, its upload included;
//!   otherwise its segments go and the upload stays.
//! - **What is still in flight is left out and named** ([`LeftOut`]): a
//!   segment still arriving or still being processed, and an import still
//!   running.
//! - **Agent memories aren't in it.** They are what an agent wrote about a
//!   time, not something recorded then.
//! - **One thing in the trash.** Every item it moves carries the stretch
//!   (`solstone-core-trash`, [`Stretch`]), under one operation, in one durable
//!   set a stop leaves for the sweep to finish.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use chrono::{Duration, NaiveDateTime};
use serde::Serialize;
use solstone_core_removals::{Action, Surface, Unit};
use solstone_core_trash::{Chosen, SetResult, Stretch, TrashError};

use crate::import_trash;
use crate::metadata::{AttemptHolder, attempt_holder};
use crate::segment_trash::{self, ImportPreview};

/// The longest stretch one delete takes.
pub const MAX_DAYS: i64 = 31;

/// Streams agents write their memories to.
const AGENT_MEMORY_STREAM_PREFIX: &str = "agent-memory-";

/// The wall-clock spelling bounds are kept and shown in.
const WALL: &str = "%Y-%m-%dT%H:%M:%S";

/// Why a stretch couldn't be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Refused {
    /// Its end isn't after its start.
    Backwards,
    /// It is longer than [`MAX_DAYS`].
    TooLong,
}

/// One segment the stretch takes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Found {
    pub day: String,
    pub stream: String,
    pub key: String,
    /// What kinds of recording it holds ([`kinds_of`]).
    pub kinds: Vec<&'static str>,
    /// The import that alone made it, if one did.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub import: Option<String>,
    #[serde(skip)]
    pub start: NaiveDateTime,
    #[serde(skip)]
    pub end: NaiveDateTime,
}

impl Found {
    /// Its journal-relative path.
    pub fn rel(&self) -> String {
        solstone_core_retention::layout::segment_rel(&self.day, &self.stream, &self.key)
    }
}

/// Why something overlapping the stretch stays in the journal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Why {
    /// It is still arriving from its device.
    Arriving,
    /// The journal is still processing it.
    Processing,
    /// The import that made it is still running.
    ImportRunning,
    /// It is about a whole day (a body source's day totals), and the stretch
    /// doesn't cover that day.
    WholeDay,
}

/// One segment overlapping the stretch that it leaves in the journal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LeftOut {
    pub day: String,
    pub stream: String,
    pub key: String,
    pub kinds: Vec<&'static str>,
    pub why: Why,
}

/// What deleting a stretch would do, for its confirmation. Reads only.
#[derive(Debug, Clone, Serialize)]
pub struct Plan {
    /// The bounds the owner is shown: the taken segments' own, whole segments
    /// counted; what was asked when it takes nothing.
    pub from: String,
    pub to: String,
    /// What was asked.
    pub asked_from: String,
    pub asked_to: String,
    /// The kinds of recording it takes, sorted ([`kind_of`]).
    pub kinds: Vec<&'static str>,
    /// What it takes. The confirmation counts none of it.
    pub segments: Vec<Found>,
    /// What overlaps it and stays, and why.
    pub left_out: Vec<LeftOut>,
    /// What it does to each import that alone made any of it.
    pub imports: Vec<ImportPreview>,
    /// Something from then is already in the trash, as its own thing: it
    /// stays there, on its own date.
    pub in_trash: bool,
}

impl Plan {
    /// The stretch its items carry.
    pub fn stretch(&self) -> Stretch {
        Stretch {
            id: String::new(),
            from: self.from.clone(),
            to: self.to.clone(),
            kinds: self.kinds.iter().map(|kind| (*kind).to_owned()).collect(),
        }
    }
}

/// Read a wall-clock bound: `YYYY-MM-DDTHH:MM`, seconds optional.
pub fn parse_wall(value: &str) -> Option<NaiveDateTime> {
    NaiveDateTime::parse_from_str(value, WALL)
        .or_else(|_| NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M"))
        .or_else(|_| NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M"))
        .ok()
}

fn wall(at: NaiveDateTime) -> String {
    at.format(WALL).to_string()
}

/// What deleting `from`..`to`, owner wall-clock times, would take.
///
/// Whole segments are taken, so what is taken can reach past what was asked.
/// The stretch then widens to that span and is read again, until it stops
/// widening: the bounds shown are the span taken, and everything in the
/// journal from then is in it or named as staying. Each pass only adds what
/// overlaps the last, so it ends; it never widens past [`MAX_DAYS`].
pub fn plan(journal: &Path, from: NaiveDateTime, to: NaiveDateTime) -> Result<Plan, Refused> {
    if to <= from {
        return Err(Refused::Backwards);
    }
    if to - from > Duration::days(MAX_DAYS) {
        return Err(Refused::TooLong);
    }
    let (mut lo, mut hi) = (from, to);
    loop {
        let mut plan = plan_once(journal, lo, hi);
        let taken_lo = plan.segments.iter().map(|found| found.start).min().unwrap_or(lo);
        let taken_hi = plan.segments.iter().map(|found| found.end).max().unwrap_or(hi);
        let (next_lo, next_hi) = (lo.min(taken_lo), hi.max(taken_hi));
        if (next_lo, next_hi) == (lo, hi) {
            plan.asked_from = wall(from);
            plan.asked_to = wall(to);
            return Ok(plan);
        }
        // Widening past the most one delete takes would show a span no pass
        // read whole: refused, as a stretch asked that long is.
        if next_hi - next_lo > Duration::days(MAX_DAYS) {
            return Err(Refused::TooLong);
        }
        lo = next_lo;
        hi = next_hi;
    }
}

/// One reading of `from`..`to`.
fn plan_once(journal: &Path, from: NaiveDateTime, to: NaiveDateTime) -> Plan {
    let listing = segment_trash::listing(journal);
    let mut running: BTreeMap<String, bool> = BTreeMap::new();
    let mut segments = Vec::new();
    let mut left_out = Vec::new();
    let mut in_trash = false;
    // A segment that began the day before can run past midnight into it.
    let mut day = from.date() - Duration::days(1);
    while day <= to.date() {
        let key = day.format("%Y%m%d").to_string();
        for (stream, segment, clock) in day_segments(journal, &key) {
            if stream.starts_with(AGENT_MEMORY_STREAM_PREFIX) {
                continue;
            }
            let Some((start, end)) = span(&key, &clock) else {
                continue;
            };
            if !overlaps(start, end, from, to) {
                continue;
            }
            let rel = solstone_core_retention::layout::segment_rel(&key, &stream, &segment);
            let place = journal.join(&rel);
            if !live(&place) {
                in_trash = in_trash
                    || solstone_core_trash::holders(journal, &rel, "")
                        .is_ok_and(|holders| !holders.is_empty());
                continue;
            }
            let import = sole_lister(&listing, &rel);
            let kinds = kinds_of(&place, &stream, import.is_some());
            let import_running = match &import {
                Some(id) => *running
                    .entry(id.clone())
                    .or_insert_with(|| attempt_holder(journal, id) == AttemptHolder::Held),
                None => crate::body_trash::stream_importing(journal, &stream),
            };
            let why = if import_running {
                Some(Why::ImportRunning)
            } else if end - start >= Duration::days(1) && !(from <= start && end <= to) {
                Some(Why::WholeDay)
            } else {
                in_flight(&place)
            };
            if let Some(why) = why {
                left_out.push(LeftOut {
                    day: key.clone(),
                    stream,
                    key: segment,
                    kinds,
                    why,
                });
                continue;
            }
            segments.push(Found {
                day: key.clone(),
                stream,
                key: segment,
                kinds,
                import,
                start,
                end,
            });
        }
        day += Duration::days(1);
    }
    segments.sort_by(|left, right| {
        (left.start, &left.stream, &left.key).cmp(&(right.start, &right.stream, &right.key))
    });
    let shown_from = segments.iter().map(|found| found.start).min().unwrap_or(from);
    let shown_to = segments.iter().map(|found| found.end).max().unwrap_or(to);
    let kinds: BTreeSet<&'static str> = segments
        .iter()
        .flat_map(|found| found.kinds.iter().copied())
        .collect();
    let paths: Vec<String> = segments.iter().map(Found::rel).collect();
    let mut imports = segment_trash::preview(journal, &paths);
    // An import another stretch's segments need stays whole in the journal:
    // taking it into this one would make erasing this one erase part of that.
    let kept_whole = needed_by_stretches(journal);
    for import in &mut imports {
        if kept_whole.contains(&import.id) {
            import.takes = false;
        }
    }
    Plan {
        from: wall(shown_from),
        to: wall(shown_to),
        asked_from: wall(from),
        asked_to: wall(to),
        kinds: kinds.into_iter().collect(),
        segments,
        left_out,
        imports,
        in_trash,
    }
}

/// Imports a segment waiting in the trash as part of a stretch needs.
fn needed_by_stretches(journal: &Path) -> BTreeSet<String> {
    solstone_core_trash::list(journal)
        .unwrap_or_default()
        .into_iter()
        .filter(|item| item.stretch.is_some())
        .flat_map(|item| item.needs)
        .filter_map(|need| need.strip_prefix("imports/").map(str::to_owned))
        .collect()
}

/// One thing's own part of the stretch: the bounds and kinds of the segments
/// it holds.
fn part_of(found: &[&Found]) -> Stretch {
    let from = found.iter().map(|found| found.start).min();
    let to = found.iter().map(|found| found.end).max();
    let kinds: BTreeSet<&'static str> = found
        .iter()
        .flat_map(|found| found.kinds.iter().copied())
        .collect();
    Stretch {
        id: String::new(),
        from: from.map(wall).unwrap_or_default(),
        to: to.map(wall).unwrap_or_default(),
        kinds: kinds.into_iter().map(str::to_owned).collect(),
    }
}

/// Every segment directory on `day`: its stream, its directory name and the
/// key its time is read from, the default stream's included. A day that can't
/// be read holds none.
fn day_segments(journal: &Path, day: &str) -> Vec<(String, String, String)> {
    let Ok(listed) = solstone_core_segment::list_segments(journal, day) else {
        return Vec::new();
    };
    listed
        .into_iter()
        .filter_map(|segment| {
            let identity = segment.record_identity().ok()?;
            Some((
                identity.stream.to_owned(),
                identity.name.to_owned(),
                identity.key.to_owned(),
            ))
        })
        .collect()
}

/// A live segment: there, and not only what a removal left.
fn live(place: &Path) -> bool {
    place.is_dir()
        && matches!(
            solstone_core_segment::owner_deletion_state(place),
            Ok(solstone_core_segment::OwnerDeletionState::Live)
        )
}

/// The import that alone lists `rel`, body imports included: a body window
/// goes as itself, but its import still says what it is.
fn sole_lister(listing: &BTreeMap<String, BTreeSet<String>>, rel: &str) -> Option<String> {
    let listers = listing.get(rel)?;
    (listers.len() == 1)
        .then(|| listers.first().cloned())
        .flatten()
}

/// The span a segment key names on its day: its start, and its start plus its
/// length. A `_0` key is one instant (a photo, a document), so its span is
/// that instant alone.
fn span(day: &str, key: &str) -> Option<(NaiveDateTime, NaiveDateTime)> {
    let date = chrono::NaiveDate::parse_from_str(day, "%Y%m%d").ok()?;
    let times = solstone_core_format::segment::segment_parse(key)?;
    let (clock, length) = key.split_once('_')?;
    if clock.len() != 6 {
        return None;
    }
    let length: i64 = length.parse().ok()?;
    let start = date.and_hms_opt(
        u32::from(times.hour),
        u32::from(times.minute),
        u32::from(times.second),
    )?;
    Some((start, start + Duration::seconds(length)))
}

/// Whether a segment spanning `start`..`end` overlaps the stretch: any time in
/// common, or, for an instant, the instant inside it.
fn overlaps(start: NaiveDateTime, end: NaiveDateTime, from: NaiveDateTime, to: NaiveDateTime) -> bool {
    if start == end {
        from <= start && start < to
    } else {
        start < to && end > from
    }
}

/// What kinds of recording a segment holds, in the words the confirmation
/// uses: `body` (a body window or workout), `imported` (what an import made),
/// or what a device recorded, `audio`, `screen` and `location`, any of them;
/// `other` when it is none of these.
pub fn kinds_of(place: &Path, stream: &str, imported: bool) -> Vec<&'static str> {
    if crate::body_trash::is_body_stream(stream)
        || place.join("body.jsonl").is_file()
        || place.join("workout.json").is_file()
    {
        return vec!["body"];
    }
    if imported {
        return vec!["imported"];
    }
    let mut kinds = BTreeSet::new();
    if let Ok(entries) = std::fs::read_dir(place) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if name == LOCATION_FILE {
                kinds.insert("location");
                continue;
            }
            match name
                .rsplit_once('.')
                .and_then(|(_, extension)| solstone_core_processing_record::media_kind(extension))
            {
                Some(solstone_core_processing_record::MediaKind::Audio) => {
                    kinds.insert("audio");
                }
                Some(_) => {
                    kinds.insert("screen");
                }
                None => {}
            }
        }
    }
    if kinds.is_empty() {
        kinds.insert("other");
    }
    kinds.into_iter().collect()
}

/// The readings a device's location source writes into its segments.
const LOCATION_FILE: &str = "location.jsonl";

/// What an ingest write leaves in a segment while it applies.
const INGEST_STAGE: &str = ".tmp_ingest-stage.tmp";

/// Whether a segment is still arriving or being processed: an ingest write
/// part way, or something holding it or one of its outputs now, or a modality
/// the journal is analyzing. A segment merely waiting for processing isn't in
/// flight: nothing is working on it, and processing may be off.
fn in_flight(place: &Path) -> Option<Why> {
    if place.join(INGEST_STAGE).exists() {
        return Some(Why::Arriving);
    }
    if solstone_core_journal_io::lock_is_held(place).unwrap_or(false) {
        return Some(Why::Processing);
    }
    if let Ok(entries) = std::fs::read_dir(place) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(target) = name.to_str().and_then(|name| name.strip_suffix(".lock")) else {
                continue;
            };
            if solstone_core_journal_io::lock_is_held(place.join(target)).unwrap_or(false) {
                return Some(Why::Processing);
            }
        }
    }
    analyzing(place).then_some(Why::Processing)
}

/// How long a reprocess's `.analyzing_<modality>` marker counts as work going
/// on: the journal's own health reading treats an older one as stale.
const ANALYZING_FRESH: std::time::Duration = std::time::Duration::from_secs(30 * 60);

/// Whether a modality of the segment is being analyzed now: a fresh marker.
fn analyzing(place: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(place) else {
        return false;
    };
    entries.flatten().any(|entry| {
        entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with(".analyzing_"))
            && entry
                .metadata()
                .and_then(|meta| meta.modified())
                .ok()
                .and_then(|at| at.elapsed().ok())
                .is_some_and(|age| age < ANALYZING_FRESH)
    })
}

/// What one delete of a stretch did.
#[derive(Debug, Serialize)]
pub struct Outcome {
    /// The stretch, as its items carry it.
    pub stretch: Stretch,
    /// When its items are erased.
    pub erase_at: Option<String>,
    /// The owner's local day its items are erased on, `YYYYMMDD`.
    pub erase_day: Option<String>,
    /// Things it moved into the trash (an import taken whole is one).
    pub moved: usize,
    /// Of them, erased now as the owner asked.
    pub erased: usize,
    /// Imports it took whole, by id.
    pub imports_taken: Vec<String>,
    /// Segments it meant to take and couldn't, with the trash's words.
    pub refused: Vec<RefusedSegment>,
    /// What overlapped it and stays, and why.
    pub left_out: Vec<LeftOut>,
    /// What it did to each import that alone made any of it.
    pub imports: Vec<ImportPreview>,
}

/// A segment the stretch couldn't take, and why.
#[derive(Debug, Serialize)]
pub struct RefusedSegment {
    pub day: String,
    pub stream: String,
    pub key: String,
    pub reason: String,
}

/// Move the stretch `plan` describes to the trash as one thing, as the owner's
/// act from `surface`; erase it now when `erase`. The plan is read again here,
/// so what moves is what the journal holds at the moment the owner confirms.
pub fn trash(
    journal: &Path,
    from: NaiveDateTime,
    to: NaiveDateTime,
    surface: Surface,
    erase: bool,
) -> Result<Result<Outcome, Refused>, TrashError> {
    let plan = match plan(journal, from, to) {
        Ok(plan) => plan,
        Err(refused) => return Ok(Err(refused)),
    };
    if plan.segments.is_empty() {
        return Err(TrashError::Missing("stretch of time".into()));
    }
    let chosen: Vec<Chosen> = plan
        .segments
        .iter()
        .map(|found| Chosen {
            unit: Unit::Segment {
                day: found.day.clone(),
                stream: found.stream.clone(),
                segment: found.key.clone(),
            },
            paths: vec![found.rel()],
            label: None,
        })
        .collect();
    let kept_whole = needed_by_stretches(journal);
    // An import another stretch needs goes as its segments here, never whole.
    let choices: Vec<segment_trash::Choice> = segment_trash::choose(journal, chosen.clone())
        .into_iter()
        .flat_map(|choice| match &choice.import {
            Some(id) if kept_whole.contains(id) => choice
                .covers
                .iter()
                .filter_map(|&index| {
                    chosen.get(index).map(|one| segment_trash::Choice {
                        chosen: one.clone(),
                        covers: vec![index],
                        import: None,
                    })
                })
                .collect::<Vec<_>>(),
            _ => vec![choice],
        })
        .collect();
    let covers: Vec<Vec<usize>> = choices.iter().map(|choice| choice.covers.clone()).collect();
    let imports_taken: Vec<String> = choices
        .iter()
        .filter_map(|choice| choice.import.clone())
        .collect();
    let parts: Vec<(Chosen, Stretch)> = choices
        .into_iter()
        .map(|choice| {
            let found: Vec<&Found> = choice
                .covers
                .iter()
                .filter_map(|&index| plan.segments.get(index))
                .collect();
            (choice.chosen, part_of(&found))
        })
        .collect();
    let (stretch, results) = crate::trash_kinds::with_kinds(journal, |kinds| {
        solstone_core_trash::detach_stretch(
            journal,
            kinds,
            parts,
            Action::StretchDelete,
            surface,
            erase,
            plan.stretch(),
        )
    })?;
    let erase_at = results.iter().find_map(|result| match result {
        SetResult::Moved { item, .. } => Some(item.erase_at.clone()),
        SetResult::Refused(_) => None,
    });
    let erase_day = results.iter().find_map(|result| match result {
        SetResult::Moved { item, .. } => item
            .erase_at()
            .map(|at| solstone_core_journal_config::owner_day(journal, at)),
        SetResult::Refused(_) => None,
    });
    let mut outcome = Outcome {
        stretch,
        erase_at,
        erase_day,
        moved: 0,
        erased: 0,
        imports_taken,
        refused: Vec::new(),
        left_out: plan.left_out.clone(),
        imports: plan.imports.clone(),
    };
    for (covered, result) in covers.into_iter().zip(results) {
        match result {
            SetResult::Moved { erased, .. } => {
                outcome.moved += 1;
                outcome.erased += usize::from(erased);
            }
            SetResult::Refused(error) => {
                for index in covered {
                    if let Some(found) = plan.segments.get(index) {
                        outcome.refused.push(RefusedSegment {
                            day: found.day.clone(),
                            stream: found.stream.clone(),
                            key: found.key.clone(),
                            reason: import_trash::failure_key(&error).to_owned(),
                        });
                    }
                }
            }
        }
    }
    Ok(Ok(outcome))
}

#[cfg(test)]
#[path = "stretch_trash_tests.rs"]
mod tests;
