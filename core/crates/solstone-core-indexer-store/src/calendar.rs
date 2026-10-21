// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Calendar notes in the search index, and the listing derived from them.
//!
//! Each note is findable on the day it was stored, like any other segment
//! content: one stored-day row per note, filed under its file. The listing
//! adds, for every event, rows under each day the event happens, built from
//! the latest version received for its identity (`event`, `occurrence`). Those
//! rows are filed under the file that holds that latest note.
//!
//! Every write here happens inside the caller's index transaction, so a newer
//! note replaces the rows filed under an older note's file in one commit. Only
//! the single index writer reaches these functions; producers add work to the
//! indexer's to-do list. The listing is derived from stored notes alone, so a
//! rebuild yields the same listing.
//!
//! Timed events are listed in the journal's time zone as it is when they are
//! indexed. If the owner changes the zone, timed events keep their old days
//! until the search index is rebuilt.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use chrono::{NaiveDate, NaiveDateTime, Utc};
use chrono_tz::Tz;
use rusqlite::{Connection, params};
use solstone_core_format::calendar::{
    CalendarNote, When, import_stamp, listed_days, parse_calendar_note, read_calendar_notes,
    render_calendar_note, validate_import_calendar_file,
};
use solstone_core_format::segment::hour_bucket;

use crate::StoreError;
use crate::activity_provenance::{delete_activity_provenance, record_activity_membership};
use crate::chunk_sources::{delete_chunk_source_rowids, record_chunk_source};

pub(crate) const CREATE_CALENDAR_NOTES: &str = "\
CREATE TABLE IF NOT EXISTS calendar_notes(
path TEXT NOT NULL,
line INTEGER NOT NULL,
file_name TEXT NOT NULL,
stream TEXT,
event TEXT NOT NULL,
occurrence TEXT NOT NULL,
changed_at_ms INTEGER NOT NULL,
change_rank INTEGER NOT NULL,
changed_at TEXT NOT NULL,
record TEXT NOT NULL,
import TEXT,
imported_at_ms INTEGER,
PRIMARY KEY(path, line)
)";

/// Add the import columns to a table an earlier version created.
pub(crate) fn ensure_calendar_note_columns(conn: &Connection) -> Result<(), StoreError> {
    let columns = {
        let mut statement = conn.prepare("SELECT name FROM pragma_table_info('calendar_notes')")?;
        statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<BTreeSet<_>, _>>()?
    };
    for (column, kind) in [("import", "TEXT"), ("imported_at_ms", "INTEGER")] {
        if !columns.contains(column) {
            conn.execute(
                &format!("ALTER TABLE calendar_notes ADD COLUMN {column} {kind}"),
                [],
            )?;
        }
    }
    Ok(())
}

/// The stream every calendar import writes to.
pub const CALENDAR_IMPORT_STREAM: &str = "import.ics";

/// How far past an import its expansion of a repeating series reaches.
const IMPORT_EXPANSION_DAYS: i64 = 365;
pub(crate) const CREATE_CALENDAR_NOTES_IDENTITY: &str =
    "CREATE INDEX IF NOT EXISTS calendar_notes_identity ON calendar_notes(event, occurrence)";
pub(crate) const CREATE_CALENDAR_NOTES_STREAM: &str =
    "CREATE INDEX IF NOT EXISTS calendar_notes_stream ON calendar_notes(stream)";
/// One row per calendar chunk, keyed by the chunk's row id: which version the
/// hit is and when that version was recorded.
pub(crate) const CREATE_CALENDAR_HITS: &str = "\
CREATE TABLE IF NOT EXISTS calendar_hits(
rowid INTEGER PRIMARY KEY,
path TEXT NOT NULL,
idx INTEGER NOT NULL,
version TEXT NOT NULL CHECK (version IN ('latest', 'recorded')),
event TEXT NOT NULL,
occurrence TEXT NOT NULL,
changed_at TEXT NOT NULL,
continues INTEGER NOT NULL CHECK (continues IN (0, 1))
)";
pub(crate) const CREATE_CALENDAR_HITS_PATH: &str =
    "CREATE INDEX IF NOT EXISTS calendar_hits_path ON calendar_hits(path)";
