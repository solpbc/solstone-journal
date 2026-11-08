// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
#[cfg(test)]
use std::fs::OpenOptions;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use chrono::NaiveDate;
use getrandom::fill as random_fill;
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};
use serde_json::{Map, Value};
use solstone_core_body_source::{BodyRawRetention, BodySourceFamily};
use solstone_core_journal_io::{
    AtomicWriteOptions, FileLock, LockOptions, StagedDirOptions, create_directory_with_mode,
    hold_lock, publish_staged_dir, remove_dir_all, write_reader_exclusive,
};

use crate::approval::{apple_approval, pin_journal_target};
use crate::bounded_file::{
    open_descendant_regular as open_descendant_file, open_regular_file as open_nofollow,
};
use crate::bundle::{
    BodyIngestError, BodyIngestErrorKind, BodyIngestReport, MAX_RAW_ASSETS, MAX_RAW_PATH_BYTES,
    NormalizedInput, RawAsset, publish, sha256_hex,
};

const SCHEMA: &str = "solstone.health.apple_health.v1";
const MAX_APPLE_EXPORT_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const MAX_APPLE_SOURCE_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const MAX_APPLE_EVENT_BYTES: usize = 1024 * 1024;
const MAX_APPLE_XML_EVENTS_SCANNED: u64 = 50_000_000;
const UNREADABLE_ATTRIBUTES: &str = "attributes";
/// The scan path holds nothing per record, so its only whole-export bound is
/// bytes, and it is its own, well past any real history.
const MAX_APPLE_SCAN_BYTES: u64 = 64 * 1024 * 1024 * 1024;
/// One element's children (metadata, statistics, events) on the scan path. An
/// element past it is refused, so one element can't grow without limit.
pub const MAX_APPLE_SCAN_CHILDREN: usize = 10_000;
const MAX_APPLE_RECORDS_SCANNED: u64 = 50_000_000;
const MAX_APPLE_SELECTED_ROWS: usize = 100_000;
const MAX_APPLE_SELECTED_BYTES: usize = 128 * 1024 * 1024;
const MAX_APPLE_SOURCE_DIRECTORIES: usize = 1_024;
const MAX_APPLE_SOURCE_DEPTH: usize = 128;
const MAX_APPLE_ARCHIVE_ENTRIES: u16 = 10_000;
const MAX_APPLE_ARCHIVE_DIRECTORY_BYTES: u32 = 16 * 1024 * 1024;
const MAX_APPLE_IMPORT_ENTRIES_SCANNED: usize = 100_000;
const MAX_STALE_APPLE_SNAPSHOTS: usize = 1_024;
const ZIP_EOCD_MIN_BYTES: usize = 22;
const ZIP_MAX_COMMENT_BYTES: usize = u16::MAX as usize;
const ZIP64_LOCATOR_BYTES: usize = 20;
const ZIP64_END_BYTES: usize = 56;
/// An export whose `HealthData` never closes: an interrupted copy, or not an export.
pub const EXPORT_INCOMPLETE: &str = "export_incomplete";
/// What an owner reads for one.
pub const EXPORT_INCOMPLETE_TEXT: &str = "this export looks incomplete, so none of it was saved. export it from Apple Health again and import the new copy";
#[derive(Clone, Copy)]
struct SourceTraversalLimits {
    files: usize,
    directories: usize,
    depth: usize,
}

const SOURCE_TRAVERSAL_LIMITS: SourceTraversalLimits = SourceTraversalLimits {
    files: MAX_RAW_ASSETS,
    directories: MAX_APPLE_SOURCE_DIRECTORIES,
    depth: MAX_APPLE_SOURCE_DEPTH,
};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AppleImportOptions {
    pub date_from: Option<String>,
    pub date_to: Option<String>,
    pub confirm_body_save: bool,
    pub force: bool,
}

#[derive(Clone)]
struct Window {
    start: Option<String>,
    end: Option<String>,
}

impl Window {
    fn new(start: Option<&str>, end: Option<&str>) -> Result<Self, BodyIngestError> {
        let start = start.map(parse_cli_day).transpose()?;
        let end = end.map(parse_cli_day).transpose()?;
        if start
            .as_ref()
            .zip(end.as_ref())
            .is_some_and(|(start, end)| start > end)
        {
            return Err(source_error("date_window"));
        }
        Ok(Self { start, end })
    }

    fn includes(&self, day: &str) -> bool {
        self.start
            .as_ref()
            .is_none_or(|start| start.as_str() <= day)
            && self.end.as_ref().is_none_or(|end| day <= end.as_str())
    }

    fn suffix(&self) -> String {
        if self.start.is_none() && self.end.is_none() {
            String::new()
        } else {
            format!(
                "#window:{}:{}",
                self.start.as_deref().unwrap_or("open"),
                self.end.as_deref().unwrap_or("open")
            )
        }
    }
}

pub fn preview_apple(
    source: &Path,
    date_from: Option<&str>,
    date_to: Option<&str>,
) -> Result<BodyIngestReport, BodyIngestError> {
    let window = Window::new(date_from, date_to)?;
    let (rows, _) = parse_source(source, &window, None)?;
    let days = rows
        .iter()
        .filter_map(|row| {
            row.row
                .get("day")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    Ok(BodyIngestReport::preview(rows.len() as u64, days))
}

/// Detect a plausible Apple Health source without parsing its XML payload.
///
/// ZIP structure is bounded before `zip` constructs its central-directory
/// index, so the shipping Python dispatcher can classify archives without
/// opening them in Python first.
pub fn detect_apple_source(source: &Path) -> Result<bool, BodyIngestError> {
    let metadata = fs::symlink_metadata(source).map_err(|_| source_error("source"))?;
    if metadata.file_type().is_dir() {
        for candidate in ["export.xml", "apple_health_export/export.xml"] {
            match open_descendant_file(source, candidate) {
                Ok(_) => return Ok(true),
                Err(error) if missing_or_not_directory(&error) => {}
                Err(_) => return Err(source_error("source_symlink")),
            }
        }
        return Ok(false);
    }
    if !metadata.file_type().is_file() {
        return Err(source_error("source_symlink"));
    }
    let has_zip_extension = source
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("zip"));
    if !has_zip_extension {
        return Ok(false);
    }
    let mut file = open_nofollow(source).map_err(|_| source_error("archive"))?;
    preflight_zip(&mut file)?;
    let archive = zip::ZipArchive::new(file).map_err(|_| source_error("archive"))?;
    let (_archive, names) =
        crate::zip_names::prove_open_zip(archive).map_err(|_| source_error("archive"))?;
    Ok(names.iter().any(|name| is_export_member(name)))
}

pub fn save_apple(
    source: &Path,
    journal: &Path,
    options: &AppleImportOptions,
) -> Result<BodyIngestReport, BodyIngestError> {
    save_apple_before_lock(source, journal, options, &mut || {})
}

fn save_apple_before_lock(
    source: &Path,
    journal: &Path,
    options: &AppleImportOptions,
    before_lock: &mut dyn FnMut(),
) -> Result<BodyIngestReport, BodyIngestError> {
    let journal = pin_journal_target(journal)?;
    let journal = journal.as_path();
    apple_approval(journal, options.confirm_body_save)?;
    let imports = journal.join("imports");
    create_directory_with_mode(&imports, 0o700)
        .map_err(|_| BodyIngestError::new(BodyIngestErrorKind::Publication, "imports_directory"))?;
    before_lock();
    let _lock = hold_apple_ingest_lock(journal)?;
    let retention = apple_approval(journal, options.confirm_body_save)?;
    clean_stale_snapshots(journal)?;
    let window = Window::new(options.date_from.as_deref(), options.date_to.as_deref())?;
    let snapshot = AppleSnapshot::create(source, journal, retention)?;
    let raw_assets = raw_plan(snapshot.source(), retention)?;
    let raw_base = raw_reference_base(snapshot.source(), retention)?;
    let (rows, export) = parse_source(snapshot.source(), &window, raw_base.as_deref())?;
    if rows.is_empty() {
        return Ok(BodyIngestReport::preview(0, Vec::new()));
    }
    let digest = with_export(snapshot.source(), &export, |reader| sha256_hex(reader))?;
    publish(
        journal,
        BodySourceFamily::AppleHealth,
        format!("{digest}{}", window.suffix()),
        retention,
        rows,
        raw_assets,
        options.force,
    )
}

/// Exclusive flock on the Apple ingest sidecar. Hidden so ingest tests contend
/// on the same path and mode as production.
#[doc(hidden)]
pub fn hold_apple_ingest_lock(journal: &Path) -> Result<FileLock, BodyIngestError> {
    hold_lock(
        journal.join("imports/apple-body-ingest"),
        LockOptions {
            mode: Some(0o600),
            ..LockOptions::default()
        },
    )
    .map_err(|_| BodyIngestError::new(BodyIngestErrorKind::Publication, "apple_lock"))
}

#[expect(
    clippy::disallowed_methods,
    reason = "system internal: stale Apple Health snapshot staging, never kept owner data"
)]
fn clean_stale_snapshots(journal: &Path) -> Result<(), BodyIngestError> {
    let mut scanned = 0_usize;
    let mut removed = 0_usize;
    for entry in
        fs::read_dir(journal.join("imports")).map_err(|_| source_error("snapshot_cleanup"))?
    {
        scanned = scanned
            .checked_add(1)
            .filter(|count| *count <= MAX_APPLE_IMPORT_ENTRIES_SCANNED)
            .ok_or_else(|| source_error("imports_entry_limit"))?;
        let entry = entry.map_err(|_| source_error("snapshot_cleanup"))?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !is_apple_snapshot_residue(name) {
            continue;
        }
        if !entry
            .file_type()
            .map_err(|_| source_error("snapshot_cleanup"))?
            .is_dir()
        {
            return Err(source_error("snapshot_cleanup"));
        }
        removed = removed
            .checked_add(1)
            .filter(|count| *count <= MAX_STALE_APPLE_SNAPSHOTS)
            .ok_or_else(|| source_error("snapshot_entry_limit"))?;
        remove_dir_all(journal, &format!("imports/{name}"))
            .map_err(|_| source_error("snapshot_cleanup"))?;
    }
    Ok(())
}

