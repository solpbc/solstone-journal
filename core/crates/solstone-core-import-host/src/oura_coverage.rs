// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! What Oura has answered for that the journal wrote whole:
//! `imports/.oura-coverage.json`, outside the run folder, which a finished run
//! removes.
//!
//! Per endpoint it holds the ranges covered and the holes. A date endpoint's
//! ranges are Oura days, first and last inclusive. Heartrate's are UTC instants,
//! the second the one after the last. A hole is an Oura day, or for heartrate a
//! UTC hour, with why it wasn't written whole.
//!
//! Claims are taken back eagerly and added lazily. A hole takes back what it
//! touches as soon as the run state holds it; removing a claim twice changes
//! nothing, so a resumed run may do it again. What a run fetched whole is added
//! once, when its last day is done. Both happen in the writer, under the run's
//! lock; nothing else writes this file. Nothing here reads the old sync cursor,
//! `imports/oura.json`.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;

use chrono::{DateTime, Datelike as _, NaiveDate, TimeZone as _, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use solstone_core_import_sources::oura::{ENDPOINTS, Hole, HoleReason};
use solstone_core_journal_io::{AtomicWriteOptions, atomic_replace};

/// Where coverage is kept, under the journal.
pub const COVERAGE_REL: &str = "imports/.oura-coverage.json";
const SCHEMA: &str = "solstone.oura_coverage.v1";
const HEARTRATE: &str = "heartrate";
const HOUR: i64 = 3_600;
const PRIVATE: AtomicWriteOptions = AtomicWriteOptions { mode: Some(0o600) };

/// One hole as the file keeps it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct StoredHole {
    /// An Oura day, `YYYY-MM-DD`, or for heartrate a UTC hour,
    /// `YYYY-MM-DDTHH:00:00Z`.
    pub at: String,
    pub reason: HoleReason,
}

/// One endpoint's coverage.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndpointCoverage {
    /// Sorted, and never touching each other.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub covered: Vec<[String; 2]>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub holes: Vec<StoredHole>,
}

/// The whole file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Coverage {
    schema: String,
    #[serde(default)]
    pub endpoints: BTreeMap<String, EndpointCoverage>,
}

impl Default for Coverage {
    fn default() -> Self {
        Self {
            schema: SCHEMA.to_owned(),
            endpoints: BTreeMap::new(),
        }
    }
}

/// Something a run fetched whole: Oura days, first and last inclusive, or
/// heartrate's instants, the second the one after the last.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Claim {
    Days(NaiveDate, NaiveDate),
    Instants(DateTime<Utc>, DateTime<Utc>),
}

/// Sorted half-open spans that never touch: days as day numbers, instants as
/// seconds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Spans(Vec<(i64, i64)>);

impl Spans {
    fn add(&mut self, (start, end): (i64, i64)) {
        if start >= end {
            return;
        }
        let (mut start, mut end) = (start, end);
        let mut kept = Vec::with_capacity(self.0.len() + 1);
        for &(from, to) in &self.0 {
            if to < start || from > end {
                kept.push((from, to));
            } else {
                start = start.min(from);
                end = end.max(to);
            }
        }
        kept.push((start, end));
        kept.sort_unstable();
        self.0 = kept;
    }

    fn remove(&mut self, (start, end): (i64, i64)) {
        let mut kept = Vec::with_capacity(self.0.len() + 1);
        for &(from, to) in &self.0 {
            if to <= start || from >= end {
                kept.push((from, to));
                continue;
            }
            if from < start {
                kept.push((from, start));
            }
            if to > end {
                kept.push((end, to));
            }
        }
        self.0 = kept;
    }

    fn covers(&self, (start, end): (i64, i64)) -> bool {
        self.0.iter().any(|&(from, to)| from <= start && end <= to)
    }
}

fn day_number(date: NaiveDate) -> i64 {
    i64::from(date.num_days_from_ce())
}

fn day_of(number: i64) -> Option<NaiveDate> {
    NaiveDate::from_num_days_from_ce_opt(i32::try_from(number).ok()?)
}