pub(crate) const CREATE_CALENDAR_HITS_IDENTITY: &str =
    "CREATE INDEX IF NOT EXISTS calendar_hits_identity ON calendar_hits(event, occurrence)";

/// Listing rows take chunk positions above every stored-day row of their file,
/// so each hit has its own position for search paging. A note's listing day
/// `n` sits at `LISTING_IDX_BASE + line * LISTING_IDX_STRIDE + n`.
pub const LISTING_IDX_BASE: i64 = 1_000_000_000;
pub(crate) const LISTING_IDX_STRIDE: i64 = 1_000;

/// The agent every calendar chunk is indexed under.
pub const CALENDAR_AGENT: &str = "calendar";

pub(crate) type Identity = (String, String);

/// Index one calendar file's notes as stored-day rows, returning the
/// identities it holds. The caller relists them in the same transaction.
///
/// Calendar notes are read only from calendar streams; a calendar-named file
/// anywhere else adds nothing.
pub(crate) fn index_calendar_file(
    conn: &Connection,
    journal: &Path,
    rel: &str,
    text: &str,
    day: &str,
    stream: Option<&str>,
    bucket: &str,
) -> Result<(BTreeSet<Identity>, Vec<String>), StoreError> {
    let mut identities = BTreeSet::new();
    if stream == Some(CALENDAR_IMPORT_STREAM) {
        return index_imported_calendar_file(conn, rel, text);
    }
    let Some(stream) =
        stream.filter(|stream| solstone_core_segment::is_calendar_stream(journal, stream))
    else {
        return Ok((
            identities,
            vec![format!(
                "calendar notes are read only from calendar streams and calendar imports: {rel}"
            )],
        ));
    };
    let file_name = rel.rsplit('/').next().unwrap_or(rel);
    let (notes, warnings) = read_calendar_notes(text);
    for (line, note) in notes {
        let line = line as i64;
        let rowid = insert_chunk(
            conn,
            &render_calendar_note(&note),
            rel,
            day,
            stream,
            line,
            bucket,
        )?;
        record_hit(conn, rowid, rel, line, "recorded", &note, false)?;
        conn.execute(
            "INSERT INTO calendar_notes(path, line, file_name, stream, event, occurrence, changed_at_ms, change_rank, changed_at, record) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                rel,
                line,
                file_name,
                stream,
                note.event,
                note.occurrence,
                note.changed_at_ms(),
                note.change.rank(),
                note.changed_at_text(),
                serde_json::Value::Object(note.record.clone()).to_string(),
            ],
        )?;
        identities.insert((note.event.clone(), note.occurrence.clone()));
    }
    Ok((identities, warnings))
}

/// The imports whose surviving lines hold one event, newest first by upload
/// time with the import id breaking ties, each with what it covers.
///
/// Each import defines the event across what it covers, for itself and every
/// older import: an import's line counts only where no newer import covers
/// its date.
struct EventImports {
    newest_first: Vec<ImportCover>,
}

/// What one import covers of an event.
struct ImportCover {
    import: String,
    /// It holds a line for the event as a single, non-repeating event.
    single: bool,
    /// The earliest and latest instants it covers, for a series: its earliest
    /// and latest written occurrences. When it expanded the series' rule, it
    /// covers up to a year past the import, as far as an import expands one,
    /// even when its last written occurrence is earlier.
    span: Option<(NaiveDateTime, NaiveDateTime)>,
}

impl ImportCover {
    fn covers(&self, occurrence: &str) -> bool {
        if occurrence.is_empty() {
            return self.single;
        }
        match (self.span, occurrence_instant(occurrence)) {
            (Some((first, last)), Some(at)) => first <= at && at <= last,
            _ => false,
        }
    }
}

impl EventImports {
    /// Whether a line from `import` for `occurrence` still counts. Lines that
    /// did not come from an import always count; an import's count unless a
    /// newer import covers their date.
    fn admits(&self, import: Option<&str>, occurrence: &str) -> bool {
        let Some(import) = import else {
            return true;
        };
        self.newest_first
            .iter()
            .take_while(|cover| cover.import != import)
            .all(|cover| !cover.covers(occurrence))
    }
}

