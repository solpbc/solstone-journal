// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! What the journal made from recordings that go to the trash or come back.
//!
//! When a recording leaves, the activities over it are re-made at once from
//! the recordings still in the journal, with no model ([`rederive`]); one
//! with nothing left goes. A copy of each, as it was, is kept with the trash
//! item, so recovering the recording gives them back their segments. Then the
//! journal is asked to make the day again ([`record_for_trash`]): the
//! activities' talents, the day's briefing however old it is, and the week's
//! page, on the owner's own processing. While that ask is pending the index
//! writer keeps the day's briefing and the week's page out of search, and an
//! activity row naming a recording that isn't in the journal is never in it.
//!
//! Search is told through queued hints inside the trash's hold, and brought
//! up to date by [`MadeFromIt::refresh_search`] once the hold is released.
//!
//! [`rederive`]: solstone_core_system::activity_state::rederive_with
//! [`record_for_trash`]: solstone_core_system::owner_ask::record_for_trash

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use solstone_core_retention::made_from::MadeFrom;
use solstone_core_retention::receipt::Target;
use solstone_core_system::activity_state::{Rederived, rederive_with};
use solstone_core_system::owner_ask::{self, AskedActivity, AskedExperience};
use solstone_core_trash::{Change, Item};

/// What the trash keeps with an item: the activities over its recordings as
/// they were when they left.
const KEPT: &str = "made-from.json";

/// What the trash keeps of the voices drawn from its recordings.
const KEPT_VOICE: &str = "voice.json";

#[derive(Debug, Default, Serialize, Deserialize)]
struct Kept {
    version: u32,
    records: Vec<KeptRecord>,
}

/// What the trash keeps of the detections naming its recordings, in its own
/// file: a detach an older build started kept `made-from.json` without them.
const KEPT_DETECTIONS: &str = "detections.json";

/// What the trash keeps of facet Experiences touched by removed recordings.
const KEPT_EXPERIENCES: &str = "experiences.json";

#[derive(Debug, Default, Serialize, Deserialize)]
struct KeptExperiences {
    version: u32,
    records: Vec<solstone_core_facets::ExperienceRecord>,
}

#[derive(Debug, Serialize, Deserialize)]
struct KeptDetections {
    day: String,
    facet: String,
    rows: Vec<Value>,
}

#[derive(Debug, Serialize, Deserialize)]
struct KeptRecord {
    day: String,
    facet: String,
    record: Map<String, Value>,
}

/// What search was told, and what it says now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchNow {
    /// Nothing made from the recordings needed it.
    Untouched,
    /// Brought up to date before the owner's change returned.
    Now,
    /// Queued for the index writer's next drain: it was busy, or failed.
    Queued,
}

/// The journal's [`MadeFrom`].
#[derive(Default)]
pub struct MadeFromIt {
    /// Journal-relative paths whose search entries changed.
    touched: RefCell<BTreeSet<String>>,
    /// Facets whose activities on a day couldn't be re-made, by day.
    skipped: RefCell<BTreeSet<String>>,
}

impl MadeFromIt {
    pub fn new() -> Self {
        Self::default()
    }

    /// Bring search up to date for what this change touched. Call it once the
    /// trash call has returned, so no hold of the trash's is held while the
    /// index writer is waited for.
    pub fn refresh_search(&self, journal: &Path) -> SearchNow {
        let paths = std::mem::take(&mut *self.touched.borrow_mut())
            .into_iter()
            .collect::<Vec<_>>();
        if paths.is_empty() {
            return SearchNow::Untouched;
        }
        match solstone_core_indexer_store::todo::refresh_now(
            journal,
            &paths,
            solstone_core_indexer_store::todo::REFRESH_NOW_WAIT,
        ) {
            Ok(_) => SearchNow::Now,
            Err(error) => {
                log::warn!(
                    "trash: search for what was made from a recording is queued, not done now: {error}"
                );
                SearchNow::Queued
            }
        }
    }

    /// Activities on a day that couldn't be re-made (`<day>/<facet>`), for the
    /// change's own report. Their rows stay out of search while they name a
    /// recording that's gone.
    pub fn skipped(&self) -> Vec<String> {
        self.skipped.borrow().iter().cloned().collect()
    }

    fn touch(&self, journal: &Path, path: String) {
        if let Err(error) =
            solstone_core_indexer_store::todo::enqueue_path(journal, &journal.join(&path))
        {
            log::warn!("trash: search update for {path} could not be queued: {error}");
        }
        self.touched.borrow_mut().insert(path);
    }