fn day_text(date: NaiveDate) -> String {
    date.format("%Y-%m-%d").to_string()
}

fn instant_text(seconds: i64) -> String {
    DateTime::<Utc>::from_timestamp(seconds, 0)
        .map(|instant| instant.format("%Y-%m-%dT%H:%M:%SZ").to_string())
        .unwrap_or_default()
}

fn instant_of(text: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|instant| instant.timestamp())
}

fn parse_day(text: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(text, "%Y-%m-%d").ok()
}

/// The span a hole touches: its Oura day, or heartrate's whole UTC hour.
fn hole_span(endpoint: &str, at: &str) -> Option<(i64, i64)> {
    if endpoint == HEARTRATE {
        let start = instant_of(at)?;
        let start = start - start.rem_euclid(HOUR);
        Some((start, start + HOUR))
    } else {
        let day = day_number(parse_day(at)?);
        Some((day, day + 1))
    }
}

fn claim_span(claim: Claim) -> (i64, i64) {
    match claim {
        Claim::Days(first, last) => (day_number(first), day_number(last) + 1),
        Claim::Instants(from, to) => (from.timestamp(), to.timestamp()),
    }
}

impl EndpointCoverage {
    fn spans(&self, endpoint: &str) -> Result<Spans, String> {
        let mut spans = Spans::default();
        for [from, to] in &self.covered {
            let span = if endpoint == HEARTRATE {
                instant_of(from).zip(instant_of(to))
            } else {
                parse_day(from)
                    .zip(parse_day(to))
                    .map(|(first, last)| (day_number(first), day_number(last) + 1))
            };
            spans.add(span.ok_or_else(|| format!("{endpoint}: a range isn't one: {from} {to}"))?);
        }
        Ok(spans)
    }

    fn set_spans(&mut self, endpoint: &str, spans: &Spans) {
        self.covered = spans
            .0
            .iter()
            .filter_map(|&(from, to)| {
                if endpoint == HEARTRATE {
                    Some([instant_text(from), instant_text(to)])
                } else {
                    Some([day_text(day_of(from)?), day_text(day_of(to - 1)?)])
                }
            })
            .collect();
    }
}

impl Coverage {
    /// Take back every claim a hole touches, and keep the hole. Doing it again
    /// changes nothing.
    pub(crate) fn take_back(&mut self, holes: &BTreeSet<Hole>) -> Result<(), String> {
        for hole in holes {
            let endpoint = self.endpoints.entry(hole.endpoint.clone()).or_default();
            let mut spans = endpoint.spans(&hole.endpoint)?;
            if let Some(span) = hole_span(&hole.endpoint, &hole.at) {
                spans.remove(span);
            }
            endpoint.set_spans(&hole.endpoint, &spans);
            let stored = StoredHole {
                at: hole.at.clone(),
                reason: hole.reason,
            };
            if !endpoint.holes.contains(&stored) {
                endpoint.holes.push(stored);
                endpoint.holes.sort();
            }
        }
        Ok(())
    }

    /// Take back every claim, keeping the holes: for a run whose state can't say
    /// what it could have claimed.
    pub(crate) fn take_back_all(&mut self) {
        for endpoint in self.endpoints.values_mut() {
            endpoint.covered.clear();
        }
    }

    /// Add what a finishing run fetched whole, apart from its own holes. A hole an
    /// earlier run left goes where this run fetched its whole span; this run's own
    /// holes stay, whatever it fetched.
    pub(crate) fn add(
        &mut self,
        fetched: &BTreeMap<String, Vec<Claim>>,
        holes: &BTreeSet<Hole>,
    ) -> Result<(), String> {
        for (name, claims) in fetched {
            let endpoint = self.endpoints.entry(name.clone()).or_default();
            let mut whole = Spans::default();
            for claim in claims {
                whole.add(claim_span(*claim));
            }
            endpoint
                .holes
                .retain(|hole| hole_span(name, &hole.at).is_none_or(|span| !whole.covers(span)));
            let mut spans = endpoint.spans(name)?;
            for &span in &whole.0 {
                spans.add(span);
            }
            endpoint.set_spans(name, &spans);
        }
        self.take_back(holes)?;
        self.endpoints
            .retain(|_, endpoint| *endpoint != EndpointCoverage::default());
        Ok(())
    }

