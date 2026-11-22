// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The day's body document: `chronicle/<day>/body.md`, for search.
//!
//! It says what was recorded that day and by what, never a reading's value, so
//! search finds the day by what a person would type. It sits here because it
//! shares location's whole lifecycle: rebuilt deterministically from the segments
//! that survive, in the same maintenance pass and under the same owned-frame I/O.
//! `chronicle/<day>/health/body.json` records which state it was built from.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use jiff::ZonedDateTime;
use jiff::civil::DateTime;
use jiff::tz::TimeZone;
use serde::{Deserialize, Serialize};
use solstone_core_format::content::body_hidden_days::{HiddenDays, RowDays, read_hidden_days};
use solstone_core_format::content::body_records::{
    BODY_RECORD_STREAMS, BODY_RECORDS_FILE, BodyRow, RowKind, decode_rows, one_copy_per_row,
    parse_source_time, source_words, user_entered,
};
use solstone_core_format::stored_markdown::{
    Document, Message,
    body_day::{Copy, TallyKind},
};
use solstone_core_journal_io::{HealthMarkerKind, day_dirs};

use crate::projection::{ProjectionError, marker_token};

pub const BODY_DOCUMENT: &str = "body.md";
const BUILD_RECORD: &str = "body.json";
const RECORD_VERSION: u32 = 1;
/// Bumped whenever the document's wording changes, so each day is rewritten once.
const DOCUMENT_VERSION: u32 = 1;

fn document_path(journal: &Path, day: &str) -> PathBuf {
    journal.join("chronicle").join(day).join(BODY_DOCUMENT)
}

fn record_path(journal: &Path, day: &str) -> PathBuf {
    journal
        .join("chronicle")
        .join(day)
        .join("health")
        .join(BUILD_RECORD)
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct BuildRecord {
    version: u32,
    source: String,
    document: u32,
}

/// How far back an experience can start and still have pieces on a day.
const REACH_DAYS: i64 = 14;

/// What a day's document depends on: what its segments did, either way, and
/// which days within an experience's reach the owner hid. Nothing is added
/// while none is, so a journal with no hidden day builds as it did.
fn build_token(journal: &Path, day: &str, hidden: &BTreeSet<String>) -> String {
    let markers = format!(
        "{}+{}",
        marker_token(journal, day, HealthMarkerKind::Stream),
        marker_token(journal, day, HealthMarkerKind::Body)
    );
    let reach = days_reaching(day);
    let within: Vec<&str> = hidden
        .iter()
        .filter(|hidden| reach.contains(*hidden))
        .map(String::as_str)
        .collect();
    if within.is_empty() {
        markers
    } else {
        format!("{markers}+hidden:{}", within.join(","))
    }
}

/// `day` and the days before it an experience on it can have started on.
fn days_reaching(day: &str) -> BTreeSet<String> {
    let Some(date) = solstone_core_time::parse_day(day) else {
        return BTreeSet::from([day.to_owned()]);
    };
    (0..=REACH_DAYS)
        .map(|back| {
            solstone_core_time::day_key(
                date.checked_sub(jiff::Span::new().days(back))
                    .expect("a day a few days back"),
            )
        })
        .collect()
}

/// A day's rows the owner hasn't hidden: none on a hidden day, and on another
/// day none of a night or workout that started on one.
fn shown_rows(
    journal: &Path,
    day: &str,
    hidden: &BTreeSet<String>,
    mut rows: BTreeMap<String, Vec<BodyRow>>,
) -> BTreeMap<String, Vec<BodyRow>> {
    if hidden.contains(day) {
        return BTreeMap::new();
    }
    if hidden.is_empty() {
        return rows;
    }
    for (source, rows) in &mut rows {
        let zone = solstone_core_journal_config::placed_zone(journal, source);
        let days = RowDays::new(day, zone, rows.iter());
        rows.retain(|row| !days.hidden(row, hidden));
    }
    rows.retain(|_, rows| !rows.is_empty());
    rows
}

/// A day's rows per source. A window's unreadable file stops the day: writing a
/// document without it would state less than the journal holds.
fn read_rows(journal: &Path, day: &str) -> Result<BTreeMap<String, Vec<BodyRow>>, ProjectionError> {
    let mut by_source: BTreeMap<String, Vec<BodyRow>> = BTreeMap::new();
    for stream in BODY_RECORD_STREAMS {
        let dir = journal.join("chronicle").join(day).join(stream);
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        let mut windows = Vec::new();
        for entry in entries.flatten() {
            // The stream directory also holds each window's `<key>.lock`, and a removal
            // in progress keeps its file under a staged name.
            if !entry.file_type().is_ok_and(|kind| kind.is_dir())
                || entry.file_name().to_string_lossy().starts_with('.')
            {
                continue;
            }
            let path = entry.path().join(BODY_RECORDS_FILE);
            let text = match fs::read_to_string(&path) {
                Ok(text) => text,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(ProjectionError::Write {
                        path,
                        detail: error.to_string(),
                    });
                }
            };
            let rows = decode_rows(&text).map_err(|error| ProjectionError::Write {
                path,
                detail: error.to_string(),
            })?;
            windows.push((entry.file_name().to_string_lossy().into_owned(), rows));
        }
        // A moved row marks what is in another window now, so a source whose
        // rows here are all moved has nothing to say for the day. A row an
        // archive merge left a second copy of counts once.
        for (_, rows) in one_copy_per_row(windows) {
            for row in rows {
                by_source.entry(row.source.clone()).or_default().push(row);
            }
        }
    }
    Ok(by_source)
}

