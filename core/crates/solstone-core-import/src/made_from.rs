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

use chrono::Datelike;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use solstone_core_retention::made_from::MadeFrom;
use solstone_core_retention::receipt::Target;
use solstone_core_system::activity_state::{Rederived, rederive_with};
use solstone_core_system::owner_ask::{self, AskedActivity};
use solstone_core_trash::{Change, Item};

/// What the trash keeps with an item: the activities over its recordings as
/// they were when they left.
const KEPT: &str = "made-from.json";

#[derive(Debug, Default, Serialize, Deserialize)]
struct Kept {
    version: u32,
    records: Vec<KeptRecord>,
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
    let date = chrono::NaiveDate::parse_from_str(day, "%Y%m%d").ok()?;
    let sunday = date - chrono::Duration::days(i64::from(date.weekday().num_days_from_sunday()));
    Some(format!("reflections/weekly/{}.md", sunday.format("%Y%m%d")))
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

/// Whether a record was made over one of `segments`.
fn over(record: &Map<String, Value>, segments: &BTreeSet<(String, String)>) -> bool {
    let Some(stream) = record.get("stream").and_then(Value::as_str) else {
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
                for record in rows.into_iter().filter(|record| over(record, segments)) {
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
                    let mut all = segments_of(row);
                    all.extend(segments_of(back));
                    all.sort();
                    all.dedup();
                    merged.insert("segments".into(), Value::from(all));
                }
                match rederive_with(journal, day, &merged, force) {
                    Rederived::Unchanged => {
                        next.push(row.clone());
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
            chrono::Utc::now().timestamp_millis(),
        )
        .map_err(|error| format!("the day was not asked to be made again: {error}"))?;
        owner_ask::wake(journal, day);
        Ok(())
    }
}

impl MadeFrom for MadeFromIt {
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
                for record in self.records_over(journal, &days)? {
                    by_place
                        .entry((record.day, record.facet))
                        .or_default()
                        .0
                        .insert(record_id(&record.record).to_owned());
                }
            }
            Change::Attached => {
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
        Ok(())
    }
}

#[cfg(test)]
#[path = "made_from_tests.rs"]
mod tests;