fn event_imports(conn: &Connection, event: &str) -> Result<EventImports, StoreError> {
    let rows = {
        let mut statement = conn.prepare_cached(
            "SELECT import, imported_at_ms, occurrence, json_extract(record, '$.expanded') IS 1 \
             FROM calendar_notes WHERE event=? AND import IS NOT NULL",
        )?;
        statement
            .query_map([event], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, bool>(3)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?
    };
    // Per import: its upload time, whether it expanded a rule, and its dates.
    let mut imports: BTreeMap<String, (i64, bool, Vec<String>)> = BTreeMap::new();
    for (import, imported_at_ms, occurrence, expanded) in rows {
        let entry = imports
            .entry(import)
            .or_insert_with(|| (imported_at_ms, false, Vec::new()));
        entry.0 = entry.0.max(imported_at_ms);
        entry.1 |= expanded;
        entry.2.push(occurrence);
    }
    let mut newest_first = imports
        .into_iter()
        .map(|(import, (imported_at_ms, expanded, occurrences))| {
            let single = occurrences.iter().any(String::is_empty);
            let instants = occurrences
                .iter()
                .filter_map(|occurrence| occurrence_instant(occurrence))
                .collect::<Vec<_>>();
            let span = match (instants.iter().min(), instants.iter().max()) {
                (Some(&first), Some(&last)) => {
                    let expanded_to = chrono::DateTime::from_timestamp_millis(imported_at_ms)
                        .map(|at| (at + chrono::Duration::days(IMPORT_EXPANSION_DAYS)).naive_utc());
                    let end = expanded_to
                        .filter(|_| expanded)
                        .map_or(last, |end| end.max(last));
                    Some((first, end))
                }
                _ => None,
            };
            (
                imported_at_ms,
                ImportCover {
                    import,
                    single,
                    span,
                },
            )
        })
        .collect::<Vec<_>>();
    newest_first.sort_by(|left, right| {
        right
            .0
            .cmp(&left.0)
            .then_with(|| right.1.import.cmp(&left.1.import))
    });
    Ok(EventImports {
        newest_first: newest_first.into_iter().map(|(_, cover)| cover).collect(),
    })
}

/// A normalized occurrence as a point in time: the UTC instant of a zoned
/// start, the wall clock of a floating one, and the start of an all-day date.
fn occurrence_instant(occurrence: &str) -> Option<NaiveDateTime> {
    if let Some(utc) = occurrence.strip_suffix('Z') {
        return NaiveDateTime::parse_from_str(utc, "%Y-%m-%dT%H:%M:%S%.f").ok();
    }
    NaiveDateTime::parse_from_str(occurrence, "%Y-%m-%dT%H:%M:%S%.f")
        .ok()
        .or_else(|| {
            NaiveDate::parse_from_str(occurrence, "%Y-%m-%d")
                .ok()
                .and_then(|date| date.and_hms_opt(0, 0, 0))
        })
}

/// Index a calendar file an import wrote beside a window's transcript. Its
/// notes join the listing only: the transcript stays the record of the day
/// the event was placed on. A file that fails the contract, checked against
/// its segment's own span, is skipped and reported.
fn index_imported_calendar_file(
    conn: &Connection,
    rel: &str,
    text: &str,
) -> Result<(BTreeSet<Identity>, Vec<String>), StoreError> {
    let parts = rel
        .strip_prefix("chronicle/")
        .unwrap_or(rel)
        .split('/')
        .collect::<Vec<_>>();
    let [day, _, segment, file_name] = parts.as_slice() else {
        return Ok((
            BTreeSet::new(),
            vec![format!("calendar notes skipped: {rel} is not in a segment")],
        ));
    };
    let notes = match validate_import_calendar_file(file_name, text.as_bytes(), day, segment) {
        Ok(notes) => notes,
        Err(error) => {
            return Ok((
                BTreeSet::new(),
                vec![format!("calendar notes in {rel} skipped: {error}")],
            ));
        }
    };
    let mut events = BTreeSet::new();
    for (line, note) in notes.iter().enumerate() {
        let (import, imported_at) =
            import_stamp(&note.record).expect("validated import lines carry their import");
        conn.execute(
            "INSERT INTO calendar_notes(path, line, file_name, stream, event, occurrence, changed_at_ms, change_rank, changed_at, record, import, imported_at_ms) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                rel,
                line as i64,
                file_name,
                CALENDAR_IMPORT_STREAM,
                note.event,
                note.occurrence,
                note.changed_at_ms(),
                note.change.rank(),
                note.changed_at_text(),
                serde_json::Value::Object(note.record.clone()).to_string(),
                import,
                imported_at.timestamp_millis(),
            ],
        )?;
        events.insert(note.event.clone());
    }
    Ok((identities_of_events(conn, &events)?, Vec::new()))
}