/// A time on the clock that placed this source's windows, so the day reads as its
/// windows do, whatever offset an export rendered. Its own clock when there's no
/// such record.
fn on_clock(time: &ZonedDateTime, zone: Option<&TimeZone>) -> DateTime {
    zone.map_or_else(
        || time.datetime(),
        |zone| zone.to_datetime(time.timestamp()),
    )
}

fn clock(time: &ZonedDateTime, zone: Option<&TimeZone>) -> String {
    on_clock(time, zone).strftime("%H:%M").to_string()
}

fn time_of(text: Option<&str>) -> Option<ZonedDateTime> {
    parse_source_time(text?)
}

fn span_times(row: &BodyRow, day: &str, zone: Option<&TimeZone>) -> (String, String) {
    let word = |text: Option<&str>| match time_of(text).map(|time| on_clock(&time, zone)) {
        Some(time) if solstone_core_time::day_key(time.date()) == day => {
            time.strftime("%H:%M").to_string()
        }
        Some(time) => time.strftime("%Y-%m-%d %H:%M").to_string(),
        None => String::new(),
    };
    (word(row.start.as_deref()), word(row.end.as_deref()))
}

#[derive(Default)]
struct Tally {
    count: u64,
    first: Option<ZonedDateTime>,
    last: Option<ZonedDateTime>,
}

impl Tally {
    fn add(&mut self, row: &BodyRow) {
        self.count += 1;
        if let Some(time) = time_of(row.begins()) {
            self.first = Some(
                self.first
                    .take()
                    .map_or_else(|| time.clone(), |first| first.min(time.clone())),
            );
            self.last = Some(self.last.take().map_or(time.clone(), |last| last.max(time)));
        }
    }

    fn times(&self, zone: Option<&TimeZone>) -> (Option<String>, Option<String>) {
        let start = self.first.as_ref().map(|time| clock(time, zone));
        let end = self
            .last
            .as_ref()
            .filter(|time| Some(*time) != self.first.as_ref())
            .map(|time| clock(time, zone));
        (start, end)
    }
}

fn line(document: &mut Document, prefix: &str, copy: Copy) {
    document.push_str(prefix);
    document.push_message(Message::BodyDay { copy });
    document.push_str("\n");
}

