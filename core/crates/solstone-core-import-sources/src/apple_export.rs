// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Reading an Apple Health export: finding `export.xml` in a folder or zip, and
//! streaming its records and workouts to a visitor without holding them.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};
use solstone_core_import::{BodyIngestError, BodyIngestErrorKind};

use crate::nofollow_open::{
    open_descendant_regular as open_descendant_file, open_regular_file as open_nofollow,
};

const MAX_APPLE_SOURCE_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const MAX_APPLE_EVENT_BYTES: usize = 1024 * 1024;
const UNREADABLE_ATTRIBUTES: &str = "attributes";
/// The scan path holds nothing per record, so its only whole-export bound is
/// bytes, and it is its own, well past any real history.
const MAX_APPLE_SCAN_BYTES: u64 = 64 * 1024 * 1024 * 1024;
/// One element's children (metadata, statistics, events) on the scan path. An
/// element past it is refused, so one element can't grow without limit.
pub const MAX_APPLE_SCAN_CHILDREN: usize = 10_000;
const MAX_APPLE_ARCHIVE_ENTRIES: u16 = 10_000;
const MAX_APPLE_ARCHIVE_DIRECTORY_BYTES: u32 = 16 * 1024 * 1024;
const ZIP_EOCD_MIN_BYTES: usize = 22;
const ZIP_MAX_COMMENT_BYTES: usize = u16::MAX as usize;
const ZIP64_LOCATOR_BYTES: usize = 20;
const ZIP64_END_BYTES: usize = 56;
/// An export whose `HealthData` never closes: an interrupted copy, or not an export.
pub const EXPORT_INCOMPLETE: &str = "export_incomplete";
/// What an owner reads for one.
pub const EXPORT_INCOMPLETE_TEXT: &str = "this export looks incomplete, so none of it was saved. export it from Apple Health again and import the new copy";

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
                Err(error) => return Err(open_failure(&error, "source_symlink")),
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
    let mut file = open_nofollow(source).map_err(|error| open_failure(&error, "archive"))?;
    preflight_zip(&mut file)?;
    let archive = zip::ZipArchive::new(file).map_err(|_| source_error("archive"))?;
    let (_archive, names) =
        crate::zip_names::prove_open_zip(archive).map_err(|_| source_error("archive"))?;
    Ok(names.iter().any(|name| is_export_member(name)))
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

/// Stream an Apple export's elements to `visit` without holding them, under its
/// containment and archive checks. Nothing is held per
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

/// How many bytes the export's `export.xml` holds uncompressed, from the folder or
/// the zip's own directory. Nothing is unpacked or read past that directory.
pub fn apple_export_bytes(source: &Path) -> Result<u64, BodyIngestError> {
    let export = find_export_within(source, MAX_APPLE_SCAN_BYTES)?;
    if fs::symlink_metadata(source).is_ok_and(|metadata| metadata.file_type().is_dir()) {
        let file = open_descendant_file(source, &export)
            .map_err(|error| open_failure(&error, "export_xml"))?;
        return file
            .metadata()
            .map(|metadata| metadata.len())
            .map_err(|_| source_error("export_xml"));
    }
    let mut file = open_nofollow(source).map_err(|error| open_failure(&error, "archive"))?;
    preflight_zip_within(&mut file, MAX_APPLE_SCAN_BYTES.max(MAX_APPLE_SOURCE_BYTES))?;
    let archive = zip::ZipArchive::new(file).map_err(|_| source_error("archive"))?;
    let (mut archive, names) =
        crate::zip_names::prove_open_zip(archive).map_err(|_| source_error("archive"))?;
    let index = names
        .iter()
        .position(|name| *name == export)
        .ok_or_else(|| source_error("export_xml"))?;
    let entry = archive
        .by_index(index)
        .map_err(|_| source_error("archive"))?;
    Ok(entry.size())
}