fn is_apple_snapshot_residue(name: &str) -> bool {
    if name
        .strip_prefix(".tmp-apple-source-")
        .is_some_and(is_lower_hex_32)
    {
        return true;
    }
    let Some((snapshot, suffix)) = name
        .strip_prefix("..tmp-apple-source-")
        .and_then(|name| name.split_once(".staging."))
    else {
        return false;
    };
    if !is_lower_hex_32(snapshot) {
        return false;
    }
    suffix
        .strip_suffix(".tmp")
        .and_then(|value| value.split_once('_'))
        .is_some_and(|(process, time)| {
            !process.is_empty()
                && !time.is_empty()
                && process.bytes().all(|byte| byte.is_ascii_digit())
                && time.bytes().all(|byte| byte.is_ascii_digit())
        })
}

fn is_lower_hex_32(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn parse_source(
    source: &Path,
    window: &Window,
    raw_base: Option<&str>,
) -> Result<(Vec<NormalizedInput>, String), BodyIngestError> {
    let export = find_export(source)?;
    let rows = with_export(source, &export, |reader| {
        parse_export(reader, window, raw_base)
    })?;
    Ok((rows, export))
}

fn parse_export(
    reader: &mut dyn BufRead,
    window: &Window,
    raw_base: Option<&str>,
) -> Result<Vec<NormalizedInput>, BodyIngestError> {
    parse_export_with_event_limit(reader, window, raw_base, MAX_APPLE_XML_EVENTS_SCANNED)
}

fn parse_export_with_event_limit(
    reader: &mut dyn BufRead,
    window: &Window,
    raw_base: Option<&str>,
    event_limit: u64,
) -> Result<Vec<NormalizedInput>, BodyIngestError> {
    type WorkoutState = (BTreeMap<String, String>, Map<String, Value>, u64);

    let mut xml = Reader::from_reader(EventCappedReader::new(reader));
    xml.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    let mut rows = Vec::new();
    let mut selected_bytes = 0_usize;
    let mut ordinal = 0_u64;
    let mut events_scanned = 0_u64;
    let mut workout: Option<WorkoutState> = None;
    let mut closed = false;
    loop {
        let event = match xml.read_event_into(&mut buffer) {
            Ok(event) => event,
            Err(error) => return Err(read_failure(&xml, &error)),
        };
        xml.get_mut().reset();
        if !matches!(&event, Event::Eof) {
            events_scanned = events_scanned
                .checked_add(1)
                .filter(|count| *count <= event_limit)
                .ok_or_else(|| source_error("xml_event_count_limit"))?;
        }
        match event {
            Event::Eof => break,
            Event::End(end) if end.name().as_ref() == b"HealthData" => closed = true,
            Event::Empty(start) if start.name().as_ref() == b"HealthData" => closed = true,
            Event::Empty(start) if start.name().as_ref() == b"Record" => {
                ordinal = next_ordinal(ordinal)?;
                enforce_record_limit(ordinal)?;
                if let Some(row) = normalize(
                    "Record",
                    attributes(&xml, &start)?,
                    Map::new(),
                    ordinal,
                    window,
                    raw_base,
                )? {
                    push_row(&mut rows, &mut selected_bytes, row)?;
                }
            }
            Event::Start(start) if start.name().as_ref() == b"Record" => {
                ordinal = next_ordinal(ordinal)?;
                enforce_record_limit(ordinal)?;
                if let Some(row) = normalize(
                    "Record",
                    attributes(&xml, &start)?,
                    Map::new(),
                    ordinal,
                    window,
                    raw_base,
                )? {
                    push_row(&mut rows, &mut selected_bytes, row)?;
                }
            }
            Event::Empty(start) if start.name().as_ref() == b"Workout" => {
                ordinal = next_ordinal(ordinal)?;
                enforce_record_limit(ordinal)?;
                if let Some(row) = normalize(
                    "Workout",
                    attributes(&xml, &start)?,
                    Map::new(),
                    ordinal,
                    window,
                    raw_base,
                )? {
                    push_row(&mut rows, &mut selected_bytes, row)?;
                }
            }
            Event::Start(start) if start.name().as_ref() == b"Workout" => {
                ordinal = next_ordinal(ordinal)?;
                enforce_record_limit(ordinal)?;
                if workout.is_some() {
                    return Err(normalize_error("workout_nesting"));
                }
                workout = Some((attributes(&xml, &start)?, Map::new(), ordinal));
            }
            Event::Empty(start) | Event::Start(start)
                if start.name().as_ref() == b"WorkoutStatistics" =>
            {
                if let Some((_, metadata, _)) = workout.as_mut() {
                    add_workout_stat(metadata, &attributes(&xml, &start)?);
                }
            }
            Event::End(end) if end.name().as_ref() == b"Workout" => {
                let (attributes, statistics, item_ordinal) = workout
                    .take()
                    .ok_or_else(|| normalize_error("workout_nesting"))?;
                if let Some(row) = normalize(
                    "Workout",
                    attributes,
                    statistics,
                    item_ordinal,
                    window,
                    raw_base,
                )? {
                    push_row(&mut rows, &mut selected_bytes, row)?;
                }
            }
            _ => {}
        }
        buffer.clear();
    }
    if !closed {
        return Err(source_error(EXPORT_INCOMPLETE));
    }
    if workout.is_some() {
        return Err(normalize_error("workout_nesting"));
    }
    Ok(rows)
}

/// Why a read of the export stopped. XML that ends partway through a tag is an
/// export that was cut off, which is said apart from XML the reader can't get past.
fn read_failure(xml: &Reader<EventCappedReader<'_>>, error: &quick_xml::Error) -> BodyIngestError {
    if xml.get_ref().exceeded() {
        source_error("xml_event_limit")
    } else if matches!(error, quick_xml::Error::Syntax(_)) {
        source_error(EXPORT_INCOMPLETE)
    } else {
        source_error("xml")
    }
}

/// One element of an Apple export as a body-record reader sees it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AppleElement {
    pub attributes: BTreeMap<String, String>,
    /// `MetadataEntry` children, by key.
    pub metadata: BTreeMap<String, String>,
    /// A workout's `WorkoutStatistics` children.
    pub statistics: Vec<BTreeMap<String, String>>,
    /// A workout's `WorkoutEvent` children.
    pub events: Vec<BTreeMap<String, String>>,
}

/// What a scan of an Apple export hands its visitor, in document order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppleItem {
    Record(AppleElement),
    Workout(AppleElement),
    /// An element this reader doesn't open, by name.
    NotRead(String),
    /// A record or workout the scan refused, by stage. Nothing of it is held.
    Refused(&'static str),
}

/// Stream an Apple export's elements to `visit` without holding them, under the
/// same containment and archive checks as a bundle save. Nothing is held per
/// record, so there is no record or event count: the bounds are one event's
/// bytes, one element's children, and `MAX_APPLE_SCAN_BYTES` for the export.
pub fn scan_apple_export(
    source: &Path,
    visit: &mut dyn FnMut(AppleItem),
) -> Result<(), BodyIngestError> {
    let export = find_export_within(source, MAX_APPLE_SCAN_BYTES)?;
    with_export_within(source, &export, MAX_APPLE_SCAN_BYTES, |reader| {
        scan_elements(reader, MAX_APPLE_SCAN_CHILDREN, visit)
    })
}

/// Which file of a folder source holds the export, within the scan path's bounds. A
/// zip's bytes are already its identity, so it has none.
pub fn apple_folder_export(source: &Path) -> Result<Option<PathBuf>, BodyIngestError> {
    if !source.is_dir() {
        return Ok(None);
    }
    find_export_within(source, MAX_APPLE_SCAN_BYTES).map(|member| Some(source.join(member)))
}