/// A source's rows for one day. The same rows make the same bytes, in any order.
fn render_source(
    document: &mut Document,
    source: &str,
    rows: &[BodyRow],
    day: &str,
    zone: Option<&TimeZone>,
) {
    line(
        document,
        "## ",
        Copy::Source {
            source: source_words(source).to_owned(),
        },
    );
    document.push_str("\n");
    let starts_today: BTreeSet<&str> = rows
        .iter()
        .filter(|row| row.kind == RowKind::Experience)
        .map(|row| row.id.as_str())
        .collect();
    let mut experiences = Vec::new();
    let mut continuing = BTreeMap::new();
    // Logged entries sort under who recorded them, and say the owner entered one
    // only where the source says so.
    let mut tallies: BTreeMap<(u8, String, String, bool), Tally> = BTreeMap::new();
    let mut dated = Vec::new();
    let mut documents = BTreeSet::new();
    let mut document_count = 0;
    let mut words = BTreeSet::new();
    let mut owners = BTreeSet::new();
    for row in rows {
        match row.kind {
            RowKind::Experience | RowKind::ExperienceTile => {
                let tile = row.kind == RowKind::ExperienceTile;
                let of = row.of.as_deref().unwrap_or_default();
                if tile && starts_today.contains(of) {
                    continue;
                }
                let (start, end) = span_times(row, day, zone);
                let copy = Copy::Experience {
                    continuing: tile,
                    experience: row.experience.clone(),
                    name: row.label.clone(),
                    start,
                    end,
                    recorder: row.by.clone(),
                    words: (!tile && !row.words.is_empty()).then(|| row.words.join(", ")),
                };
                let english = copy.render("en-US");
                if tile {
                    // A repeated tile may supply the same words as a fallback.
                    // Only mark their one displayed line when the sources agree.
                    continuing
                        .entry((of.to_owned(), english))
                        .and_modify(|previous: &mut Option<Copy>| {
                            if previous.as_ref() != Some(&copy) {
                                *previous = None;
                            }
                        })
                        .or_insert(Some(copy));
                } else {
                    let begins =
                        time_of(row.begins()).map(|time| time.timestamp().as_millisecond());
                    experiences.push(((begins, row.id.clone(), english), copy));
                }
            }
            // A dated row with a value says it. One without, such as Oura's daily
            // documents, is named only: what it holds stays for an agent to read.
            RowKind::Dated => match row.value.as_deref() {
                Some(value) => dated.push(Copy::Dated {
                    name: row.label.clone(),
                    value: value.to_owned(),
                }),
                None => {
                    documents.insert(row.label.clone());
                    document_count += 1;
                }
            },
            RowKind::Event | RowKind::Moved => {}
            RowKind::Stage | RowKind::Reading | RowKind::Count | RowKind::Logged => {
                let by = row.by.clone().unwrap_or_default();
                let key = match row.kind {
                    RowKind::Stage => (0, row.label.clone(), by, false),
                    RowKind::Reading => (1, row.label.clone(), by, false),
                    RowKind::Count => (2, row.label.clone(), by, false),
                    _ => (3, by, row.label.clone(), user_entered(row)),
                };
                tallies.entry(key).or_default().add(row);
                if row.kind == RowKind::Logged {
                    // The owner's own words, where the source marks them so, say
                    // whose they are; any other words are the source's.
                    let own =
                        solstone_core_format::content::body_records::owner_words(&row.source, row);
                    if own.is_empty() {
                        words.extend(row.words.iter().cloned());
                    } else {
                        owners.extend(own);
                    }
                }
            }
        }
    }
    experiences.sort_by(|(left, _), (right, _)| left.cmp(right));
    for (_, copy) in experiences {
        line(document, "- ", copy);
    }
    for ((_, english), agreed) in continuing {
        if let Some(copy) = agreed {
            line(document, "- ", copy);
        } else {
            document.push_str("- ");
            document.push_str(&english);
            document.push_str("\n");
        }
    }
    let mut names: Vec<_> = documents.into_iter().collect();
    names.sort_by_cached_key(|name| (name.to_lowercase(), name.clone()));
    if let Some(last) = names.pop() {
        dated.push(Copy::Documents {
            count: document_count,
            source: source_words(source).to_owned(),
            leading: (!names.is_empty()).then(|| names.join(", ")),
            last,
        });
    }
    dated.sort_by_cached_key(|copy| copy.render("en-US"));
    for copy in dated {
        line(document, "- ", copy);
    }
    for ((order, first, second, entered), tally) in &tallies {
        let (section, name, by) = match order {
            0 => (TallyKind::Stage, first, second),
            1 => (TallyKind::Reading, first, second),
            2 => (TallyKind::Count, first, second),
            _ => (TallyKind::Logged, second, first),
        };
        let (start, end) = tally.times(zone);
        line(
            document,
            "- ",
            Copy::Tally {
                section,
                count: tally.count,
                name: name.clone(),
                start,
                end,
                recorder: (!by.is_empty()).then(|| by.clone()),
                entered: *entered,
            },
        );
    }
    for (owner, terms) in [(false, words), (true, owners)] {
        if !terms.is_empty() {
            line(
                document,
                "- ",
                Copy::Words {
                    owner,
                    words: terms.into_iter().collect::<Vec<_>>().join(", "),
                },
            );
        }
    }
}

pub(crate) fn render_display_document(
    journal: &Path,
    day: &str,
    rows: &BTreeMap<String, Vec<BodyRow>>,
) -> Document {
    let mut document = Document::default();
    let heading = solstone_core_time::parse_day(day).map_or_else(
        || Copy::RawDay {
            day: day.to_owned(),
        },
        |date| Copy::Heading { date },
    );
    line(&mut document, "# ", heading);
    for (source, rows) in rows {
        document.push_str("\n");
        let zone = solstone_core_journal_config::placed_zone(journal, source);
        render_source(&mut document, source, rows, day, zone.as_ref());
    }
    while document.ends_with("\n\n") {
        document.truncate(document.len() - 1);
    }
    document
}