    /// The days from `floor` to `today` an endpoint doesn't hold, as spans,
    /// with every day from `trailing` on asked again. Days whose only trouble is
    /// a `Deferred` hole stay out: asking can't fill those.
    pub(crate) fn unheld_spans(
        &self,
        endpoint: &str,
        floor: NaiveDate,
        trailing: NaiveDate,
        today: NaiveDate,
        zone: Tz,
    ) -> Result<Vec<(NaiveDate, NaiveDate)>, String> {
        let stored = self.endpoints.get(endpoint);
        let covered = stored.map_or_else(|| Ok(Spans::default()), |e| e.spans(endpoint))?;
        let mut deferred = BTreeSet::new();
        for hole in stored.into_iter().flat_map(|e| &e.holes) {
            if hole.reason == HoleReason::Deferred
                && let Some(day) = hole_day(endpoint, &hole.at, zone)
            {
                deferred.insert(day);
            }
        }
        let held = |day: NaiveDate| {
            let span = if endpoint == HEARTRATE {
                owner_day_instants(day, zone)
            } else {
                Some((day_number(day), day_number(day) + 1))
            };
            span.is_some_and(|span| covered.covers(span))
        };
        let mut spans: Vec<(NaiveDate, NaiveDate)> = Vec::new();
        let mut day = floor;
        while day <= today {
            if day >= trailing || (!held(day) && !deferred.contains(&day)) {
                match spans.last_mut() {
                    Some(span) if span.1.succ_opt() == Some(day) => span.1 = day,
                    _ => spans.push((day, day)),
                }
            }
            let Some(next) = day.succ_opt() else {
                break;
            };
            day = next;
        }
        Ok(spans)
    }
}

/// The owner days a set of claims covers, as inclusive `YYYYMMDD` ranges,
/// each run of days whole: never the asked range, so a failed chunk's days
/// stay out.
pub(crate) fn claimed_days(claims: &BTreeMap<String, Vec<Claim>>, zone: Tz) -> Vec<[String; 2]> {
    let mut days = Spans::default();
    for claim in claims.values().flatten() {
        let (first, last) = match *claim {
            Claim::Days(first, last) => (first, last),
            Claim::Instants(from, to) => (
                from.with_timezone(&zone).date_naive(),
                (to - chrono::Duration::seconds(1))
                    .with_timezone(&zone)
                    .date_naive(),
            ),
        };
        days.add((day_number(first), day_number(last) + 1));
    }
    days.0
        .iter()
        .filter_map(|&(from, to)| {
            Some([
                day_of(from)?.format("%Y%m%d").to_string(),
                day_of(to - 1)?.format("%Y%m%d").to_string(),
            ])
        })
        .collect()
}

/// The owner day a hole falls on.
fn hole_day(endpoint: &str, at: &str, zone: Tz) -> Option<NaiveDate> {
    if endpoint == HEARTRATE {
        let instant = DateTime::<Utc>::from_timestamp(instant_of(at)?, 0)?;
        Some(instant.with_timezone(&zone).date_naive())
    } else {
        parse_day(at)
    }
}

/// An owner day as UTC seconds, half open.
fn owner_day_instants(day: NaiveDate, zone: Tz) -> Option<(i64, i64)> {
    let start = zone
        .from_local_datetime(&day.and_hms_opt(0, 0, 0)?)
        .earliest()?;
    let end = zone
        .from_local_datetime(&day.succ_opt()?.and_hms_opt(0, 0, 0)?)
        .earliest()?;
    Some((start.timestamp(), end.timestamp()))
}