/// What a failed Apple import says: plain words where they're owed, its stage otherwise.
#[must_use]
pub fn apple_refusal_text(error: &BodyIngestError) -> String {
    if error.stage() == EXPORT_INCOMPLETE {
        EXPORT_INCOMPLETE_TEXT.to_owned()
    } else {
        error.to_string()
    }
}

/// The scan path's whole-export bound, for a message that names it.
#[must_use]
pub const fn apple_scan_byte_bound() -> u64 {
    MAX_APPLE_SCAN_BYTES
}

fn scan_elements(
    reader: &mut dyn BufRead,
    child_limit: usize,
    visit: &mut dyn FnMut(AppleItem),
) -> Result<(), BodyIngestError> {
    let mut xml = Reader::from_reader(EventCappedReader::new(reader));
    xml.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    // The open `Record` or `Workout`, and how deep an unread subtree has gone.
    let mut open: Option<(bool, AppleElement)> = None;
    // Why the open element is refused, once it is: its children are dropped, and
    // it's counted when it closes.
    let mut refused: Option<&'static str> = None;
    let mut unread_depth = 0_u32;
    let mut closed = false;
    loop {
        let event = match xml.read_event_into(&mut buffer) {
            Ok(event) => event,
            Err(error) => return Err(read_failure(&xml, &error)),
        };
        xml.get_mut().reset();
        let (start, is_empty) = match event {
            Event::Eof => break,
            Event::Start(start) => (start, false),
            Event::Empty(start) => (start, true),
            Event::End(end) => {
                if unread_depth > 0 {
                    unread_depth -= 1;
                } else if open.is_none() && end.name().as_ref() == b"HealthData" {
                    closed = true;
                } else if matches!(end.name().as_ref(), b"Record" | b"Workout")
                    && let Some((workout, element)) = open.take()
                {
                    visit(if let Some(stage) = refused.take() {
                        AppleItem::Refused(stage)
                    } else if workout {
                        AppleItem::Workout(element)
                    } else {
                        AppleItem::Record(element)
                    });
                }
                buffer.clear();
                continue;
            }
            _ => {
                buffer.clear();
                continue;
            }
        };
        let name = start.name();
        let name = name.as_ref();
        if unread_depth > 0 {
            unread_depth += u32::from(!is_empty);
        } else if let Some((_, element)) = open.as_mut() {
            let children = element.metadata.len() + element.statistics.len() + element.events.len();
            let child = matches!(
                name,
                b"MetadataEntry" | b"WorkoutStatistics" | b"WorkoutEvent"
            );
            if child && (refused.is_some() || children >= child_limit) {
                if refused.is_none() {
                    refused = Some("children");
                    *element = AppleElement::default();
                }
            } else if child {
                match attributes(&xml, &start) {
                    Ok(entry) => match name {
                        b"MetadataEntry" => {
                            if let (Some(key), Some(value)) = (entry.get("key"), entry.get("value"))
                            {
                                element.metadata.insert(key.clone(), value.clone());
                            }
                        }
                        b"WorkoutStatistics" => element.statistics.push(entry),
                        _ => element.events.push(entry),
                    },
                    Err(_) => {
                        refused = Some(UNREADABLE_ATTRIBUTES);
                        *element = AppleElement::default();
                    }
                }
            } else {
                visit(AppleItem::NotRead(
                    String::from_utf8_lossy(name).into_owned(),
                ));
            }
            if !is_empty {
                unread_depth = 1;
            }
        } else {
            match name {
                b"Record" | b"Workout" => {
                    let workout = name == b"Workout";
                    let Ok(attributes) = attributes(&xml, &start) else {
                        // The tag's end is known, so only this record is lost.
                        visit(AppleItem::Refused(UNREADABLE_ATTRIBUTES));
                        if !is_empty {
                            unread_depth = 1;
                        }
                        buffer.clear();
                        continue;
                    };
                    let element = AppleElement {
                        attributes,
                        ..AppleElement::default()
                    };
                    if is_empty {
                        visit(if workout {
                            AppleItem::Workout(element)
                        } else {
                            AppleItem::Record(element)
                        });
                    } else {
                        open = Some((workout, element));
                    }
                }
                // Wrappers: a correlation's records are read as records.
                b"HealthData" => closed = is_empty,
                b"Correlation" => {}
                b"MetadataEntry" => {}
                _ => {
                    visit(AppleItem::NotRead(
                        String::from_utf8_lossy(name).into_owned(),
                    ));
                    if !is_empty {
                        unread_depth = 1;
                    }
                }
            }
        }
        buffer.clear();
    }
    if open.is_some() || unread_depth > 0 || !closed {
        return Err(source_error(EXPORT_INCOMPLETE));
    }
    Ok(())
}

struct EventCappedReader<'a> {
    inner: &'a mut dyn BufRead,
    remaining: usize,
    exceeded: bool,
}

impl<'a> EventCappedReader<'a> {
    fn new(inner: &'a mut dyn BufRead) -> Self {
        Self {
            inner,
            remaining: MAX_APPLE_EVENT_BYTES,
            exceeded: false,
        }
    }

    fn reset(&mut self) {
        self.remaining = MAX_APPLE_EVENT_BYTES;
        self.exceeded = false;
    }

    fn exceeded(&self) -> bool {
        self.exceeded
    }
}

impl Read for EventCappedReader<'_> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        let available = self.fill_buf()?;
        let count = available.len().min(output.len());
        output[..count].copy_from_slice(&available[..count]);
        self.consume(count);
        Ok(count)
    }
}

impl BufRead for EventCappedReader<'_> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        let available = self.inner.fill_buf()?;
        if available.is_empty() {
            return Ok(available);
        }
        if self.remaining == 0 {
            self.exceeded = true;
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Apple XML event exceeds the supported limit",
            ));
        }
        Ok(&available[..available.len().min(self.remaining)])
    }

    fn consume(&mut self, amount: usize) {
        self.remaining = self.remaining.saturating_sub(amount);
        self.inner.consume(amount);
    }
}

fn push_row(
    rows: &mut Vec<NormalizedInput>,
    selected_bytes: &mut usize,
    row: NormalizedInput,
) -> Result<(), BodyIngestError> {
    if rows.len() >= MAX_APPLE_SELECTED_ROWS {
        return Err(source_error("selected_row_limit"));
    }
    let row_bytes = serde_json::to_vec(&row.row)
        .map_err(|_| normalize_error("selected_row_size"))?
        .len();
    let identity_bytes = row
        .identity_metadata
        .as_ref()
        .map(serde_json::to_vec)
        .transpose()
        .map_err(|_| normalize_error("selected_row_size"))?
        .map_or(0, |bytes| bytes.len());
    *selected_bytes = selected_bytes
        .checked_add(row_bytes)
        .and_then(|total| total.checked_add(identity_bytes))
        .filter(|total| *total <= MAX_APPLE_SELECTED_BYTES)
        .ok_or_else(|| source_error("selected_bytes_limit"))?;
    rows.push(row);
    Ok(())
}

fn enforce_record_limit(ordinal: u64) -> Result<(), BodyIngestError> {
    if ordinal > MAX_APPLE_RECORDS_SCANNED {
        return Err(source_error("record_limit"));
    }
    Ok(())
}

fn normalize(
    tag: &str,
    attributes: BTreeMap<String, String>,
    statistics: Map<String, Value>,
    ordinal: u64,
    window: &Window,
    raw_base: Option<&str>,
) -> Result<Option<NormalizedInput>, BodyIngestError> {
    let start = attributes
        .get("startDate")
        .ok_or_else(|| normalize_error("start_date"))?;
    let day = apple_day(start).ok_or_else(|| normalize_error("start_date"))?;
    if !window.includes(&day) {
        return Ok(None);
    }
    let workout = tag == "Workout";
    let record_type = attributes
        .get(if workout {
            "workoutActivityType"
        } else {
            "type"
        })
        .cloned()
        .unwrap_or_else(|| tag.to_owned());
    let identity_metadata = metadata(&attributes, workout);
    let mut stored_metadata = identity_metadata.clone();
    for (key, value) in statistics {
        stored_metadata.entry(key).or_insert(value);
    }
    let mut row = Map::new();
    row.insert("schema".to_owned(), Value::String(SCHEMA.to_owned()));
    row.insert(
        "source_family".to_owned(),
        Value::String("apple_health".to_owned()),
    );
    row.insert(
        "kind".to_owned(),
        Value::String(if workout { "workout" } else { "record" }.to_owned()),
    );
    row.insert("record_type".to_owned(), Value::String(record_type));
    row.insert("day".to_owned(), Value::String(day));
    row.insert("start_date".to_owned(), Value::String(start.clone()));
    copy_string(&attributes, &mut row, "endDate", "end_date");
    copy_string(&attributes, &mut row, "sourceName", "source_name");
    copy_string(&attributes, &mut row, "sourceVersion", "source_version");
    copy_string(&attributes, &mut row, "unit", "unit");
    copy_string(&attributes, &mut row, "value", "value");
    row.insert("metadata".to_owned(), Value::Object(stored_metadata));
    Ok(Some(NormalizedInput {
        row,
        identity_metadata: Some(identity_metadata),
        raw_locator: raw_base.map(|base| format!("{base}#{}-{ordinal}", tag.to_ascii_lowercase())),
    }))
}