fn rebuild_day(
    journal: &Path,
    day: &str,
    hidden: &BTreeSet<String>,
) -> Result<(), ProjectionError> {
    // The marker is read first: a change that lands while this reads moves it, and
    // the next pass builds again.
    let source = build_token(journal, day, hidden);
    let rows = shown_rows(journal, day, hidden, read_rows(journal, day)?);
    let document = document_path(journal, day);
    let record = record_path(journal, day);
    let write_error = |path: &Path, error: std::io::Error| ProjectionError::Write {
        path: path.to_path_buf(),
        detail: error.to_string(),
    };
    if rows.is_empty() {
        let had = crate::day_display::publish(journal, &document, None, || Ok(()))
            .map_err(|e| write_error(&document, e))?;
        if hidden.contains(day) {
            // A hidden day is current with no document: its record says so.
            write_record(&record, source).map_err(|e| write_error(&record, e))?;
        } else {
            crate::reconcile_io::remove(&record).map_err(|e| write_error(&record, e))?;
        }
        if had {
            let rel = format!("{day}/{BODY_DOCUMENT}");
            crate::reconcile_io::enqueue(journal, Path::new(&rel)).map_err(|error| {
                ProjectionError::Index {
                    path: document.clone(),
                    detail: error.to_string(),
                }
            })?;
        }
        return Ok(());
    }
    let text = render_display_document(journal, day, &rows);
    crate::day_display::publish(journal, &document, Some(&text), || Ok(()))
        .map_err(|e| write_error(&document, e))?;
    // A current record certifies the document and its index hint alike.
    crate::reconcile_io::enqueue(journal, &document).map_err(|error| ProjectionError::Index {
        path: document.clone(),
        detail: error.to_string(),
    })?;
    write_record(&record, source).map_err(|e| write_error(&record, e))
}

fn write_record(record: &Path, source: String) -> std::io::Result<()> {
    let built = BuildRecord {
        version: RECORD_VERSION,
        source,
        document: DOCUMENT_VERSION,
    };
    let mut bytes = serde_json::to_vec_pretty(&built).unwrap_or_default();
    bytes.push(b'\n');
    crate::reconcile_io::replace(record, &bytes)
}

fn holds_body_stream(journal: &Path, day: &str) -> bool {
    BODY_RECORD_STREAMS
        .iter()
        .any(|stream| journal.join("chronicle").join(day).join(stream).is_dir())
}

/// Remembers, across passes, which days are already current.
#[derive(Debug, Default)]
pub struct BodyDays {
    seen: HashMap<String, String>,
}

impl BodyDays {
    /// Bring every day's document up to date. A day whose markers haven't moved
    /// costs their read. Failures are logged here, since this pass runs whatever
    /// happens to location's.
    pub fn reconcile(&mut self, journal: &Path) {
        // While the owner's hidden days can't be read no day is rendered: the
        // documents stay as they were until it reads again.
        let HiddenDays::Days(hidden) = read_hidden_days(journal) else {
            log::warn!("body documents wait: the hidden body days can't be read");
            return;
        };
        let mut days: Vec<String> = day_dirs(journal)
            .map(|days| days.into_keys().collect())
            .unwrap_or_default();
        days.sort_unstable();
        self.seen.retain(|day, _| days.binary_search(day).is_ok());
        for day in days {
            let token = build_token(journal, &day, &hidden);
            if self.seen.get(&day) == Some(&token) {
                continue;
            }
            let document = document_path(journal, &day);
            let record = record_path(journal, &day);
            let current = fs::read(&record)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<BuildRecord>(&bytes).ok())
                .is_some_and(|built| {
                    built.version == RECORD_VERSION
                        && built.source == token
                        && built.document == DOCUMENT_VERSION
                });
            let quiet = if current {
                document.is_file() || hidden.contains(&day)
            } else {
                !document.exists() && !record.exists() && !holds_body_stream(journal, &day)
            };
            if !quiet
                && let Err(error) =
                    crate::reconcile_io::transaction(|| rebuild_day(journal, &day, &hidden))
            {
                log::warn!("body document for {day}: {error}");
                continue;
            }
            self.seen.insert(day, token);
        }
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use solstone_core_format::content::body_records::encode_rows;
    use solstone_core_journal_io::{bump_body_marker, bump_stream_marker};

    const STREAM: &str = "import.apple_health";

    fn reading(id: &str, at: &str, value: &str) -> BodyRow {
        let mut row = BodyRow::new(
            RowKind::Reading,
            id.into(),
            "apple_health",
            "HKQuantityTypeIdentifierHeartRate",
            "heart rate".into(),
        );
        row.at = Some(at.into());
        row.value = Some(value.into());
        row.unit = Some("count/min".into());
        row.by = Some("Synthetic Watch".into());
        row
    }

    fn night(kind: RowKind, id: &str, of: Option<&str>) -> BodyRow {
        let mut row = BodyRow::new(
            kind,
            id.into(),
            "apple_health",
            "HKCategoryTypeIdentifierSleepAnalysis",
            "night".into(),
        );
        row.experience = Some("sleep".into());
        row.start = Some("2026-10-10 22:40:00 -0600".into());
        row.end = Some("2026-10-11 06:10:00 -0600".into());
        row.by = Some("Synthetic Watch".into());
        row.of = of.map(str::to_owned);
        row
    }

    fn logged(id: &str, at: &str, label: &str, words: &[&str]) -> BodyRow {
        let mut row = BodyRow::new(
            RowKind::Logged,
            id.into(),
            "apple_health",
            "HKQuantityTypeIdentifierDietaryEnergyConsumed",
            label.into(),
        );
        row.start = Some(at.into());
        row.value = Some("312".into());
        row.words = words.iter().map(|word| (*word).to_owned()).collect();
        row
    }