/// The coverage a journal holds. None yet is none; a file that can't be read is
/// an error, since a run that went on would forget its holes.
pub(crate) fn read_coverage(journal: &Path) -> Result<Coverage, String> {
    let path = journal.join(COVERAGE_REL);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Coverage::default());
        }
        Err(error) => return Err(unreadable(&path, &error.to_string())),
    };
    let coverage: Coverage =
        serde_json::from_slice(&bytes).map_err(|error| unreadable(&path, &error.to_string()))?;
    if coverage.schema != SCHEMA {
        return Err(unreadable(&path, &format!("schema {}", coverage.schema)));
    }
    for (endpoint, held) in &coverage.endpoints {
        held.spans(endpoint)
            .map_err(|error| unreadable(&path, &error))?;
    }
    Ok(coverage)
}

fn unreadable(path: &Path, detail: &str) -> String {
    format!(
        "the record of what Oura has answered for can't be read ({}: {detail}); move it aside to start that record again",
        path.display()
    )
}

/// Replace the file, when what it holds changed.
pub(crate) fn save_coverage(journal: &Path, coverage: &Coverage) -> Result<(), String> {
    let path = journal.join(COVERAGE_REL);
    let mut bytes = serde_json::to_vec_pretty(coverage).map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    if std::fs::read(&path).is_ok_and(|held| held == bytes) {
        return Ok(());
    }
    atomic_replace(&path, &bytes, PRIVATE).map_err(|error| error.to_string())
}

/// Read, change and save the file.
pub(crate) fn update_coverage(
    journal: &Path,
    change: impl FnOnce(&mut Coverage) -> Result<(), String>,
) -> Result<(), String> {
    let mut coverage = read_coverage(journal)?;
    change(&mut coverage)?;
    save_coverage(journal, &coverage)
}

/// A UTC hour as a person reads it: `2026-01-18 17:00 UTC`.
fn hour_words(at: &str) -> String {
    instant_of(at)
        .and_then(|seconds| DateTime::<Utc>::from_timestamp(seconds, 0))
        .map_or_else(
            || at.to_owned(),
            |instant| instant.format("%Y-%m-%d %H:%M UTC").to_string(),
        )
}

/// Where a hole is, as a person reads it.
pub(crate) fn hole_place(endpoint: &str, at: &str) -> String {
    if endpoint == HEARTRATE {
        hour_words(at)
    } else {
        at.to_owned()
    }
}