fn metadata(attributes: &BTreeMap<String, String>, workout: bool) -> Map<String, Value> {
    const CORE: [&str; 9] = [
        "type",
        "workoutActivityType",
        "sourceName",
        "sourceVersion",
        "creationDate",
        "startDate",
        "endDate",
        "unit",
        "value",
    ];
    let mut metadata = Map::new();
    for (key, value) in attributes {
        if !CORE.contains(&key.as_str()) {
            metadata.insert(key.clone(), Value::String(value.clone()));
        }
    }
    if workout {
        for key in [
            "duration",
            "durationUnit",
            "totalDistance",
            "totalDistanceUnit",
            "totalEnergyBurned",
            "totalEnergyBurnedUnit",
        ] {
            if let Some(value) = attributes.get(key) {
                metadata.insert(key.to_owned(), Value::String(value.clone()));
            }
        }
    }
    metadata
}

fn add_workout_stat(metadata: &mut Map<String, Value>, attributes: &BTreeMap<String, String>) {
    let Some(sum) = attributes.get("sum") else {
        return;
    };
    let kind = attributes.get("type").map(String::as_str).unwrap_or("");
    let keys = if kind == "HKQuantityTypeIdentifierActiveEnergyBurned" {
        Some((
            "totalEnergyBurned",
            "totalEnergyBurnedUnit",
            "totalEnergyBurnedType",
        ))
    } else if kind.contains("Distance") {
        Some(("totalDistance", "totalDistanceUnit", "totalDistanceType"))
    } else {
        None
    };
    let Some((value_key, unit_key, type_key)) = keys else {
        return;
    };
    metadata
        .entry(value_key.to_owned())
        .or_insert_with(|| Value::String(sum.clone()));
    if let Some(unit) = attributes.get("unit") {
        metadata
            .entry(unit_key.to_owned())
            .or_insert_with(|| Value::String(unit.clone()));
    }
    if !kind.is_empty() {
        metadata
            .entry(type_key.to_owned())
            .or_insert_with(|| Value::String(kind.to_owned()));
    }
}

fn attributes<R: BufRead>(
    reader: &Reader<R>,
    start: &BytesStart<'_>,
) -> Result<BTreeMap<String, String>, BodyIngestError> {
    let mut result = BTreeMap::new();
    for attribute in start.attributes().with_checks(true) {
        let attribute = attribute.map_err(|_| source_error("xml_attribute"))?;
        let key = std::str::from_utf8(attribute.key.as_ref())
            .map_err(|_| source_error("xml_attribute"))?
            .to_owned();
        let value = {
            #[allow(deprecated)]
            attribute
                .decode_and_unescape_value(reader.decoder())
                .map_err(|_| source_error("xml_attribute"))?
                .into_owned()
        };
        result.insert(key, value);
    }
    Ok(result)
}

fn copy_string(
    source: &BTreeMap<String, String>,
    target: &mut Map<String, Value>,
    source_key: &str,
    target_key: &str,
) {
    if let Some(value) = source.get(source_key) {
        target.insert(target_key.to_owned(), Value::String(value.clone()));
    }
}

fn apple_day(value: &str) -> Option<String> {
    let day = value.get(..10)?;
    NaiveDate::parse_from_str(day, "%Y-%m-%d").ok()?;
    Some(day.replace('-', ""))
}

fn parse_cli_day(value: &str) -> Result<String, BodyIngestError> {
    for format in ["%Y-%m-%d", "%Y%m%d"] {
        if let Ok(day) = NaiveDate::parse_from_str(value.trim(), format) {
            return Ok(day.format("%Y%m%d").to_string());
        }
    }
    Err(source_error("date_window"))
}

fn find_export(source: &Path) -> Result<String, BodyIngestError> {
    find_export_within(source, MAX_APPLE_SOURCE_BYTES)
}

fn find_export_within(source: &Path, source_limit: u64) -> Result<String, BodyIngestError> {
    if fs::symlink_metadata(source).is_ok_and(|metadata| metadata.file_type().is_dir()) {
        for candidate in ["export.xml", "apple_health_export/export.xml"] {
            match open_descendant_file(source, candidate) {
                Ok(_) => return Ok(candidate.to_owned()),
                Err(error) if missing_or_not_directory(&error) => {}
                Err(_) => return Err(source_error("source_symlink")),
            }
        }
        return Err(source_error("export_xml"));
    }
    if source
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("zip"))
    {
        let mut file = open_nofollow(source).map_err(|_| source_error("archive"))?;
        preflight_zip_within(&mut file, source_limit)?;
        let archive = zip::ZipArchive::new(file).map_err(|_| source_error("archive"))?;
        let (_archive, names) =
            crate::zip_names::prove_open_zip(archive).map_err(|_| source_error("archive"))?;
        for candidate in ["export.xml", "apple_health_export/export.xml"] {
            if names
                .iter()
                .any(|name| name.trim_end_matches('/') == candidate)
            {
                return Ok(candidate.to_owned());
            }
        }
        if let Some(name) = names.iter().find(|name| {
            let clean = name.trim_end_matches('/');
            clean.ends_with("/export.xml") && clean.contains("apple_health_export/")
        }) {
            return Ok(name.to_owned());
        }
    }
    Err(source_error("export_xml"))
}

fn is_export_member(name: &str) -> bool {
    let clean = name.trim_end_matches('/');
    matches!(clean, "export.xml" | "apple_health_export/export.xml")
        || (clean.ends_with("/export.xml") && clean.contains("apple_health_export/"))
}

fn with_export<T>(
    source: &Path,
    export: &str,
    operation: impl FnOnce(&mut dyn BufRead) -> Result<T, BodyIngestError>,
) -> Result<T, BodyIngestError> {
    with_export_within(source, export, MAX_APPLE_EXPORT_BYTES, operation)
}

/// `limit` bounds `export.xml`, and an archive holding it when it's larger
/// than the ordinary source bound.
fn with_export_within<T>(
    source: &Path,
    export: &str,
    limit: u64,
    operation: impl FnOnce(&mut dyn BufRead) -> Result<T, BodyIngestError>,
) -> Result<T, BodyIngestError> {
    if fs::symlink_metadata(source).is_ok_and(|metadata| metadata.file_type().is_dir()) {
        let file = open_descendant_file(source, export).map_err(|_| source_error("export_xml"))?;
        let metadata = file.metadata().map_err(|_| source_error("export_xml"))?;
        if !metadata.file_type().is_file() || metadata.len() > limit {
            return Err(source_error("export_size_limit"));
        }
        return with_capped_reader(file, limit, operation);
    }
    let mut file = open_nofollow(source).map_err(|_| source_error("archive"))?;
    preflight_zip_within(&mut file, limit.max(MAX_APPLE_SOURCE_BYTES))?;
    let archive = zip::ZipArchive::new(file).map_err(|_| source_error("archive"))?;
    let (mut archive, names) =
        crate::zip_names::prove_open_zip(archive).map_err(|_| source_error("archive"))?;
    let index = names
        .iter()
        .position(|name| name == export)
        .ok_or_else(|| source_error("export_xml"))?;
    let entry = archive
        .by_index(index)
        .map_err(|_| source_error("archive"))?;
    if entry.size() > limit {
        return Err(source_error("export_size_limit"));
    }
    with_capped_reader(entry, limit, operation)
}

struct CappedReader<R> {
    inner: R,
    remaining: u64,
    exceeded: Rc<Cell<bool>>,
}

impl<R: Read> Read for CappedReader<R> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        if self.remaining == 0 {
            let mut probe = [0_u8; 1];
            if self.inner.read(&mut probe)? == 0 {
                return Ok(0);
            }
            self.exceeded.set(true);
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Apple export exceeds the supported limit",
            ));
        }
        let allowed = usize::try_from(self.remaining)
            .unwrap_or(usize::MAX)
            .min(output.len());
        let count = self.inner.read(&mut output[..allowed])?;
        self.remaining = self.remaining.saturating_sub(count as u64);
        Ok(count)
    }
}

fn with_capped_reader<T>(
    reader: impl Read,
    limit: u64,
    operation: impl FnOnce(&mut dyn BufRead) -> Result<T, BodyIngestError>,
) -> Result<T, BodyIngestError> {
    let exceeded = Rc::new(Cell::new(false));
    let mut reader = BufReader::new(CappedReader {
        inner: reader,
        remaining: limit,
        exceeded: Rc::clone(&exceeded),
    });
    let result = operation(&mut reader);
    if exceeded.get() {
        Err(source_error("export_size_limit"))
    } else {
        result
    }
}

fn missing_or_not_directory(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
    )
}

fn preflight_zip(file: &mut File) -> Result<(), BodyIngestError> {
    preflight_zip_within(file, MAX_APPLE_SOURCE_BYTES)
}