    fn touch_day(&self, journal: &Path, day: &str) {
        self.touch(
            journal,
            format!("chronicle/{day}/talents/morning_briefing.json"),
        );
        self.touch(journal, format!("chronicle/{day}/talents/pulse.jsonl"));
        if let Some(week) = week_page(day) {
            self.touch(journal, week);
        }
    }
}

/// The week page holding `day`: `reflections/weekly/<sunday>.md`.
fn week_page(day: &str) -> Option<String> {
    let date = solstone_core_time::parse_day(day)?;
    let sunday = date
        .checked_sub(jiff::Span::new().days(date.weekday().to_sunday_zero_offset()))
        .ok()?;
    Some(format!(
        "reflections/weekly/{}.md",
        solstone_core_time::day_key(sunday)
    ))
}

/// The segments, by day, as `(stream, key)`: a record names segments by key,
/// and a folder's name can carry more than its key.
fn by_day(segments: &[Target]) -> BTreeMap<String, BTreeSet<(String, String)>> {
    let mut days = BTreeMap::<String, BTreeSet<(String, String)>>::new();
    for target in segments {
        let key = solstone_core_format::segment::segment_key(&target.dir)
            .unwrap_or_else(|| target.dir.clone());
        days.entry(target.day.clone())
            .or_default()
            .insert((target.stream.clone(), key));
    }
    days
}

/// The recordings, by day, as a detection names them: chronicle-relative.
fn origins_by_day(segments: &[Target]) -> BTreeMap<String, BTreeSet<String>> {
    let mut days = BTreeMap::<String, BTreeSet<String>>::new();
    for target in segments {
        let rel =
            solstone_core_retention::layout::segment_rel(&target.day, &target.stream, &target.dir);
        let origin = rel.strip_prefix("chronicle/").unwrap_or(&rel).to_owned();
        days.entry(target.day.clone()).or_default().insert(origin);
    }
    days
}

/// The recordings as a voiceprint row names them.
fn voice_recordings(
    days: &BTreeMap<String, BTreeSet<(String, String)>>,
) -> BTreeSet<solstone_core_entity::VoiceprintRecording> {
    days.iter()
        .flat_map(|(day, segments)| {
            segments
                .iter()
                .map(|(stream, key)| (day.clone(), stream.clone(), key.clone()))
        })
        .collect()
}

/// Whether a record was made over one of `segments`.
fn over(
    journal: &Path,
    day: &str,
    record: &Map<String, Value>,
    segments: &BTreeSet<(String, String)>,
) -> bool {
    let Some(stream) = solstone_core_segment::effective_activity_stream(journal, day, record)
    else {
        return false;
    };
    record
        .get("segments")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .any(|key| segments.contains(&(stream.to_owned(), key.to_owned())))
}

fn record_id(record: &Map<String, Value>) -> &str {
    record.get("id").and_then(Value::as_str).unwrap_or_default()
}