/// Every identity of these events the index holds a note or a listing for.
/// Which import's lines count for one date of a series depends on the
/// series' other dates, so an import's change relists them all.
fn identities_of_events(
    conn: &Connection,
    events: &BTreeSet<String>,
) -> Result<BTreeSet<Identity>, StoreError> {
    let mut identities = BTreeSet::new();
    let mut statement = conn.prepare_cached(
        "SELECT event, occurrence FROM calendar_notes WHERE event=? \
         UNION SELECT event, occurrence FROM calendar_hits WHERE event=? AND version='latest'",
    )?;
    for event in events {
        let rows = statement.query_map(params![event, event], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            identities.insert(row?);
        }
    }
    Ok(identities)
}

fn insert_chunk(
    conn: &Connection,
    content: &str,
    path: &str,
    day: &str,
    stream: &str,
    idx: i64,
    bucket: &str,
) -> Result<i64, StoreError> {
    conn.execute(
        "INSERT INTO chunks(content, path, day, facet, agent, stream, idx, time_bucket) VALUES (?, ?, ?, '', ?, ?, ?, ?)",
        params![content, path, day, CALENDAR_AGENT, stream, idx, bucket],
    )?;
    let rowid = conn.last_insert_rowid();
    record_activity_membership(conn, rowid, false)?;
    record_chunk_source(conn, Some(rowid), Some(path), Some(stream))?;
    Ok(rowid)
}

fn record_hit(
    conn: &Connection,
    rowid: i64,
    path: &str,
    idx: i64,
    version: &str,
    note: &CalendarNote,
    continues: bool,
) -> Result<(), StoreError> {
    conn.execute(
        "INSERT OR REPLACE INTO calendar_hits(rowid, path, idx, version, event, occurrence, changed_at, continues) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        params![
            rowid,
            path,
            idx,
            version,
            note.event,
            note.occurrence,
            note.changed_at_text(),
            i64::from(continues),
        ],
    )?;
    Ok(())
}

/// Delete chunks by row id, with everything keyed to them.
fn delete_chunk_rows(conn: &Connection, rows: &[(i64, String)]) -> Result<(), StoreError> {
    let mut delete = conn.prepare_cached("DELETE FROM chunks WHERE rowid=? AND path=?")?;
    for (rowid, path) in rows {
        delete.execute(params![rowid, path])?;
    }
    let rowids = rows.iter().map(|(rowid, _)| *rowid).collect::<Vec<_>>();
    delete_activity_provenance(conn, &rowids)?;
    delete_chunk_source_rowids(conn, &rowids)?;
    let mut forget = conn.prepare_cached("DELETE FROM calendar_hits WHERE rowid=?")?;
    for rowid in &rowids {
        forget.execute([rowid])?;
    }
    Ok(())
}