fn preflight_zip_within(file: &mut File, limit: u64) -> Result<(), BodyIngestError> {
    let length = file.metadata().map_err(|_| source_error("archive"))?.len();
    if length > limit {
        return Err(source_error("source_size_limit"));
    }
    let tail_length = usize::try_from(length)
        .unwrap_or(usize::MAX)
        .min(ZIP_EOCD_MIN_BYTES + ZIP_MAX_COMMENT_BYTES);
    if tail_length < ZIP_EOCD_MIN_BYTES {
        return Err(source_error(EXPORT_INCOMPLETE));
    }
    let tail_offset = length
        .checked_sub(tail_length as u64)
        .ok_or_else(|| source_error("archive"))?;
    file.seek(SeekFrom::Start(tail_offset))
        .map_err(|_| source_error("archive"))?;
    let mut tail = vec![0_u8; tail_length];
    file.read_exact(&mut tail)
        .map_err(|_| source_error("archive"))?;
    let eocd = (0..=tail.len() - ZIP_EOCD_MIN_BYTES)
        .rev()
        .find(|index| {
            tail[*index..].starts_with(b"PK\x05\x06")
                && read_u16(&tail[*index + 20..*index + 22]).is_some_and(|comment| {
                    *index + ZIP_EOCD_MIN_BYTES + usize::from(comment) == tail.len()
                })
        })
        // A zip's end record is the last thing written, so an interrupted copy has none.
        .ok_or_else(|| source_error(EXPORT_INCOMPLETE))?;
    let disk = read_u16(&tail[eocd + 4..eocd + 6]).expect("fixed EOCD slice");
    let directory_disk = read_u16(&tail[eocd + 6..eocd + 8]).expect("fixed EOCD slice");
    let disk_entries = read_u16(&tail[eocd + 8..eocd + 10]).expect("fixed EOCD slice");
    let entries = read_u16(&tail[eocd + 10..eocd + 12]).expect("fixed EOCD slice");
    let directory_bytes = read_u32(&tail[eocd + 12..eocd + 16]).expect("fixed EOCD slice");
    let directory_offset = read_u32(&tail[eocd + 16..eocd + 20]).expect("fixed EOCD slice");
    if disk != 0 || directory_disk != 0 || disk_entries != entries {
        return Err(source_error("archive"));
    }
    let end_offset = tail_offset + eocd as u64;
    let (entries, directory_bytes, directory_offset, directory_limit) =
        if entries == u16::MAX || directory_bytes == u32::MAX || directory_offset == u32::MAX {
            zip64_directory(file, end_offset)?
        } else {
            (
                u64::from(entries),
                u64::from(directory_bytes),
                u64::from(directory_offset),
                end_offset,
            )
        };
    if entries > u64::from(MAX_APPLE_ARCHIVE_ENTRIES) {
        return Err(source_error("archive_entry_limit"));
    }
    if directory_bytes > u64::from(MAX_APPLE_ARCHIVE_DIRECTORY_BYTES) {
        return Err(source_error("archive_directory_limit"));
    }
    if directory_offset
        .checked_add(directory_bytes)
        .is_none_or(|end| end > directory_limit)
    {
        return Err(source_error("archive"));
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|_| source_error("archive"))?;
    Ok(())
}

/// A zip over 4 GiB keeps its directory's place in a zip64 record the plain end
/// record points past: entries, directory bytes, directory offset, and where the
/// directory must end by.
fn zip64_directory(
    file: &mut File,
    end_offset: u64,
) -> Result<(u64, u64, u64, u64), BodyIngestError> {
    let archive = || source_error("archive");
    let locator_offset = end_offset
        .checked_sub(ZIP64_LOCATOR_BYTES as u64)
        .ok_or_else(archive)?;
    let mut locator = [0_u8; ZIP64_LOCATOR_BYTES];
    file.seek(SeekFrom::Start(locator_offset))
        .and_then(|_| file.read_exact(&mut locator))
        .map_err(|_| archive())?;
    if !locator.starts_with(b"PK\x06\x07")
        || read_u32(&locator[4..8]) != Some(0)
        || read_u32(&locator[16..20]) != Some(1)
    {
        return Err(archive());
    }
    let record_offset = read_u64(&locator[8..16]).expect("fixed locator slice");
    if record_offset
        .checked_add(ZIP64_END_BYTES as u64)
        .is_none_or(|end| end > locator_offset)
    {
        return Err(archive());
    }
    let mut record = [0_u8; ZIP64_END_BYTES];
    file.seek(SeekFrom::Start(record_offset))
        .and_then(|_| file.read_exact(&mut record))
        .map_err(|_| archive())?;
    let on_this_disk = read_u64(&record[24..32]).expect("fixed record slice");
    let entries = read_u64(&record[32..40]).expect("fixed record slice");
    if !record.starts_with(b"PK\x06\x06")
        || read_u32(&record[16..20]) != Some(0)
        || read_u32(&record[20..24]) != Some(0)
        || on_this_disk != entries
    {
        return Err(archive());
    }
    Ok((
        entries,
        read_u64(&record[40..48]).expect("fixed record slice"),
        read_u64(&record[48..56]).expect("fixed record slice"),
        record_offset,
    ))
}

fn read_u16(bytes: &[u8]) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.try_into().ok()?))
}

fn read_u32(bytes: &[u8]) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.try_into().ok()?))
}

fn read_u64(bytes: &[u8]) -> Option<u64> {
    Some(u64::from_le_bytes(bytes.try_into().ok()?))
}

struct AppleSnapshot {
    journal: PathBuf,
    relative: String,
    source: PathBuf,
}

impl AppleSnapshot {
    fn create(
        source: &Path,
        journal: &Path,
        retention: BodyRawRetention,
    ) -> Result<Self, BodyIngestError> {
        Self::create_with_limits(source, journal, retention, SOURCE_TRAVERSAL_LIMITS)
    }

    fn create_with_limits(
        source: &Path,
        journal: &Path,
        retention: BodyRawRetention,
        traversal_limits: SourceTraversalLimits,
    ) -> Result<Self, BodyIngestError> {
        let metadata = fs::symlink_metadata(source).map_err(|_| source_error("source"))?;
        if !metadata.file_type().is_file() && !metadata.file_type().is_dir() {
            return Err(source_error("source_symlink"));
        }
        let imports = journal.join("imports");
        // `.tmp*` is excluded by the shipping backup engine, so a backup that
        // overlaps an import never captures this private, not-yet-authoritative
        // snapshot as owner history.
        let relative = format!("imports/.tmp-apple-source-{}", random_hex()?);
        let destination = journal.join(&relative);
        let (source_relative, assets) = if metadata.file_type().is_file() {
            if metadata.len() > MAX_APPLE_SOURCE_BYTES {
                return Err(source_error("source_size_limit"));
            }
            let name = source
                .file_name()
                .and_then(|name| name.to_str())
                .filter(|name| !name.is_empty() && !name.contains(['/', '\\']))
                .ok_or_else(|| source_error("source_name"))?
                .to_owned();
            (
                name.clone(),
                vec![RawAsset::File {
                    source: source.to_owned(),
                    relative: name,
                }],
            )
        } else if retention == BodyRawRetention::RetainComplete {
            let assets = collect_files_with_limits(source, traversal_limits)?;
            enforce_snapshot_budget(&assets, Some(source))?;
            (String::new(), assets)
        } else {
            let export = find_export(source)?;
            let asset = RawAsset::File {
                source: source.join(&export),
                relative: export,
            };
            enforce_snapshot_budget(std::slice::from_ref(&asset), Some(source))?;
            (String::new(), vec![asset])
        };
        create_directory_with_mode(&imports, 0o700)
            .map_err(|_| source_error("snapshot_directory"))?;
        publish_staged_dir(
            &destination,
            StagedDirOptions {
                directory_mode: Some(0o700),
            },
            |staging| {
                copy_snapshot_assets(
                    staging,
                    &assets,
                    metadata.file_type().is_dir().then_some(source),
                )
            },
        )
        .map_err(|_| source_error("source_snapshot"))?;
        let snapshot_source = if source_relative.is_empty() {
            destination.clone()
        } else {
            destination.join(source_relative)
        };
        Ok(Self {
            journal: journal.to_owned(),
            relative,
            source: snapshot_source,
        })
    }

    fn source(&self) -> &Path {
        &self.source
    }
}

impl Drop for AppleSnapshot {
    #[expect(
        clippy::disallowed_methods,
        reason = "system internal: drops this run's own Apple Health snapshot staging"
    )]
    fn drop(&mut self) {
        let _ = remove_dir_all(&self.journal, &self.relative);
    }
}

fn random_hex() -> Result<String, BodyIngestError> {
    let mut random = [0_u8; 16];
    random_fill(&mut random).map_err(|_| source_error("snapshot_random"))?;
    Ok(random
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(""))
}