fn segments_of(record: &Map<String, Value>) -> Vec<String> {
    record
        .get("segments")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

/// The operation an ask is filed under: the owner act's, so a set or a
/// stretch asks once per day.
fn operation(item: &Item, change: Change) -> String {
    match change {
        Change::Detached => item.operation.clone().unwrap_or_else(|| item.id.clone()),
        Change::Attached => format!(
            "recover:{}",
            item.pending
                .as_ref()
                .and_then(|pending| pending.operation.clone())
                .unwrap_or_else(|| item.id.clone())
        ),
    }
}

impl MadeFromIt {
    /// Every activity over the segments, in every facet, as it is now.
    fn records_over(
        &self,
        journal: &Path,
        days: &BTreeMap<String, BTreeSet<(String, String)>>,
    ) -> Result<Vec<KeptRecord>, String> {
        let facets = solstone_core_facets::list_declared_facet_names(journal)
            .map_err(|error| error.to_string())?;
        let mut kept = Vec::new();
        for (day, segments) in days {
            for facet in &facets {
                let rows =
                    match solstone_core_facets::read_activity_records_strict(journal, facet, day) {
                        Ok(rows) => rows,
                        Err(error) => {
                            log::warn!("trash: activities of {facet} on {day} not read: {error}");
                            continue;
                        }
                    };
                for record in rows
                    .into_iter()
                    .filter(|record| over(journal, day, record, segments))
                {
                    kept.push(KeptRecord {
                        day: day.clone(),
                        facet: facet.clone(),
                        record,
                    });
                }
            }
        }
        Ok(kept)
    }

    /// Re-make, in place, the records of one facet and day listed in `ids`
    /// (or, for a recover, given back as `returned`), and return the ones
    /// still there for the ask.
    fn remake(
        &self,
        journal: &Path,
        day: &str,
        facet: &str,
        ids: &BTreeSet<String>,
        returned: &[Map<String, Value>],
    ) -> Vec<String> {
        let mut gone = Vec::new();
        let remade = solstone_core_facets::remake_activity_day(journal, facet, day, |rows| {
            let mut next = Vec::with_capacity(rows.len() + returned.len());
            let mut kept = Vec::new();
            let mut changed = false;
            let mut seen = BTreeSet::new();
            let force = !returned.is_empty();
            for row in rows {
                let id = record_id(row).to_owned();
                seen.insert(id.clone());
                if !ids.contains(&id) {
                    next.push(row.clone());
                    continue;
                }
                // A recover: what's on disk wins, and gains the segments the
                // kept copy had.
                let mut merged = row.clone();
                if let Some(back) = returned.iter().find(|back| record_id(back) == id) {
                    let existing = segments_of(row);
                    let mut all = existing.clone();
                    all.extend(segments_of(back));
                    all.sort();
                    all.dedup();
                    if all.len() != existing.len() || all.iter().any(|key| !existing.contains(key))
                    {
                        merged.insert("segments".into(), Value::from(all));
                    }
                }
                match rederive_with(journal, day, &merged, force) {
                    Rederived::Unchanged => {
                        if merged != *row {
                            changed = true;
                            next.push(merged);
                        } else {
                            next.push(row.clone());
                        }
                        kept.push(id);
                    }
                    Rederived::Changed(record) => {
                        changed = true;
                        next.push(record);
                        kept.push(id);
                    }
                    Rederived::Removed => {
                        changed = true;
                        gone.push(id);
                    }
                }
            }
            for back in returned {
                let id = record_id(back).to_owned();
                if seen.contains(&id) {
                    continue;
                }
                match rederive_with(journal, day, back, true) {
                    Rederived::Changed(record) => {
                        changed = true;
                        next.push(record);
                        kept.push(id);
                    }
                    Rederived::Unchanged => {
                        changed = true;
                        next.push(back.clone());
                        kept.push(id);
                    }
                    Rederived::Removed => {}
                }
            }
            (changed.then_some(next), kept)
        });
        let kept = match remade {
            Ok(Some(kept)) => kept,
            Ok(None) if returned.is_empty() => Vec::new(),
            Ok(None) => {
                // The facet's day file is gone; a recover writes it again.
                self.skipped.borrow_mut().insert(format!("{day}/{facet}"));
                Vec::new()
            }
            Err(error) => {
                log::warn!("trash: activities of {facet} on {day} not re-made: {error}");
                self.skipped.borrow_mut().insert(format!("{day}/{facet}"));
                return Vec::new();
            }
        };
        let folder = format!("facets/{facet}/activities/{day}");
        for id in &gone {
            let outputs = journal.join(&folder).join(id);
            if let Err(error) = std::fs::remove_dir_all(&outputs)
                && error.kind() != std::io::ErrorKind::NotFound
            {
                log::warn!("trash: outputs of {id} not removed: {error}");
            }
            self.touch(journal, format!("{folder}/{id}"));
        }
        for id in &kept {
            // What its talents wrote from every segment goes with them; they
            // write it again.
            let outputs = journal.join(&folder).join(id);
            if outputs.is_dir()
                && let Err(error) = std::fs::remove_dir_all(&outputs)
            {
                log::warn!("trash: outputs of {id} not removed: {error}");
            }
            self.touch(journal, format!("{folder}/{id}"));
        }
        self.touch(journal, format!("{folder}.jsonl"));
        kept
    }

    /// Every facet's detections naming the recordings, as they are.
    fn detections_over(
        &self,
        journal: &Path,
        origins: &BTreeMap<String, BTreeSet<String>>,
    ) -> Result<Vec<KeptDetections>, String> {
        // Every facet folder search reads, declared or not.
        let facets = solstone_core_facets::list_facet_directories(journal)
            .map_err(|error| error.to_string())?;
        let mut kept = Vec::new();
        for (day, gone) in origins {
            for facet in &facets {
                let rows = solstone_core_facets::detections_naming(journal, facet, day, gone)
                    .map_err(|error| format!("detections of {facet} on {day}: {error}"))?;
                if !rows.is_empty() {
                    kept.push(KeptDetections {
                        day: day.clone(),
                        facet: facet.clone(),
                        rows,
                    });
                }
            }
        }
        Ok(kept)
    }

    /// The names found in the recordings leave with them: kept once with the
    /// item, then each facet's day of detections drops them, and search reads
    /// the day again.
    fn take_detections(
        &self,
        journal: &Path,
        item: &Item,
        origins: &BTreeMap<String, BTreeSet<String>>,
    ) -> Result<(), String> {
        if solstone_core_trash::kept(journal, &item.id, KEPT_DETECTIONS)
            .map_err(|error| error.to_string())?
            .is_none()
        {
            let found = self.detections_over(journal, origins)?;
            let bytes = serde_json::to_vec(&found).map_err(|error| error.to_string())?;
            solstone_core_trash::keep(journal, &item.id, KEPT_DETECTIONS, &bytes)
                .map_err(|error| format!("what it found wasn't kept: {error}"))?;
        }
        let facets = solstone_core_facets::list_facet_directories(journal)
            .map_err(|error| error.to_string())?;
        // Each facet's day on its own: one that can't be changed is named,
        // and the rest still are.
        let mut failed = Vec::new();
        for (day, gone) in origins {
            for facet in &facets {
                match solstone_core_facets::narrow_detections(journal, facet, day, gone) {
                    Ok(before) if !before.is_empty() => {
                        self.touch(journal, format!("facets/{facet}/entities/{day}.jsonl"));
                    }
                    Ok(_) => {}
                    Err(error) => failed.push(format!("{day}/{facet}: {error}")),
                }
            }
        }
        if failed.is_empty() {
            Ok(())
        } else {
            Err(failed.join(", "))
        }
    }

    /// The voices drawn from the recordings leave every voiceprint, kept once
    /// with the item first, so a stop part way keeps the first copy.
    fn take_voices(
        &self,
        journal: &Path,
        item: &Item,
        recordings: &BTreeSet<solstone_core_entity::VoiceprintRecording>,
    ) -> Result<(), String> {
        if solstone_core_trash::kept(journal, &item.id, KEPT_VOICE)
            .map_err(|error| error.to_string())?
            .is_none()
        {
            let rows = solstone_core_entity::recording_voiceprints(journal, recordings)
                .map_err(|error| format!("its voices weren't read: {error}"))?;
            let bytes = serde_json::to_vec(&rows).map_err(|error| error.to_string())?;
            solstone_core_trash::keep(journal, &item.id, KEPT_VOICE, &bytes)
                .map_err(|error| format!("its voices weren't kept: {error}"))?;
        }
        solstone_core_entity::take_recording_voiceprints(journal, recordings)
            .map(|_| ())
            .map_err(|error| format!("its voices weren't taken out: {error}"))
    }

    /// What it found goes back, to the facets still here.
    fn put_back_detections(&self, journal: &Path, item: &Item) -> Result<(), String> {
        let Some(bytes) = solstone_core_trash::kept(journal, &item.id, KEPT_DETECTIONS)
            .map_err(|error| error.to_string())?
        else {
            return Ok(());
        };
        let kept: Vec<KeptDetections> = serde_json::from_slice(&bytes)
            .map_err(|error| format!("what it found is unreadable: {error}"))?;
        // The facets taking read; one gone since has nowhere to take its names.
        let facets = solstone_core_facets::list_facet_directories(journal)
            .map_err(|error| error.to_string())?;
        let mut failed = Vec::new();
        for found in kept.iter().filter(|found| facets.contains(&found.facet)) {
            match solstone_core_facets::restore_detections(
                journal,
                &found.facet,
                &found.day,
                &found.rows,
            ) {
                Ok(()) => self.touch(
                    journal,
                    format!("facets/{}/entities/{}.jsonl", found.facet, found.day),
                ),
                Err(error) => failed.push(format!("{}/{}: {error}", found.day, found.facet)),
            }
        }
        if failed.is_empty() {
            Ok(())
        } else {
            Err(failed.join(", "))
        }
    }

    /// The voices kept with the item go back to the people still here.
    fn put_back_voices(&self, journal: &Path, item: &Item) -> Result<(), String> {
        let Some(bytes) = solstone_core_trash::kept(journal, &item.id, KEPT_VOICE)
            .map_err(|error| error.to_string())?
        else {
            return Ok(());
        };
        let rows: Vec<solstone_core_entity::TakenVoiceprint> = serde_json::from_slice(&bytes)
            .map_err(|error| format!("its kept voices are unreadable: {error}"))?;
        let (_, homeless) = solstone_core_entity::put_back_recording_voiceprints(journal, &rows)
            .map_err(|error| format!("its voices weren't put back: {error}"))?;
        if homeless > 0 {
            // A person deleted or merged away since: their voice from it goes
            // with the item.
            self.skipped
                .borrow_mut()
                .insert(format!("voices/{homeless}"));
        }
        Ok(())
    }

    fn ask(
        &self,
        journal: &Path,
        day: &str,
        operation: &str,
        item: &str,
        activities: &[AskedActivity],
    ) -> Result<(), String> {
        owner_ask::record_for_trash(
            journal,
            day,
            operation,
            item,
            activities,
            jiff::Timestamp::now().as_millisecond(),
        )
        .map_err(|error| format!("the day was not asked to be made again: {error}"))?;
        owner_ask::wake(journal, day);
        Ok(())
    }
}

/// Drop, from every edit made before `before`, the values it records the
/// fields held just before: they can hold what a model wrote from a recording
/// now erased. What the edit set stays, so the correction stays; its undo
/// then says it can't be undone. Whether anything changed.
fn drop_old_words(record: &mut Map<String, Value>, before: jiff::Timestamp) -> bool {
    // The owner's own record holds no model words.
    if record.get("source").and_then(Value::as_str) == Some("user") {
        return false;
    }
    let Some(Value::Array(edits)) = record.get_mut("edits") else {
        return false;
    };
    let mut changed = false;
    for edit in edits.iter_mut().filter_map(Value::as_object_mut) {
        // Only an edit known to be after the delete keeps them; a time that
        // can't be read counts as before, the safe side for an erase.
        let earlier = !edit
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(|at| solstone_core_time::parse_rfc3339(at).ok())
            .is_some_and(|at| at.timestamp() > before);
        if earlier && edit.remove(solstone_core_facets::EDIT_BEFORE).is_some() {
            changed = true;
        }
    }
    changed
}

impl MadeFrom for MadeFromIt {
    fn recordings_erased(
        &self,
        journal: &Path,
        item: &Item,
        _segments: &[Target],
    ) -> Result<(), String> {
        let Some(before) = item.trashed_at() else {
            return Ok(());
        };
        let Some(bytes) = solstone_core_trash::kept(journal, &item.id, KEPT)
            .map_err(|error| error.to_string())?
        else {
            return Ok(());
        };
        let kept: Kept = serde_json::from_slice(&bytes)
            .map_err(|error| format!("what was kept is unreadable: {error}"))?;
        let mut by_place = BTreeMap::<(String, String), BTreeSet<String>>::new();
        for record in &kept.records {
            by_place
                .entry((record.day.clone(), record.facet.clone()))
                .or_default()
                .insert(record_id(&record.record).to_owned());
        }
        let mut failed = Vec::new();
        for ((day, facet), ids) in by_place {
            let result = solstone_core_facets::remake_activity_day(journal, &facet, &day, |rows| {
                let mut next = rows.to_vec();
                let mut changed = false;
                for row in next.iter_mut().filter(|row| ids.contains(record_id(row))) {
                    changed |= drop_old_words(row, before);
                }
                (changed.then_some(next), changed)
            });
            match result {
                Ok(Some(true)) => {
                    self.touch(journal, format!("facets/{facet}/activities/{day}.jsonl"))
                }
                Ok(_) => {}
                Err(error) => failed.push(format!("{day}/{facet}: {error}")),
            }
        }
        let before_iso = solstone_core_time::utc_z(before, solstone_core_time::Precision::Secs);
        if let Ok(Some(exp_bytes)) = solstone_core_trash::kept(journal, &item.id, KEPT_EXPERIENCES)
            && let Ok(kept_exps) = serde_json::from_slice::<KeptExperiences>(&exp_bytes)
            && let Ok(changed) = solstone_core_facets::scrub_experience_edit_befores(
                journal,
                &kept_exps.records,
                &before_iso,
            )
        {
            for path in changed {
                self.touch(journal, path);
            }
        }
        if failed.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "old words of activities stay: {}",
                failed.join(", ")
            ))
        }
    }

    fn recordings_changed(
        &self,
        journal: &Path,
        item: &Item,
        change: Change,
        segments: &[Target],
    ) -> Result<(), String> {
        // Nothing this handles is made from body data: a briefing never
        // reads it, and its records are read from the windows themselves.
        let segments = segments
            .iter()
            .filter(|target| !crate::body_trash::is_body_stream(&target.stream))
            .cloned()
            .collect::<Vec<_>>();
        let days = by_day(&segments);
        if days.is_empty() {
            return Ok(());
        }
        let origins = origins_by_day(&segments);
        let voices = voice_recordings(&days);
        // Voice discovery's groups point into the voices that moved: they're
        // found again from what's there, as after a wipe.
        if let Err(error) = solstone_core_speaker_resolve::discovery_scan::clear_discovery_cache(
            journal,
            &|| Ok(()),
        ) {
            log::warn!("trash: voice groups weren't cleared: {error}");
        }
        // The recording's own entries leave (or come back) with the same
        // refresh, not only when the queued drain reaches them.
        for target in &segments {
            let segment = solstone_core_retention::layout::segment_rel(
                &target.day,
                &target.stream,
                &target.dir,
            );
            self.touch(journal, segment);
        }
        let operation = operation(item, change);
        let mut by_place =
            BTreeMap::<(String, String), (BTreeSet<String>, Vec<Map<String, Value>>)>::new();
        match change {
            Change::Detached => {
                // Kept once: a re-run after a stop keeps the first copy, taken
                // before anything was re-made.
                if solstone_core_trash::kept(journal, &item.id, KEPT)
                    .map_err(|error| error.to_string())?
                    .is_none()
                {
                    let records = self.records_over(journal, &days)?;
                    let bytes = serde_json::to_vec(&Kept {
                        version: 1,
                        records,
                    })
                    .map_err(|error| error.to_string())?;
                    solstone_core_trash::keep(journal, &item.id, KEPT, &bytes)
                        .map_err(|error| format!("what was made from it wasn't kept: {error}"))?;
                }
                // Reported, never a failed change, as the ask: a damaged store
                // would otherwise hold every delete part-way in. Nothing is
                // taken that wasn't kept first.
                if let Err(error) = self.take_detections(journal, item, &origins) {
                    log::warn!("trash: what its recordings found stays: {error}");
                    self.skipped.borrow_mut().insert("detections".to_owned());
                }
                if let Err(error) = self.take_voices(journal, item, &voices) {
                    log::warn!("trash: the voices drawn from its recordings stay: {error}");
                    self.skipped.borrow_mut().insert("voices".to_owned());
                }
                for record in self.records_over(journal, &days)? {
                    by_place
                        .entry((record.day, record.facet))
                        .or_default()
                        .0
                        .insert(record_id(&record.record).to_owned());
                }
            }
            Change::Attached => {
                if let Err(error) = self.put_back_voices(journal, item) {
                    log::warn!("trash: the voices kept with it weren't put back: {error}");
                    self.skipped.borrow_mut().insert("voices".to_owned());
                }
                if let Err(error) = self.put_back_detections(journal, item) {
                    log::warn!("trash: what it found wasn't put back: {error}");
                    self.skipped.borrow_mut().insert("detections".to_owned());
                }
                let kept = solstone_core_trash::kept(journal, &item.id, KEPT)
                    .map_err(|error| error.to_string())?;
                if let Some(bytes) = kept {
                    let kept: Kept = serde_json::from_slice(&bytes)
                        .map_err(|error| format!("what was kept is unreadable: {error}"))?;
                    for record in kept.records {
                        let place = by_place.entry((record.day, record.facet)).or_default();
                        place.0.insert(record_id(&record.record).to_owned());
                        place.1.push(record.record);
                    }
                }
            }
        }
        let mut asked = BTreeMap::<String, Vec<AskedActivity>>::new();
        for ((day, facet), (ids, returned)) in &by_place {
            for activity in self.remake(journal, day, facet, ids, returned) {
                asked.entry(day.clone()).or_default().push(AskedActivity {
                    facet: facet.clone(),
                    activity,
                });
            }
        }
        for day in days.keys() {
            let activities = asked.remove(day).unwrap_or_default();
            // Reported, never a failed change: a damaged store would otherwise
            // hold every delete part-way in.
            if let Err(error) = self.ask(journal, day, &operation, &item.id, &activities) {
                log::warn!("trash: {day}: {error}");
                self.skipped.borrow_mut().insert(format!("{day}/ask"));
            }
            self.touch_day(journal, day);
        }
        // Cascade trash detach/attach onto derived facet Experiences and enqueue
        // experience_compose via OwnerAsk for any surviving Experience whose span changed.
        match change {
            Change::Detached => {
                let removed_segments: Vec<(String, String, String)> = segments
                    .iter()
                    .map(|target| {
                        let seg_key = solstone_core_format::segment::segment_key(&target.dir)
                            .unwrap_or_else(|| target.dir.clone());
                        (target.day.clone(), target.stream.clone(), seg_key)
                    })
                    .collect();
                let now_iso = solstone_core_time::utc_z(
                    jiff::Timestamp::now(),
                    solstone_core_time::Precision::Secs,
                );
                match solstone_core_facets::cascade_experiences_for_removed_segments(
                    journal,
                    &removed_segments,
                    &[],
                    &now_iso,
                ) {
                    Ok(cascade) => {
                        if !cascade.snapshots.is_empty()
                            && solstone_core_trash::kept(journal, &item.id, KEPT_EXPERIENCES)
                                .ok()
                                .flatten()
                                .is_none()
                            && let Ok(bytes) = serde_json::to_vec(&KeptExperiences {
                                version: 1,
                                records: cascade.snapshots,
                            })
                            && let Err(error) = solstone_core_trash::keep(
                                journal,
                                &item.id,
                                KEPT_EXPERIENCES,
                                &bytes,
                            )
                        {
                            log::warn!("trash: experiences snapshot wasn't kept: {error}");
                        }
                        for path in cascade.changed_files {
                            self.touch(journal, path);
                        }
                        let mut by_day = BTreeMap::<String, Vec<AskedExperience>>::new();
                        for (facet, anchor_day, experience_id) in cascade.recompose {
                            by_day
                                .entry(anchor_day.clone())
                                .or_default()
                                .push(AskedExperience {
                                    facet,
                                    anchor_day,
                                    experience_id,
                                });
                        }
                        let cause = format!("{}{operation}", owner_ask::TRASH_CAUSE);
                        let now_ms = jiff::Timestamp::now().as_millisecond();
                        for (day, exps) in by_day {
                            if let Err(error) = owner_ask::record_experiences(
                                journal,
                                &day,
                                &exps,
                                Some(&cause),
                                now_ms,
                            ) {
                                log::warn!("trash: experience ask failed for {day}: {error}");
                            }
                        }
                    }
                    Err(error) => {
                        log::warn!("trash: experiences cascade failed: {error}");
                        self.skipped.borrow_mut().insert("experiences".to_owned());
                    }
                }
            }
            Change::Attached => {
                if let Ok(Some(bytes)) =
                    solstone_core_trash::kept(journal, &item.id, KEPT_EXPERIENCES)
                    && let Ok(kept_exps) = serde_json::from_slice::<KeptExperiences>(&bytes)
                {
                    match solstone_core_facets::restore_experiences_snapshot(
                        journal,
                        &kept_exps.records,
                    ) {
                        Ok((recompose, changed_files)) => {
                            for path in changed_files {
                                self.touch(journal, path);
                            }
                            let mut by_day = BTreeMap::<String, Vec<AskedExperience>>::new();
                            for (facet, anchor_day, experience_id) in recompose {
                                by_day.entry(anchor_day.clone()).or_default().push(
                                    AskedExperience {
                                        facet,
                                        anchor_day,
                                        experience_id,
                                    },
                                );
                            }
                            let cause = format!("{}{operation}", owner_ask::TRASH_CAUSE);
                            let now_ms = jiff::Timestamp::now().as_millisecond();
                            for (day, exps) in by_day {
                                if let Err(error) = owner_ask::record_experiences(
                                    journal,
                                    &day,
                                    &exps,
                                    Some(&cause),
                                    now_ms,
                                ) {
                                    log::warn!(
                                        "trash: experience restore ask failed for {day}: {error}"
                                    );
                                }
                            }
                        }
                        Err(error) => {
                            log::warn!("trash: experiences weren't restored: {error}");
                            self.skipped.borrow_mut().insert("experiences".to_owned());
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "made_from_tests.rs"]
mod tests;