/// Remove everything filed under one calendar file: its stored-day rows, any
/// listing rows it held, and its notes. Returns the identities it had notes
/// for, which the caller relists.
pub(crate) fn forget_calendar_path(
    conn: &Connection,
    rel: &str,
) -> Result<BTreeSet<Identity>, StoreError> {
    let identities = {
        let mut statement = conn
            .prepare_cached("SELECT DISTINCT event, occurrence FROM calendar_notes WHERE path=?")?;
        statement
            .query_map([rel], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<BTreeSet<Identity>, _>>()?
    };
    let rows = {
        let mut statement =
            conn.prepare_cached("SELECT rowid, path FROM calendar_hits WHERE path=?")?;
        statement
            .query_map([rel], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<Vec<(i64, String)>, _>>()?
    };
    let imported_events = {
        let mut statement = conn.prepare_cached(
            "SELECT DISTINCT event FROM calendar_notes WHERE path=? AND import IS NOT NULL",
        )?;
        statement
            .query_map([rel], |row| row.get(0))?
            .collect::<Result<BTreeSet<String>, _>>()?
    };
    delete_chunk_rows(conn, &rows)?;
    conn.execute("DELETE FROM calendar_notes WHERE path=?", [rel])?;
    let mut identities = identities;
    identities.extend(identities_of_events(conn, &imported_events)?);
    Ok(identities)
}

/// Every calendar file with notes in one stream.
pub(crate) fn calendar_paths_in_stream(
    conn: &Connection,
    stream: &str,
) -> Result<Vec<String>, StoreError> {
    let mut statement =
        conn.prepare("SELECT DISTINCT path FROM calendar_notes WHERE stream=? ORDER BY path")?;
    Ok(statement
        .query_map([stream], |row| row.get(0))?
        .collect::<Result<Vec<_>, _>>()?)
}

/// The journal zone timed events are listed in: the configured zone, or the
/// journal's usual fallback when none is configured. Only a settings file
/// that cannot be read is an error.
pub(crate) fn listing_zone(journal: &Path) -> Result<Tz, StoreError> {
    Ok(solstone_core_indexer::edges::owner_timezone_for_journal(
        journal,
    )?)
}

/// What a relist leaves for its caller.
#[derive(Debug, Default)]
pub(crate) struct Relisted {
    pub(crate) warnings: Vec<String>,
    /// Files whose latest note could not be listed yet. The caller marks them
    /// for the next scan after its own `files` writes, so nothing overwrites
    /// the mark.
    pub(crate) reindex: BTreeSet<String>,
}

impl Relisted {
    /// Mark each file left for the next scan to index again.
    pub(crate) fn mark_for_reindex(&self, conn: &Connection) -> Result<(), StoreError> {
        for path in &self.reindex {
            conn.execute("UPDATE files SET mtime=-1 WHERE path=?", [path])?;
        }
        Ok(())
    }
}

/// One listing row an identity should have: position, day and bucket.
type WantedRow = (i64, String, String);

/// Rebuild the listing rows of each identity from its latest surviving note.
///
/// The latest note wins by `changed_at` as an instant; equal instants break
/// removed over changed over added, then by file name. A removal, or no
/// surviving note, lists nothing. Rows already listed from the winning note
/// stay as they are, keeping their row ids. The new rows are worked out before
/// anything is removed: when a timed event cannot be listed because the
/// settings file is unreadable, the rows it has keep standing, its file is
/// left for the next scan to index again, and a warning says so.
pub(crate) fn relist(
    conn: &Connection,
    journal: &Path,
    identities: &BTreeSet<Identity>,
) -> Result<Relisted, StoreError> {
    let mut relisted = Relisted::default();
    let mut zone: Option<Result<Tz, String>> = None;
    let mut event_covers: BTreeMap<String, EventImports> = BTreeMap::new();
    for (event, occurrence) in identities {
        if !event_covers.contains_key(event) {
            event_covers.insert(event.clone(), event_imports(conn, event)?);
        }
        let covers = &event_covers[event];
        let winner: Option<(String, i64, String, Option<String>)> = {
            let mut statement = conn.prepare_cached(
                "SELECT path, line, record, stream, import FROM calendar_notes WHERE event=? AND occurrence=? \
                 ORDER BY changed_at_ms DESC, change_rank DESC, file_name DESC, path DESC, line DESC",
            )?;
            let mut rows = statement.query(params![event, occurrence])?;
            let mut winner = None;
            while let Some(row) = rows.next()? {
                let import: Option<String> = row.get(4)?;
                if !covers.admits(import.as_deref(), occurrence) {
                    continue;
                }
                winner = Some((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?));
                break;
            }
            winner
        };
        // The rows the winner should have, worked out before any change.
        let mut wanted: Option<(String, String, CalendarNote, Vec<WantedRow>, bool)> = None;
        if let Some((path, line, record, stream)) = winner {
            match parse_calendar_note(&record) {
                Err(reason) => relisted
                    .warnings
                    .push(format!("calendar note in {path} not listed: {reason}")),
                Ok(note) => {
                    if let Some(schedule) = note.schedule {
                        let listing = if matches!(schedule.start, When::Date(_)) {
                            Some(listed_days(&schedule, &Utc))
                        } else {
                            match zone.get_or_insert_with(|| {
                                listing_zone(journal).map_err(|error| error.to_string())
                            }) {
                                Ok(zone) => Some(listed_days(&schedule, zone)),
                                Err(error) => {
                                    relisted.warnings.push(format!(
                                        "calendar event in {path} is listed once the settings file can be read: {error}"
                                    ));
                                    relisted.reindex.insert(path.clone());
                                    None
                                }
                            }
                        };
                        let Some(listing) = listing else {
                            // Leave whatever is listed now in place.
                            continue;
                        };
                        let rows = listing
                            .days
                            .iter()
                            .enumerate()
                            .map(|(offset, day)| {
                                let bucket = match listing.start_hour {
                                    Some(hour) if offset == 0 => hour_bucket(hour),
                                    _ => "",
                                };
                                (
                                    LISTING_IDX_BASE + line * LISTING_IDX_STRIDE + offset as i64,
                                    day.format("%Y%m%d").to_string(),
                                    bucket.to_owned(),
                                )
                            })
                            .collect();
                        wanted = Some((
                            path,
                            stream.unwrap_or_default(),
                            note,
                            rows,
                            listing.continues,
                        ));
                    }
                }
            }
        }
        let listed = {
            let mut statement = conn.prepare_cached(
                "SELECT h.rowid, h.path, h.idx, c.day, c.time_bucket FROM calendar_hits h \
                 LEFT JOIN chunks c ON c.rowid=h.rowid AND c.path=h.path \
                 WHERE h.version='latest' AND h.event=? AND h.occurrence=?",
            )?;
            statement
                .query_map(params![event, occurrence], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?
        };
        // Rows the winning note already owns are left alone.
        if let Some((path, _, _, rows, _)) = &wanted {
            let current = listed
                .iter()
                .filter(|row| &row.1 == path)
                .map(|row| {
                    (
                        row.2,
                        row.3.clone().unwrap_or_default(),
                        row.4.clone().unwrap_or_default(),
                    )
                })
                .collect::<BTreeSet<_>>();
            if listed.len() == rows.len() && current == rows.iter().cloned().collect() {
                continue;
            }
        }
        if listed.is_empty() && wanted.is_none() {
            continue;
        }
        let stale = listed
            .iter()
            .map(|row| (row.0, row.1.clone()))
            .collect::<Vec<_>>();
        delete_chunk_rows(conn, &stale)?;
        interruption_point()?;
        let Some((path, stream, note, rows, continues)) = wanted else {
            continue;
        };
        let content = render_calendar_note(&note);
        for (idx, day, bucket) in rows {
            let rowid = insert_chunk(conn, &content, &path, &day, &stream, idx, &bucket)?;
            record_hit(conn, rowid, &path, idx, "latest", &note, continues)?;
        }
    }
    crate::calendar_schedule::relink_people(conn, journal, identities)?;
    Ok(relisted)
}

#[cfg(test)]
thread_local! {
    static INTERRUPT_AFTER_FIRST_WRITE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Run `body` with a relist interrupted after its first write, before the
/// transaction can commit.
#[cfg(test)]
pub(crate) fn interrupted_after_first_write<T>(body: impl FnOnce() -> T) -> T {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            INTERRUPT_AFTER_FIRST_WRITE.with(|flag| flag.set(false));
        }
    }
    INTERRUPT_AFTER_FIRST_WRITE.with(|flag| flag.set(true));
    let _reset = Reset;
    body()
}

fn interruption_point() -> Result<(), StoreError> {
    #[cfg(test)]
    if INTERRUPT_AFTER_FIRST_WRITE.with(std::cell::Cell::get) {
        return Err(StoreError::Io(std::io::Error::other(
            "index write interrupted",
        )));
    }
    Ok(())
}

#[cfg(test)]
#[path = "calendar_tests.rs"]
mod tests;