fn enforce_snapshot_budget(
    assets: &[RawAsset],
    directory_root: Option<&Path>,
) -> Result<(), BodyIngestError> {
    let mut total = 0_u64;
    for asset in assets {
        let RawAsset::File { source, relative } = asset else {
            return Err(source_error("snapshot_asset"));
        };
        let file = match directory_root {
            Some(root) => open_descendant_file(root, relative),
            None => open_nofollow(source),
        }
        .map_err(|_| source_error("source_tree"))?;
        let metadata = file.metadata().map_err(|_| source_error("source_tree"))?;
        if !metadata.file_type().is_file() {
            return Err(source_error("source_symlink"));
        }
        total = total
            .checked_add(metadata.len())
            .filter(|total| *total <= MAX_APPLE_SOURCE_BYTES)
            .ok_or_else(|| source_error("source_size_limit"))?;
    }
    Ok(())
}

fn copy_snapshot_assets(
    staging: &Path,
    assets: &[RawAsset],
    directory_root: Option<&Path>,
) -> Result<(), BodyIngestError> {
    let options = AtomicWriteOptions { mode: Some(0o600) };
    let mut total = 0_u64;
    for asset in assets {
        let RawAsset::File { source, relative } = asset else {
            return Err(source_error("snapshot_asset"));
        };
        let destination = staging.join(relative);
        if let Some(parent) = destination.parent() {
            create_directory_with_mode(parent, 0o700)
                .map_err(|_| source_error("snapshot_directory"))?;
        }
        let remaining = MAX_APPLE_SOURCE_BYTES
            .checked_sub(total)
            .ok_or_else(|| source_error("source_size_limit"))?;
        let file = match directory_root {
            Some(root) => open_descendant_file(root, relative),
            None => open_nofollow(source),
        }
        .map_err(|_| source_error("snapshot_source"))?;
        let mut reader = BufReader::new(file).take(remaining.saturating_add(1));
        write_reader_exclusive(&destination, &mut reader, options)
            .map_err(|_| source_error("source_snapshot"))?;
        let copied = fs::symlink_metadata(&destination)
            .map_err(|_| source_error("source_snapshot"))?
            .len();
        total = total
            .checked_add(copied)
            .filter(|total| *total <= MAX_APPLE_SOURCE_BYTES)
            .ok_or_else(|| source_error("source_size_limit"))?;
    }
    Ok(())
}

fn raw_plan(source: &Path, retention: BodyRawRetention) -> Result<Vec<RawAsset>, BodyIngestError> {
    if retention == BodyRawRetention::Discard {
        return Ok(Vec::new());
    }
    let export = find_export(source)?;
    if retention == BodyRawRetention::RetainParsed {
        if source.is_dir() {
            return Ok(vec![RawAsset::File {
                source: source.join(export),
                relative: "export.xml".to_owned(),
            }]);
        }
        return Ok(vec![RawAsset::ZipMember {
            archive: source.to_owned(),
            member: export,
            relative: "export.xml".to_owned(),
        }]);
    }
    if source.is_file() {
        let name = source
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| source_error("source_name"))?;
        return Ok(vec![RawAsset::File {
            source: source.to_owned(),
            relative: name.to_owned(),
        }]);
    }
    collect_files_with_limits(source, SOURCE_TRAVERSAL_LIMITS)
}

fn collect_files_with_limits(
    root: &Path,
    limits: SourceTraversalLimits,
) -> Result<Vec<RawAsset>, BodyIngestError> {
    if limits.directories == 0 {
        return Err(source_error("source_directory_limit"));
    }
    let mut assets = Vec::new();
    let mut directories = 1_usize;
    let mut pending = vec![(root.to_owned(), 0_usize)];
    while let Some((directory, depth)) = pending.pop() {
        for entry in fs::read_dir(&directory).map_err(|_| source_error("source_tree"))? {
            let entry = entry.map_err(|_| source_error("source_tree"))?;
            let file_type = entry.file_type().map_err(|_| source_error("source_tree"))?;
            if file_type.is_symlink() {
                return Err(source_error("source_symlink"));
            }
            if file_type.is_dir() {
                let child_depth = depth
                    .checked_add(1)
                    .ok_or_else(|| source_error("source_depth_limit"))?;
                if child_depth > limits.depth {
                    return Err(source_error("source_depth_limit"));
                }
                directories = directories
                    .checked_add(1)
                    .filter(|count| *count <= limits.directories)
                    .ok_or_else(|| source_error("source_directory_limit"))?;
                pending.push((entry.path(), child_depth));
                continue;
            }
            if !file_type.is_file() {
                return Err(source_error("source_tree"));
            }
            if assets.len() >= limits.files {
                return Err(source_error("source_asset_limit"));
            }
            let path = entry.path();
            // The raw path is stored with `/` between its parts on every platform, so it
            // is built from the path's own components rather than its native string: on
            // Windows the native string separates them with `\`.
            let relative = path
                .strip_prefix(root)
                .map_err(|_| source_error("source_tree"))?
                .components()
                .map(|component| match component {
                    std::path::Component::Normal(name) => {
                        name.to_str().filter(|name| !name.contains(['/', '\\']))
                    }
                    _ => None,
                })
                .collect::<Option<Vec<_>>>()
                .map(|names| names.join("/"))
                .filter(|value| value.len() <= MAX_RAW_PATH_BYTES)
                .ok_or_else(|| source_error("source_name"))?;
            assets.push(RawAsset::File {
                source: path,
                relative,
            });
        }
    }
    assets.sort_by(|left, right| raw_relative(left).cmp(raw_relative(right)));
    Ok(assets)
}

fn raw_relative(asset: &RawAsset) -> &str {
    match asset {
        RawAsset::File { relative, .. }
        | RawAsset::ZipMember { relative, .. }
        | RawAsset::Bytes { relative, .. } => relative,
    }
}

fn raw_reference_base(
    source: &Path,
    retention: BodyRawRetention,
) -> Result<Option<String>, BodyIngestError> {
    match retention {
        BodyRawRetention::Discard => Ok(None),
        BodyRawRetention::RetainParsed => Ok(Some("export.xml".to_owned())),
        BodyRawRetention::RetainComplete if source.is_file() => Ok(Some(
            source
                .file_name()
                .and_then(|name| name.to_str())
                .filter(|name| !name.contains(['/', '\\']))
                .ok_or_else(|| source_error("source_name"))?
                .to_owned(),
        )),
        BodyRawRetention::RetainComplete => Ok(Some(find_export(source)?)),
    }
}

fn next_ordinal(value: u64) -> Result<u64, BodyIngestError> {
    value
        .checked_add(1)
        .ok_or_else(|| normalize_error("row_count"))
}

fn source_error(stage: &'static str) -> BodyIngestError {
    BodyIngestError::new(BodyIngestErrorKind::Source, stage)
}