    fn window(root: &Path, day: &str, key: &str, rows: &[BodyRow]) -> PathBuf {
        window_in(root, STREAM, day, key, rows)
    }

    fn window_in(root: &Path, stream: &str, day: &str, key: &str, rows: &[BodyRow]) -> PathBuf {
        let dir = root.join("chronicle").join(day).join(stream).join(key);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(BODY_RECORDS_FILE), encode_rows(rows)).unwrap();
        // The real writer leaves a lock beside the window and one inside it.
        fs::write(dir.with_extension("lock"), "").unwrap();
        fs::write(dir.join("body.jsonl.lock"), "").unwrap();
        bump_body_marker(root, day).unwrap();
        dir
    }

    fn document(root: &Path, day: &str) -> Option<String> {
        fs::read_to_string(document_path(root, day)).ok()
    }

    #[test]
    fn a_row_an_archive_merge_left_a_second_copy_of_counts_once_and_reads_the_live_copy() {
        let journal = tempfile::tempdir().unwrap();
        let root = journal.path();
        // The target's frozen copy, and the merged copy the importer now writes to.
        window(
            root,
            "20261010",
            "090000_3600",
            &[reading("r1", "2026-10-10 09:05:00 -0600", "60")],
        );
        window(
            root,
            "20261010",
            "090000_3601",
            &[
                reading("r1", "2026-10-10 09:05:00 -0600", "61"),
                reading("r2", "2026-10-10 09:10:00 -0600", "62"),
            ],
        );
        let rows = read_rows(root, "20261010").unwrap();
        let rows = &rows["apple_health"];
        assert_eq!(rows.len(), 2, "{rows:?}");
        let first = rows.iter().find(|row| row.id == "r1").unwrap();
        assert_eq!(first.value.as_deref(), Some("61"));
    }

    #[test]
    fn a_day_reads_on_the_clock_that_placed_it_whatever_offset_a_row_states() {
        let journal = tempfile::tempdir().unwrap();
        let root = journal.path();
        fs::create_dir_all(root.join("imports")).unwrap();
        fs::write(
            root.join("imports/.apple_health-zone.json"),
            r#"{"zone":"America/Denver"}"#,
        )
        .unwrap();
        // Written at -0400: 21:30 and 22:30 in Denver, the second one's own clock on
        // the next day.
        window(
            root,
            "20261010",
            "210000_3600",
            &[
                reading("a", "2026-10-10 23:30:00 -0400", "70"),
                reading("b", "2026-10-11 00:30:00 -0400", "71"),
            ],
        );
        let mut days = BodyDays::default();
        days.reconcile(root);
        let text = document(root, "20261010").unwrap();
        assert!(text.contains("21:30 to 22:30"), "{text}");
    }

    #[test]
    fn the_document_says_what_was_recorded_and_no_reading_value() {
        let journal = tempfile::tempdir().unwrap();
        let root = journal.path();
        window(
            root,
            "20261010",
            "063000_300",
            &[
                reading("a", "2026-10-10 06:31:10 -0600", "4817"),
                logged(
                    "c",
                    "2026-10-10 06:32:00 -0600",
                    "dietary energy consumed",
                    &["Oatmeal"],
                ),
            ],
        );
        window(
            root,
            "20261010",
            "224000_300",
            &[night(RowKind::Experience, "n", None)],
        );
        let mut days = BodyDays::default();
        days.reconcile(root);
        let text = document(root, "20261010").unwrap();
        assert!(text.contains("Apple Health · imported"), "{text}");
        assert!(text.contains("heart rate"));
        assert!(text.contains("Synthetic Watch"));
        assert!(text.contains("Oatmeal"));
        assert!(text.contains("sleep: night, 22:40 to 2026-10-11 06:10"));
        assert!(!text.contains("4817"));
        assert!(!text.contains("312"));
        assert!(record_path(root, "20261010").is_file());
    }

    #[test]
    fn an_entry_is_the_owners_only_where_the_source_says_so() {
        let journal = tempfile::tempdir().unwrap();
        let root = journal.path();
        let mut watch = logged("w", "2026-10-10 08:00:00 -0600", "handwashing event", &[]);
        watch.by = Some("Synthetic Watch".into());
        let mut typed = logged("t", "2026-10-10 09:00:00 -0600", "dietary water", &[]);
        typed.by = Some("Health".into());
        typed.fields.insert(
            "metadata".into(),
            serde_json::json!({"HKWasUserEntered": "1"}),
        );
        window(root, "20261010", "080000_300", &[watch, typed]);
        let mut days = BodyDays::default();
        days.reconcile(root);
        let text = document(root, "20261010").unwrap();
        assert!(
            text.contains("- handwashing event: 1 entry, 08:00 (recorded by Synthetic Watch)"),
            "{text}"
        );
        assert!(
            text.contains("- dietary water: 1 entry, 09:00 (entered by the owner in Health)"),
            "{text}"
        );
        assert_eq!(text.matches("the owner").count(), 1, "{text}");
        assert!(
            text.find("dietary water") < text.find("handwashing"),
            "grouped by who recorded them"
        );
    }

    #[test]
    fn it_is_deterministic_and_follows_the_segments_that_survive() {
        let journal = tempfile::tempdir().unwrap();
        let root = journal.path();
        let first = window(
            root,
            "20261010",
            "063000_300",
            &[reading("a", "2026-10-10 06:31:10 -0600", "70")],
        );
        let second = window(
            root,
            "20261010",
            "120000_300",
            &[reading("b", "2026-10-10 12:01:00 -0600", "71")],
        );
        let mut days = BodyDays::default();
        days.reconcile(root);
        let built = document(root, "20261010").unwrap();
        assert!(built.contains("readings 2, 06:31 to 12:01"), "{built}");

        // A fresh reconciler over the same state writes the same bytes.
        fs::remove_file(document_path(root, "20261010")).unwrap();
        BodyDays::default().reconcile(root);
        assert_eq!(document(root, "20261010").unwrap(), built);

        fs::remove_dir_all(&first).unwrap();
        bump_stream_marker(root, "20261010").unwrap();
        days.reconcile(root);
        assert!(
            document(root, "20261010")
                .unwrap()
                .contains("1 reading, 12:01")
        );

        fs::remove_dir_all(&second).unwrap();
        bump_stream_marker(root, "20261010").unwrap();
        days.reconcile(root);
        assert!(document(root, "20261010").is_none());
        assert!(!record_path(root, "20261010").exists());
    }

    #[test]
    fn a_hidden_day_takes_its_night_whole_and_the_next_day_keeps_its_own() {
        let journal = tempfile::tempdir().unwrap();
        let root = journal.path();
        window(
            root,
            "20261010",
            "224000_300",
            &[night(RowKind::Experience, "n", None)],
        );
        let mut tile = night(RowKind::ExperienceTile, "n-1", Some("n"));
        tile.index = Some(1);
        tile.count = Some(90);
        window(root, "20261011", "000000_300", &[tile]);
        window(
            root,
            "20261011",
            "120000_3600",
            &[reading("r", "2026-10-11 12:01:00 -0600", "70")],
        );
        let mut days = BodyDays::default();
        days.reconcile(root);
        let next = document(root, "20261011").unwrap();
        assert!(next.contains("sleep continuing: night"), "{next}");

        // Hiding the day the night starts: that day has no document, and the
        // next day loses the night's later part and keeps its own reading.
        let hide = |day: &str, hidden: bool| {
            crate::body_hidden::set_body_days_hidden(
                root,
                &BTreeSet::from([day.to_owned()]),
                hidden,
            )
            .unwrap();
        };
        hide("20261010", true);
        days.reconcile(root);
        assert!(document(root, "20261010").is_none());
        let after = document(root, "20261011").unwrap();
        assert!(!after.contains("continuing"), "{after}");
        assert!(after.contains("12:01"), "{after}");

        // Hiding the next day instead leaves the night.
        hide("20261010", false);
        hide("20261011", true);
        days.reconcile(root);
        assert!(document(root, "20261011").is_none());
        assert!(document(root, "20261010").unwrap().contains("night"));

        // Unhiding brings each back as it was.
        hide("20261011", false);
        days.reconcile(root);
        assert_eq!(document(root, "20261011").unwrap(), next);
    }

    #[test]
    fn a_crossing_experience_reads_from_each_days_own_tiles() {
        let journal = tempfile::tempdir().unwrap();
        let root = journal.path();
        let first = window(
            root,
            "20261010",
            "224000_300",
            &[night(RowKind::Experience, "n", None)],
        );
        let mut tile = night(RowKind::ExperienceTile, "n-1", Some("n"));
        tile.index = Some(1);
        tile.count = Some(90);
        window(root, "20261011", "000000_300", &[tile]);
        let mut days = BodyDays::default();
        days.reconcile(root);
        let next = document(root, "20261011").unwrap();
        assert!(next.contains("sleep continuing: night"), "{next}");

        // A window mid-removal no longer counts.
        let staged = root
            .join("chronicle/20261011")
            .join(STREAM)
            .join(".removing_120000_300");
        fs::create_dir_all(&staged).unwrap();
        fs::write(
            staged.join(BODY_RECORDS_FILE),
            encode_rows(&[reading("gone", "2026-10-11 12:01:00 -0600", "70")]),
        )
        .unwrap();
        bump_stream_marker(root, "20261011").unwrap();
        days.reconcile(root);
        assert_eq!(document(root, "20261011").unwrap(), next);

        // The first tile deleted leaves the next day's document as it was, and true.
        fs::remove_dir_all(&first).unwrap();
        bump_stream_marker(root, "20261010").unwrap();
        days.reconcile(root);
        assert_eq!(document(root, "20261011").unwrap(), next);
        assert!(document(root, "20261010").is_none());
    }

    const OURA: &str = "import.oura";

    /// A row as Oura's reader writes it: `by` Oura, the whole document in `fields`.
    fn oura_row(
        kind: RowKind,
        id: &str,
        record_type: &str,
        label: &str,
        document: serde_json::Value,
    ) -> BodyRow {
        let mut row = BodyRow::new(kind, id.into(), "oura", record_type, label.into());
        row.by = Some("Oura".into());
        row.fields = document
            .as_object()
            .unwrap()
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        row
    }

    /// Every daily document Oura sends, with each score and assessment it holds.
    fn oura_daily_documents() -> Vec<BodyRow> {
        use serde_json::json;
        [
            (
                "daily_readiness",
                "daily readiness",
                json!({"score": 82, "temperature_deviation": -0.31,
                "contributors": {"hrv_balance": 63, "resting_heart_rate": 91}}),
            ),
            (
                "daily_sleep",
                "daily sleep",
                json!({"score": 74, "contributors": {"deep_sleep": 63}}),
            ),
            (
                "daily_activity",
                "daily activity",
                json!({"score": 91, "class_5_min": "0012345",
                "contributors": {"stay_active": 63}, "inactivity_alerts": 3}),
            ),
            (
                "daily_stress",
                "daily stress",
                json!({"day_summary": "stressful",
                "stress_high": 5400, "recovery_high": 1800}),
            ),
            (
                "daily_resilience",
                "daily resilience",
                json!({"level": "limited",
                "contributors": {"sleep_recovery": 41.2, "stress": 66.0}}),
            ),
            (
                "daily_spo2",
                "daily SpO2",
                json!({"spo2_percentage": {"average": 96.5},
                "breathing_disturbance_index": 23}),
            ),
            (
                "daily_cardiovascular_age",
                "cardiovascular age",
                json!({"vascular_age": 47}),
            ),
            ("vO2_max", "VO2 max", json!({"vo2_max": 39})),
        ]
        .into_iter()
        .map(|(endpoint, label, mut document)| {
            document["id"] = json!(format!("synthetic-{endpoint}"));
            document["day"] = json!("2026-10-10");
            let mut row = oura_row(RowKind::Dated, endpoint, endpoint, label, document);
            row.date = Some("2026-10-10".into());
            row
        })
        .collect()
    }

    /// Oura's daily documents are named on one line for their date, and nothing
    /// they or a session or workout judge reaches the day: no score, no level, no
    /// summary, no mood and no intensity. Tags are the owner's own entries.
    #[test]
    fn oura_documents_are_named_for_their_date_and_nothing_oura_judges_reaches_the_day() {
        use serde_json::json;
        let journal = tempfile::tempdir().unwrap();
        let root = journal.path();
        fs::create_dir_all(root.join("imports")).unwrap();
        fs::write(
            root.join("imports/.oura-zone.json"),
            r#"{"zone":"America/Denver"}"#,
        )
        .unwrap();
        window_in(
            root,
            OURA,
            "20261010",
            "000000_86400",
            &oura_daily_documents(),
        );
        let mut session = oura_row(
            RowKind::Experience,
            "session",
            "meditation",
            "meditation session",
            json!({"id": "synthetic-session", "type": "meditation", "mood": "bad"}),
        );
        session.experience = Some("mindfulness".into());
        session.start = Some("2026-10-10T06:00:00.000-06:00".into());
        session.end = Some("2026-10-10T06:30:00.000-06:00".into());
        window_in(root, OURA, "20261010", "060000_300", &[session]);
        let mut workout = oura_row(
            RowKind::Experience,
            "workout",
            "running",
            "running",
            json!({"id": "synthetic-workout", "activity": "running", "intensity": "hard",
                "source": "autodetected", "calories": 410.0, "distance": 8020.0}),
        );
        workout.experience = Some("workout".into());
        workout.start = Some("2026-10-10T07:00:00.000-06:00".into());
        workout.end = Some("2026-10-10T07:50:00.000-06:00".into());
        workout.values.insert("distance".into(), "8020.0 m".into());
        workout.values.insert("energy".into(), "410.0 kcal".into());
        window_in(root, OURA, "20261010", "070000_300", &[workout]);
        let mut caffeine = oura_row(
            RowKind::Logged,
            "tag",
            "tag_generic_caffeine",
            "caffeine",
            json!({"id": "synthetic-tag", "tag_type_code": "tag_generic_caffeine",
                "comment": "second cup", "custom_name": null}),
        );
        caffeine.start = Some("2026-10-10T09:15:00-06:00".into());
        caffeine.words = vec!["second cup".into()];
        window_in(root, OURA, "20261010", "090000_3600", &[caffeine]);
        // Apple's dated row with a value, beside it, reads as it always has.
        let mut apple = BodyRow::new(
            RowKind::Dated,
            "apple-dated".into(),
            "apple_health",
            "HKDataTypeSleepDurationGoal",
            "sleep duration goal".into(),
        );
        apple.date = Some("2026-10-10".into());
        apple.value = Some("8 hr".into());
        window(root, "20261010", "000000_86400", &[apple]);

        BodyDays::default().reconcile(root);
        let text = document(root, "20261010").unwrap();

        let (apple, oura) = text
            .split_once("## Oura · imported")
            .unwrap_or_else(|| panic!("{text}"));
        // Every daily document is named, on one line of Oura's own section.
        let named: Vec<&str> = oura
            .lines()
            .filter(|line| line.starts_with("- Oura's daily documents for this date: "))
            .collect();
        assert_eq!(named.len(), 1, "{text}");
        for label in oura_daily_documents().iter().map(|row| row.label.clone()) {
            assert!(named[0].contains(&label), "{label}: {text}");
        }
        assert!(
            apple.ends_with(
                "## Apple Health · imported\n\n- for this date: sleep duration goal 8 hr\n\n"
            ),
            "{text}"
        );
        let tag = oura
            .lines()
            .find(|line| line.starts_with("- caffeine:"))
            .unwrap_or_else(|| panic!("{text}"));
        assert!(tag.contains("entered by the owner in Oura"), "{text}");
        // The tag's note is the owner's, and says so.
        assert!(
            oura.contains("- in the owner's words: second cup"),
            "{text}"
        );
        assert!(!oura.contains("source's words"), "{text}");
        assert!(
            oura.contains("- workout: running, 07:00 to 07:50 (Oura)"),
            "{text}"
        );
        for judged in [
            "stressful",
            "limited",
            "bad",
            "hard",
            "intensity",
            "mood",
            "score",
            "level",
            "summary",
            "contributor",
        ] {
            assert!(!text.contains(judged), "{judged}: {text}");
        }
        for number in [
            "82", "74", "91", "63", "0.31", "5400", "1800", "41.2", "66", "96.5", "23", "47", "39",
            "8020", "410",
        ] {
            assert!(!text.contains(number), "{number}: {text}");
        }
    }

    fn moved(id: &str, of: &str) -> BodyRow {
        let mut row = BodyRow::new(
            RowKind::Moved,
            id.into(),
            "oura",
            "running",
            "running".into(),
        );
        row.of = Some(of.into());
        row
    }

    /// A day whose Oura rows have all moved says nothing of Oura, and a moved row
    /// beside live ones never reads as one of them.
    #[test]
    fn moved_rows_say_nothing_and_a_day_of_only_them_has_no_heading_for_their_source() {
        let journal = tempfile::tempdir().unwrap();
        let root = journal.path();
        window_in(root, OURA, "20261010", "070000_300", &[moved("w", "w")]);
        window_in(
            root,
            OURA,
            "20261010",
            "080000_3600",
            &[moved("w@20261010/080000_3600", "w")],
        );
        window(
            root,
            "20261010",
            "063000_300",
            &[reading("a", "2026-10-10 06:31:10 -0600", "70")],
        );
        BodyDays::default().reconcile(root);
        let text = document(root, "20261010").unwrap();
        assert!(!text.contains("Oura"), "{text}");
        assert!(text.contains("## Apple Health · imported"), "{text}");

        // With a live row of Oura's beside the marks, only it reads.
        let mut walk = oura_row(
            RowKind::Experience,
            "walk",
            "walking",
            "walking",
            serde_json::json!({"id": "synthetic-walk"}),
        );
        walk.experience = Some("workout".into());
        walk.start = Some("2026-10-10T09:00:00.000-06:00".into());
        walk.end = Some("2026-10-10T09:20:00.000-06:00".into());
        window_in(root, OURA, "20261010", "090000_300", &[walk]);
        BodyDays::default().reconcile(root);
        let text = document(root, "20261010").unwrap();
        let (_, oura) = text
            .split_once("## Oura · imported")
            .unwrap_or_else(|| panic!("{text}"));
        assert_eq!(
            oura.trim(),
            "- workout: walking, 09:00 to 09:20 (Oura)",
            "{text}"
        );
        assert!(!text.contains("running"), "{text}");
    }

    #[test]
    fn a_day_with_no_body_stream_writes_nothing() {
        let journal = tempfile::tempdir().unwrap();
        let root = journal.path();
        fs::create_dir_all(root.join("chronicle/20261010/phone/090000_300")).unwrap();
        BodyDays::default().reconcile(root);
        assert!(document(root, "20261010").is_none());
        assert!(!record_path(root, "20261010").exists());
    }
}