/// Each endpoint's covered ranges and its holes, for the entry to print.
#[must_use]
pub fn coverage_text(coverage: &Coverage) -> String {
    let mut text = String::from("Read from Oura and written in full so far:\n");
    for name in ENDPOINTS {
        let held = coverage.endpoints.get(name).cloned().unwrap_or_default();
        let ranges = if held.covered.is_empty() {
            "nothing yet".to_owned()
        } else if name == HEARTRATE {
            held.covered
                .iter()
                .map(|[from, to]| format!("{} to {}", hour_words(from), hour_words(to)))
                .collect::<Vec<_>>()
                .join(", ")
        } else {
            let days = held
                .covered
                .iter()
                .map(|[first, last]| {
                    if first == last {
                        first.clone()
                    } else {
                        format!("{first} to {last}")
                    }
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("Oura's days {days}")
        };
        let _ = writeln!(text, "- {name}: {ranges}");
        for hole in &held.holes {
            let _ = writeln!(
                text,
                "  - not written in full, {}: {}",
                hole_place(name, &hole.at),
                hole.reason.words()
            );
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(text: &str) -> NaiveDate {
        parse_day(text).unwrap()
    }

    #[test]
    fn unheld_days_leave_out_a_deferred_one_and_ask_other_holes_and_the_trailing_week() {
        let mut coverage = Coverage::default();
        coverage.endpoints.insert(
            "sleep".to_owned(),
            EndpointCoverage {
                covered: vec![
                    ["2026-01-01".to_owned(), "2026-01-09".to_owned()],
                    ["2026-01-11".to_owned(), "2026-01-14".to_owned()],
                    ["2026-01-16".to_owned(), "2026-01-27".to_owned()],
                ],
                holes: vec![
                    StoredHole {
                        at: "2026-01-10".to_owned(),
                        reason: HoleReason::Deferred,
                    },
                    StoredHole {
                        at: "2026-01-15".to_owned(),
                        reason: HoleReason::Unreadable,
                    },
                ],
            },
        );
        let spans = coverage
            .unheld_spans(
                "sleep",
                day("2026-01-01"),
                day("2026-01-25"),
                day("2026-01-27"),
                chrono_tz::UTC,
            )
            .unwrap();
        assert_eq!(
            spans,
            [
                (day("2026-01-15"), day("2026-01-15")),
                (day("2026-01-25"), day("2026-01-27")),
            ]
        );
    }

    fn hole(endpoint: &str, at: &str, reason: HoleReason) -> Hole {
        Hole {
            endpoint: endpoint.to_owned(),
            at: at.to_owned(),
            reason,
        }
    }

    #[test]
    fn a_hole_splits_a_claim_and_a_later_whole_fetch_clears_it() {
        let mut coverage = Coverage::default();
        let fetched = BTreeMap::from([(
            "sleep".to_owned(),
            vec![Claim::Days(day("2026-01-01"), day("2026-01-10"))],
        )]);
        coverage.add(&fetched, &BTreeSet::new()).unwrap();
        let deferred = BTreeSet::from([hole("sleep", "2026-01-03", HoleReason::Deferred)]);
        coverage.take_back(&deferred).unwrap();
        coverage.take_back(&deferred).unwrap();
        let sleep = &coverage.endpoints["sleep"];
        assert_eq!(
            sleep.covered,
            [
                ["2026-01-01".to_owned(), "2026-01-02".to_owned()],
                ["2026-01-04".to_owned(), "2026-01-10".to_owned()],
            ]
        );
        assert_eq!(sleep.holes.len(), 1);
        coverage.add(&fetched, &BTreeSet::new()).unwrap();
        assert_eq!(
            coverage.endpoints["sleep"],
            EndpointCoverage {
                covered: vec![["2026-01-01".to_owned(), "2026-01-10".to_owned()]],
                holes: Vec::new(),
            }
        );
    }

    #[test]
    fn a_heartrate_hole_takes_back_its_whole_hour_and_only_a_whole_hour_clears_it() {
        let at = |text: &str| DateTime::parse_from_rfc3339(text).unwrap().to_utc();
        let mut coverage = Coverage::default();
        let day = BTreeMap::from([(
            HEARTRATE.to_owned(),
            vec![Claim::Instants(
                at("2026-01-18T00:00:00Z"),
                at("2026-01-19T00:00:00Z"),
            )],
        )]);
        coverage.add(&day, &BTreeSet::new()).unwrap();
        let unreadable = BTreeSet::from([hole(
            HEARTRATE,
            "2026-01-18T17:00:00Z",
            HoleReason::Unreadable,
        )]);
        coverage.take_back(&unreadable).unwrap();
        assert_eq!(
            coverage.endpoints[HEARTRATE].covered,
            [
                [
                    "2026-01-18T00:00:00Z".to_owned(),
                    "2026-01-18T17:00:00Z".to_owned()
                ],
                [
                    "2026-01-18T18:00:00Z".to_owned(),
                    "2026-01-19T00:00:00Z".to_owned()
                ],
            ]
        );
        // Half the hour isn't the hour.
        let half = BTreeMap::from([(
            HEARTRATE.to_owned(),
            vec![Claim::Instants(
                at("2026-01-18T17:00:00Z"),
                at("2026-01-18T17:30:00Z"),
            )],
        )]);
        coverage.add(&half, &BTreeSet::new()).unwrap();
        assert_eq!(coverage.endpoints[HEARTRATE].holes.len(), 1);
        let whole = BTreeMap::from([(
            HEARTRATE.to_owned(),
            vec![Claim::Instants(
                at("2026-01-18T17:00:00Z"),
                at("2026-01-18T18:00:00Z"),
            )],
        )]);
        coverage.add(&whole, &BTreeSet::new()).unwrap();
        assert!(coverage.endpoints[HEARTRATE].holes.is_empty());
        assert_eq!(coverage.endpoints[HEARTRATE].covered.len(), 1);
    }
}