fn normalize_error(stage: &'static str) -> BodyIngestError {
    BodyIngestError::new(BodyIngestErrorKind::Normalize, stage)
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write};
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "solstone-apple-source-limits-{}-{}",
                std::process::id(),
                TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).expect("create test directory");
            Self(path)
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn write_eocd(path: &Path, entries: u16, directory_bytes: u32) {
        let mut bytes = b"PK\x05\x06".to_vec();
        bytes.extend_from_slice(&0_u16.to_le_bytes());
        bytes.extend_from_slice(&0_u16.to_le_bytes());
        bytes.extend_from_slice(&entries.to_le_bytes());
        bytes.extend_from_slice(&entries.to_le_bytes());
        bytes.extend_from_slice(&directory_bytes.to_le_bytes());
        bytes.extend_from_slice(&0_u32.to_le_bytes());
        bytes.extend_from_slice(&0_u16.to_le_bytes());
        fs::write(path, bytes).expect("write synthetic EOCD");
    }

    fn write_test_approval(journal: &Path, decision: &str) {
        let path = journal.join("imports/_approvals/health_import_preflight.json");
        fs::create_dir_all(path.parent().expect("approval parent"))
            .expect("create approval directory");
        fs::write(
            path,
            serde_json::to_vec(&serde_json::json!({
                "schema": "solstone.health_import_preflight.v1",
                "checklist_version": "solstone.health_import_preflight.checklist.v3",
                "approved_by": "Synthetic Owner",
                "approved_at": "2026-08-09T00:00:00Z",
                "journal_root": journal.canonicalize().expect("canonical journal"),
                "approved_importers": ["apple_health"],
                "replication_destinations": {
                    "time_machine": {"decision": "excluded"},
                    "icloud": {"decision": "excluded"},
                    "solbase": {"decision": "excluded"},
                    "hosted_backup": {"decision": "excluded"},
                    "other": {"decision": "excluded"}
                },
                "raw_retention": {
                    "decision": decision,
                    "unparsed_sensitive_modalities_acknowledged": decision == "retain_complete"
                },
                "requires_per_run_confirmation": true,
                "no_real_health_data_in_artifact": true
            }))
            .expect("serialize approval"),
        )
        .expect("write approval");
    }

    #[test]
    fn xml_event_limit_refuses_a_single_proportional_attribute() {
        let mut bytes = b"<HealthData><Record type=\"".to_vec();
        bytes.extend(std::iter::repeat_n(b'a', MAX_APPLE_EVENT_BYTES));
        bytes.extend_from_slice(b"\" startDate=\"2026-01-02 00:00:00 +0000\"/></HealthData>");
        let mut reader = BufReader::new(Cursor::new(bytes));
        let result = parse_export(
            &mut reader,
            &Window::new(None, None).expect("open window"),
            None,
        );
        let Err(error) = result else {
            panic!("oversized XML event must fail")
        };
        assert_eq!(error.kind(), BodyIngestErrorKind::Source);
        assert_eq!(error.stage(), "xml_event_limit");
    }

    /// The save path's reader, on bytes.
    fn saved_rows(bytes: &[u8]) -> Result<usize, BodyIngestError> {
        let mut reader = BufReader::new(Cursor::new(bytes.to_vec()));
        parse_export_with_event_limit(
            &mut reader,
            &Window::new(None, None).expect("open window"),
            None,
            1_000,
        )
        .map(|rows| rows.len())
    }

    #[test]
    fn the_save_path_refuses_an_export_that_doesnt_close() {
        let record = br#"<Record type="HKQuantityTypeIdentifierHeartRate" sourceName="w" startDate="2026-10-10 06:31:10 -0600" endDate="2026-10-10 06:31:10 -0600" value="70"/>"#;
        for bytes in [
            b"".to_vec(),
            b"<cda:ClinicalDocument/>".to_vec(),
            [b"<HealthData>".as_slice(), record].concat(),
            [b"<HealthData>".as_slice(), &record[..40]].concat(),
            b"<HealthData><Workout workoutActivityType=\"x\" startDate=\"2026-10-10 06:31:10 -0600\">".to_vec(),
        ] {
            let error = saved_rows(&bytes).expect_err("an unclosed export");
            assert_eq!(error.kind(), BodyIngestErrorKind::Source);
            assert_eq!(error.stage(), EXPORT_INCOMPLETE);
        }
        assert_eq!(
            saved_rows(&[b"<HealthData>".as_slice(), record, b"</HealthData>"].concat()).unwrap(),
            1
        );
        assert_eq!(saved_rows(b"<HealthData/>").unwrap(), 0);
    }

    /// A zip whose end record defers to zip64, as each field of it should read.
    struct Zip64 {
        entries: u64,
        on_this_disk: u64,
        directory: u64,
        signature: [u8; 4],
        locator_disks: u32,
        /// The locator names a record in the end record's comment, past the locator.
        record_in_comment: bool,
    }

    impl Default for Zip64 {
        fn default() -> Self {
            Self {
                entries: 3,
                on_this_disk: 3,
                directory: 64,
                signature: *b"PK\x06\x06",
                locator_disks: 1,
                record_in_comment: false,
            }
        }
    }

    impl Zip64 {
        fn write(&self, path: &Path) {
            let mut record = self.signature.to_vec();
            record.extend_from_slice(&44_u64.to_le_bytes());
            record.extend_from_slice(&[0_u8; 4]);
            record.extend_from_slice(&[0_u8; 8]);
            record.extend_from_slice(&self.on_this_disk.to_le_bytes());
            record.extend_from_slice(&self.entries.to_le_bytes());
            record.extend_from_slice(&self.directory.to_le_bytes());
            record.extend_from_slice(&0_u64.to_le_bytes());
            let mut bytes = vec![0_u8; 64];
            let mut place = bytes.len() as u64;
            let mut comment = Vec::new();
            if self.record_in_comment {
                // Past the locator and the end record's fixed part.
                place += (ZIP64_LOCATOR_BYTES + ZIP_EOCD_MIN_BYTES) as u64;
                comment = record;
            } else {
                bytes.extend_from_slice(&record);
            }
            bytes.extend_from_slice(b"PK\x06\x07");
            bytes.extend_from_slice(&0_u32.to_le_bytes());
            bytes.extend_from_slice(&place.to_le_bytes());
            bytes.extend_from_slice(&self.locator_disks.to_le_bytes());
            bytes.extend_from_slice(b"PK\x05\x06");
            bytes.extend_from_slice(&[0_u8; 4]);
            bytes.extend_from_slice(&u16::MAX.to_le_bytes());
            bytes.extend_from_slice(&u16::MAX.to_le_bytes());
            bytes.extend_from_slice(&u32::MAX.to_le_bytes());
            bytes.extend_from_slice(&u32::MAX.to_le_bytes());
            bytes.extend_from_slice(&u16::try_from(comment.len()).unwrap().to_le_bytes());
            bytes.extend_from_slice(&comment);
            fs::write(path, bytes).expect("write synthetic zip64 end");
        }
    }

    #[test]
    fn a_zip64_directory_is_read_and_held_to_the_same_bounds() {
        let temporary = TestDir::new();
        let path = temporary.0.join("export.zip");
        let check = |zip: Zip64| {
            zip.write(&path);
            preflight_zip(&mut File::open(&path).expect("open zip")).map_err(|error| error.stage())
        };
        assert_eq!(check(Zip64::default()), Ok(()));
        assert_eq!(
            check(Zip64 {
                entries: u64::from(MAX_APPLE_ARCHIVE_ENTRIES) + 1,
                on_this_disk: u64::from(MAX_APPLE_ARCHIVE_ENTRIES) + 1,
                ..Zip64::default()
            }),
            Err("archive_entry_limit")
        );
        assert_eq!(
            check(Zip64 {
                directory: u64::from(MAX_APPLE_ARCHIVE_DIRECTORY_BYTES) + 1,
                ..Zip64::default()
            }),
            Err("archive_directory_limit")
        );
        for refused in [
            // A directory that would run past the record, and a record that isn't one.
            Zip64 {
                directory: 65,
                ..Zip64::default()
            },
            Zip64 {
                signature: *b"PK\x06\x05",
                ..Zip64::default()
            },
            // A zip split across disks, by its locator or its record.
            Zip64 {
                locator_disks: 2,
                ..Zip64::default()
            },
            Zip64 {
                on_this_disk: 2,
                ..Zip64::default()
            },
            // A well-formed record, where no record can be.
            Zip64 {
                record_in_comment: true,
                directory: 16,
                ..Zip64::default()
            },
        ] {
            assert_eq!(check(refused), Err("archive"));
        }
    }

    #[test]
    fn a_refusal_reads_plainly_where_the_export_is_why() {
        assert_eq!(
            apple_refusal_text(&source_error(EXPORT_INCOMPLETE)),
            EXPORT_INCOMPLETE_TEXT
        );
        assert_eq!(
            apple_refusal_text(&source_error("archive")),
            "body-ingest source: archive"
        );
    }

    #[test]
    fn total_xml_event_limit_counts_ignored_elements() {
        let mut reader = BufReader::new(Cursor::new(
            b"<HealthData><Metadata/><Metadata/></HealthData>".to_vec(),
        ));
        let result = parse_export_with_event_limit(
            &mut reader,
            &Window::new(None, None).expect("open window"),
            None,
            3,
        );
        let Err(error) = result else {
            panic!("ignored XML elements still consume the event budget")
        };
        assert_eq!(error.kind(), BodyIngestErrorKind::Source);
        assert_eq!(error.stage(), "xml_event_count_limit");
    }

    #[test]
    fn actual_export_reads_are_capped_after_the_file_is_opened() {
        let temporary = TestDir::new();
        let source = temporary.0.join("source");
        fs::create_dir(&source).expect("create source");
        let export = source.join("export.xml");
        fs::write(&export, b"1234").expect("write initial export");
        let file = open_descendant_file(&source, "export.xml").expect("open export");
        assert_eq!(file.metadata().expect("opened metadata").len(), 4);

        let error = with_capped_reader(file, 4, |reader| {
            OpenOptions::new()
                .append(true)
                .open(&export)
                .expect("grow opened export")
                .write_all(b"5")
                .expect("append export byte");
            let mut bytes = Vec::new();
            reader
                .read_to_end(&mut bytes)
                .map_err(|_| source_error("test_read"))?;
            Ok(bytes)
        })
        .expect_err("growth beyond the opened-file limit must fail");
        assert_eq!(error.stage(), "export_size_limit");

        let exact = with_capped_reader(Cursor::new(b"1234"), 4, |reader| {
            let mut bytes = Vec::new();
            reader
                .read_to_end(&mut bytes)
                .map_err(|_| source_error("test_read"))?;
            Ok(bytes)
        })
        .expect("the exact limit remains readable");
        assert_eq!(exact, b"1234");
    }

    #[test]
    fn complete_retention_bounds_first_source_tree_traversal() {
        let temporary = TestDir::new();
        let journal = temporary.0.join("journal");
        fs::create_dir(&journal).expect("create journal");
        let limits = SourceTraversalLimits {
            files: 1,
            directories: 2,
            depth: 1,
        };

        let too_many_files = temporary.0.join("too-many-files");
        fs::create_dir(&too_many_files).expect("create file source");
        fs::write(too_many_files.join("one"), b"").expect("write first file");
        fs::write(too_many_files.join("two"), b"").expect("write second file");
        let Err(error) = AppleSnapshot::create_with_limits(
            &too_many_files,
            &journal,
            BodyRawRetention::RetainComplete,
            limits,
        ) else {
            panic!("file count must fail before snapshotting")
        };
        assert_eq!(error.stage(), "source_asset_limit");

        let too_many_directories = temporary.0.join("too-many-directories");
        fs::create_dir(&too_many_directories).expect("create directory source");
        fs::create_dir(too_many_directories.join("one")).expect("create first child");
        fs::create_dir(too_many_directories.join("two")).expect("create second child");
        let Err(error) = AppleSnapshot::create_with_limits(
            &too_many_directories,
            &journal,
            BodyRawRetention::RetainComplete,
            limits,
        ) else {
            panic!("directory count must fail before snapshotting")
        };
        assert_eq!(error.stage(), "source_directory_limit");

        let too_deep = temporary.0.join("too-deep");
        fs::create_dir_all(too_deep.join("one/two")).expect("create deep source");
        let Err(error) = AppleSnapshot::create_with_limits(
            &too_deep,
            &journal,
            BodyRawRetention::RetainComplete,
            limits,
        ) else {
            panic!("directory depth must fail before snapshotting")
        };
        assert_eq!(error.stage(), "source_depth_limit");

        assert!(
            !journal.join("imports").exists(),
            "traversal limits must fail before a private snapshot is created"
        );
    }

    #[test]
    fn zip_directory_is_bounded_before_zip_archive_construction() {
        let temporary = TestDir::new();
        let too_many_entries = temporary.0.join("too-many-entries.zip");
        write_eocd(
            &too_many_entries,
            MAX_APPLE_ARCHIVE_ENTRIES.saturating_add(1),
            0,
        );
        let mut file = open_nofollow(&too_many_entries).expect("open entry-count archive");
        let error = preflight_zip(&mut file).expect_err("entry count must fail first");
        assert_eq!(error.stage(), "archive_entry_limit");

        let oversized_directory = temporary.0.join("oversized-directory.zip");
        write_eocd(
            &oversized_directory,
            1,
            MAX_APPLE_ARCHIVE_DIRECTORY_BYTES.saturating_add(1),
        );
        let mut file = open_nofollow(&oversized_directory).expect("open directory-size archive");
        let error = preflight_zip(&mut file).expect_err("directory size must fail first");
        assert_eq!(error.stage(), "archive_directory_limit");
    }

    #[test]
    fn save_rechecks_retention_after_the_lock_boundary() {
        let temporary = TestDir::new();
        let journal = temporary.0.join("journal");
        fs::create_dir(&journal).expect("create journal");
        write_test_approval(&journal, "retain_complete");
        let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../..")
            .join("tests/fixtures/importers/health/apple_health_synthetic");
        let report = save_apple_before_lock(
            &source,
            &journal,
            &AppleImportOptions {
                confirm_body_save: true,
                ..AppleImportOptions::default()
            },
            &mut || write_test_approval(&journal, "discard"),
        )
        .expect("save succeeds");
        let bundle = journal
            .join("imports")
            .join(report.bundle_id().expect("published bundle"));
        assert!(!bundle.join("raw").exists());
        assert!(!bundle.join("body-raw-inventory.jsonl").exists());
        let envelope: serde_json::Value = serde_json::from_slice(
            &fs::read(bundle.join("body-bundle.json")).expect("read envelope"),
        )
        .expect("parse envelope");
        assert_eq!(envelope["raw_retention"], "discard");
    }

    #[cfg(unix)]
    #[test]
    fn save_pins_the_approved_journal_before_a_symlink_can_be_retargeted() {
        use std::os::unix::fs::symlink;

        let temporary = TestDir::new();
        let approved = temporary.0.join("approved-journal");
        let alternate = temporary.0.join("alternate-journal");
        let selected = temporary.0.join("selected-journal");
        fs::create_dir(&approved).expect("create approved journal");
        fs::create_dir(&alternate).expect("create alternate journal");
        write_test_approval(&approved, "discard");
        symlink(&approved, &selected).expect("link selected journal");
        let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../..")
            .join("tests/fixtures/importers/health/apple_health_synthetic");

        let report = save_apple_before_lock(
            &source,
            &selected,
            &AppleImportOptions {
                confirm_body_save: true,
                ..AppleImportOptions::default()
            },
            &mut || {
                fs::remove_file(&selected).expect("remove selected link");
                symlink(&alternate, &selected).expect("retarget selected journal");
            },
        )
        .expect("save continues against its approved pinned journal");

        assert!(
            approved
                .join("imports")
                .join(report.bundle_id().expect("published bundle"))
                .is_dir()
        );
        assert!(
            !alternate.join("imports").exists(),
            "retargeted journal must receive no lock or body state"
        );
    }

    #[test]
    fn published_workout_pins_dedupe_and_stored_energy() {
        const EXPORT: &[u8] = br#"<HealthData>
<Workout workoutActivityType="HKWorkoutActivityTypeCycling" sourceName="Synthetic Watch" sourceVersion="1.0" creationDate="2026-01-02 18:00:00 +0000" startDate="2026-01-02 18:00:00 +0000" endDate="2026-01-02 18:30:00 +0000" totalDistance="18.2" totalDistanceUnit="km">
<WorkoutStatistics type="HKQuantityTypeIdentifierActiveEnergyBurned" sum="620" unit="kcal"/>
</Workout>
</HealthData>
"#;

        let parsed = parse_export(
            &mut Cursor::new(EXPORT),
            &Window::new(None, None).expect("window"),
            None,
        )
        .expect("parse export");
        assert_eq!(parsed.len(), 1);
        let identity_metadata = parsed[0].identity_metadata.as_ref();
        assert!(
            identity_metadata.is_none_or(|metadata| !metadata.contains_key("totalEnergyBurned")),
            "identity metadata must not contain totalEnergyBurned"
        );

        let temporary = TestDir::new();
        let source_dir = temporary.0.join("source");
        fs::create_dir(&source_dir).expect("create source directory");
        fs::write(source_dir.join("export.xml"), EXPORT).expect("write export.xml");

        let journal = temporary.0.join("journal");
        fs::create_dir(&journal).expect("create journal directory");
        write_test_approval(&journal, "discard");

        let report = save_apple_before_lock(
            &source_dir,
            &journal,
            &AppleImportOptions {
                confirm_body_save: true,
                ..AppleImportOptions::default()
            },
            &mut || {},
        )
        .expect("save succeeds");

        let bundle_id = report.bundle_id().expect("published bundle id");
        let normalized_dir = journal.join("imports").join(bundle_id).join("normalized");
        let mut jsonl_files: Vec<_> = fs::read_dir(&normalized_dir)
            .expect("read normalized dir")
            .map(|entry| entry.expect("entry").path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
            .collect();
        jsonl_files.sort();

        let mut published_rows = Vec::new();
        for file in jsonl_files {
            let content = fs::read_to_string(&file).expect("read jsonl");
            for line in content.lines() {
                if !line.trim().is_empty() {
                    let obj: Value = serde_json::from_str(line).expect("parse row json");
                    published_rows.push(obj);
                }
            }
        }
        assert_eq!(published_rows.len(), 1);
        let published = &published_rows[0];

        assert_eq!(
            published["dedupe_key"].as_str(),
            Some("sha256:ad4e0c8a66d5fd0901dac74aa5220d029791a0dd200d008ef09e33914226700d")
        );
        assert_eq!(
            published["metadata"]["totalEnergyBurned"],
            Value::String("620".into())
        );
    }

    #[test]
    fn zip2_name_detect_and_export_refuse_collision() {
        use crate::zip_names::fixtures::{Member, UTF8_FLAG, build_zip, colliding};

        let dir = TestDir::new();
        let ok_path = dir.0.join("ok.zip");
        fs::write(
            &ok_path,
            build_zip(&[Member {
                name: b"export.xml".to_vec(),
                flags: UTF8_FLAG,
                extra: Vec::new(),
                data: b"<HealthData></HealthData>".to_vec(),
            }]),
        )
        .unwrap();
        assert!(detect_apple_source(&ok_path).unwrap());
        with_export(&ok_path, "export.xml", |_reader| Ok(())).expect("literal export");

        let missing = dir.0.join("missing.zip");
        fs::write(
            &missing,
            build_zip(&[Member {
                name: b"other.xml".to_vec(),
                flags: UTF8_FLAG,
                extra: Vec::new(),
                data: b"<x/>".to_vec(),
            }]),
        )
        .unwrap();
        assert!(!detect_apple_source(&missing).unwrap());
        assert_eq!(find_export(&missing).unwrap_err().stage(), "export_xml");

        let collision = dir.0.join("collision.zip");
        fs::write(
            &collision,
            colliding("export.xml", b"<HealthData></HealthData>"),
        )
        .unwrap();
        let detect = detect_apple_source(&collision).unwrap_err();
        assert_eq!(detect.stage(), "archive");
        let export = with_export(&collision, "export.xml", |_reader| Ok(())).unwrap_err();
        assert_eq!(export.stage(), "archive");
        assert_ne!(export.stage(), "export_xml");
    }
}