/// Which file of a folder source holds the export, within the scan path's bounds. A
/// zip's bytes are already its identity, so it has none.
pub fn apple_folder_export(source: &Path) -> Result<Option<PathBuf>, BodyIngestError> {
    if !source.is_dir() {
        return Ok(None);
    }
    find_export_within(source, MAX_APPLE_SCAN_BYTES).map(|member| Some(source.join(member)))
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

fn find_export_within(source: &Path, source_limit: u64) -> Result<String, BodyIngestError> {
    if fs::symlink_metadata(source).is_ok_and(|metadata| metadata.file_type().is_dir()) {
        for candidate in ["export.xml", "apple_health_export/export.xml"] {
            match open_descendant_file(source, candidate) {
                Ok(_) => return Ok(candidate.to_owned()),
                Err(error) if missing_or_not_directory(&error) => {}
                Err(error) => return Err(open_failure(&error, "source_symlink")),
            }
        }
        return Err(source_error("export_xml"));
    }
    if source
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("zip"))
    {
        let mut file = open_nofollow(source).map_err(|error| open_failure(&error, "archive"))?;
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

/// `limit` bounds `export.xml`, and an archive holding it when it's larger
/// than the ordinary source bound.
fn with_export_within<T>(
    source: &Path,
    export: &str,
    limit: u64,
    operation: impl FnOnce(&mut dyn BufRead) -> Result<T, BodyIngestError>,
) -> Result<T, BodyIngestError> {
    if fs::symlink_metadata(source).is_ok_and(|metadata| metadata.file_type().is_dir()) {
        let file = open_descendant_file(source, export)
            .map_err(|error| open_failure(&error, "export_xml"))?;
        let metadata = file.metadata().map_err(|_| source_error("export_xml"))?;
        if !metadata.file_type().is_file() || metadata.len() > limit {
            return Err(source_error("export_size_limit"));
        }
        return with_capped_reader(file, limit, operation);
    }
    let mut file = open_nofollow(source).map_err(|error| open_failure(&error, "archive"))?;
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

/// An export the journal isn't allowed to open is said as that, not as the
/// shape of what was opened.
fn open_failure(error: &std::io::Error, otherwise: &'static str) -> BodyIngestError {
    if error.kind() == std::io::ErrorKind::PermissionDenied {
        source_error("permission")
    } else {
        source_error(otherwise)
    }
}

fn source_error(stage: &'static str) -> BodyIngestError {
    BodyIngestError::new(BodyIngestErrorKind::Source, stage)
}

#[cfg(test)]
mod tests {
    use std::fs::OpenOptions;
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

    fn scanned(bytes: &[u8]) -> Result<usize, BodyIngestError> {
        let mut records = 0;
        let mut reader = BufReader::new(Cursor::new(bytes.to_vec()));
        scan_elements(&mut reader, MAX_APPLE_SCAN_CHILDREN, &mut |item| {
            if matches!(item, AppleItem::Record(_)) {
                records += 1;
            }
        })?;
        Ok(records)
    }

    #[test]
    fn xml_event_limit_refuses_a_single_proportional_attribute() {
        let mut bytes = b"<HealthData><Record type=\"".to_vec();
        bytes.extend(std::iter::repeat_n(b'a', MAX_APPLE_EVENT_BYTES));
        bytes.extend_from_slice(b"\" startDate=\"2026-01-02 00:00:00 +0000\"/></HealthData>");
        let error = scanned(&bytes).expect_err("oversized XML event must fail");
        assert_eq!(error.kind(), BodyIngestErrorKind::Source);
        assert_eq!(error.stage(), "xml_event_limit");
    }

    #[test]
    fn a_scan_refuses_an_export_that_doesnt_close() {
        let record = br#"<Record type="HKQuantityTypeIdentifierHeartRate" sourceName="w" startDate="2026-10-10 06:31:10 -0600" endDate="2026-10-10 06:31:10 -0600" value="70"/>"#;
        for bytes in [
            b"".to_vec(),
            b"<cda:ClinicalDocument/>".to_vec(),
            [b"<HealthData>".as_slice(), record].concat(),
            [b"<HealthData>".as_slice(), &record[..40]].concat(),
            b"<HealthData><Workout workoutActivityType=\"x\" startDate=\"2026-10-10 06:31:10 -0600\">".to_vec(),
        ] {
            let error = scanned(&bytes).expect_err("an unclosed export");
            assert_eq!(error.kind(), BodyIngestErrorKind::Source);
            assert_eq!(error.stage(), EXPORT_INCOMPLETE);
        }
        assert_eq!(
            scanned(&[b"<HealthData>".as_slice(), record, b"</HealthData>"].concat()).unwrap(),
            1
        );
        assert_eq!(scanned(b"<HealthData/>").unwrap(), 0);
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
        with_export_within(&ok_path, "export.xml", MAX_APPLE_SCAN_BYTES, |_reader| {
            Ok(())
        })
        .expect("literal export");

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
        assert_eq!(
            find_export_within(&missing, MAX_APPLE_SCAN_BYTES)
                .unwrap_err()
                .stage(),
            "export_xml"
        );

        let collision = dir.0.join("collision.zip");
        fs::write(
            &collision,
            colliding("export.xml", b"<HealthData></HealthData>"),
        )
        .unwrap();
        let detect = detect_apple_source(&collision).unwrap_err();
        assert_eq!(detect.stage(), "archive");
        let export =
            with_export_within(&collision, "export.xml", MAX_APPLE_SCAN_BYTES, |_reader| {
                Ok(())
            })
            .unwrap_err();
        assert_eq!(export.stage(), "archive");
        assert_ne!(export.stage(), "export_xml");
    }

    #[test]
    fn an_exports_size_is_read_from_the_zips_directory_without_unpacking() {
        use zip::write::SimpleFileOptions;
        let directory = TestDir::new();
        let path = directory.0.join("export.zip");
        let body = format!("<HealthData>{}</HealthData>", "x".repeat(10_000));
        let mut writer = zip::ZipWriter::new(File::create(&path).unwrap());
        writer
            .start_file(
                "apple_health_export/export.xml",
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated),
            )
            .unwrap();
        writer.write_all(body.as_bytes()).unwrap();
        writer.finish().unwrap();
        assert!(fs::metadata(&path).unwrap().len() < body.len() as u64);
        assert_eq!(apple_export_bytes(&path).unwrap(), body.len() as u64);
        let folder = TestDir::new();
        fs::write(folder.0.join("export.xml"), &body).unwrap();
        assert_eq!(apple_export_bytes(&folder.0).unwrap(), body.len() as u64);
    }

    #[cfg(unix)]
    #[test]
    fn an_export_the_journal_cant_open_says_so_not_what_shape_it_is() {
        use std::os::unix::fs::PermissionsExt as _;
        let directory = TestDir::new();
        let zip = directory.0.join("export.zip");
        fs::write(&zip, b"PK").unwrap();
        fs::set_permissions(&zip, fs::Permissions::from_mode(0o000)).unwrap();
        if File::open(&zip).is_ok() {
            // Running as a user who can read anything, so nothing to show.
            return;
        }
        assert_eq!(scan_apple_export(&zip, &mut |_| {}).unwrap_err().stage(), "permission");
        let folder = directory.0.join("folder");
        fs::create_dir(&folder).unwrap();
        fs::write(folder.join("export.xml"), b"<HealthData/>").unwrap();
        fs::set_permissions(folder.join("export.xml"), fs::Permissions::from_mode(0o000)).unwrap();
        assert_eq!(detect_apple_source(&folder).unwrap_err().stage(), "permission");
    }
}
