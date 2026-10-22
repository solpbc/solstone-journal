// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! One day of location, assembled from the segments that hold it.
//!
//! A day is read from its surviving segments every time, so a deleted segment
//! is simply absent. Records stay at area level in this release:
//!
//! - **readings** are grouped into runs that share one area name, per device:
//!   *"readings near Golden, Colorado, 14:00 to 14:10"*. The two times are the
//!   first and last reading; they are not a claim about time spent anywhere.
//! - **visits** are the ones an iPhone reported. A visit the phone delivered
//!   more than once counts once, and its ends stay unknown when the phone did
//!   not know them. The phone reports a visit first on arrival and again,
//!   complete, when the owner leaves, and the complete report lands on the day
//!   of leaving, so a day also looks at the next day for it.
//! - **notes** say what could not be read.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, FixedOffset, NaiveTime, Offset, TimeZone, Utc};
use chrono_tz::Tz;
use solstone_core_journal_io::{DEFAULT_STREAM, PathOrDay, Segment, StreamLocation, iter_segments};

use crate::accuracy::{Packs, StopWords};
use crate::area::{AreaName, AreaTier};
use crate::record::{Platform, Precision, SegmentFile, Visit, parse_segment_file};
use crate::stay::{Sample, find_stays};

/// The file a phone writes into a segment.
pub const LOCATION_FILE: &str = "location.jsonl";
/// Readings further apart than this start a new run even in the same area.
const RUN_BREAK_SECONDS: i64 = 60 * 60;
/// A complete report is the arrival-only one again when it comes from the same
/// device, arrived no earlier and at most this much later, and within
/// [`TWIN_DISTANCE_M`]: the phone refines both when it reports the departure.
const TWIN_ARRIVAL_SECONDS: i64 = 3 * 60;
const TWIN_DISTANCE_M: f64 = 300.0;

/// Whether a stream directory is the direct layout or a named stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Layout {
    Direct,
    Named,
}

impl Layout {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Named => "named",
        }
    }
}

/// Exactly which segment a record came from, and which lines of its file.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SourceRef {
    pub day: String,
    pub layout: Layout,
    /// The stream as transcripts spells it (`_default` for the direct layout).
    pub stream: String,
    /// The exact segment directory name.
    pub segment: String,
    /// Zero-based lines of `location.jsonl` this record draws on.
    pub lines: Vec<u32>,
}

impl SourceRef {
    /// Where the owner opens this segment, and can delete it.
    pub fn transcripts_url(&self) -> String {
        format!(
            "/app/transcripts/{}?stream={}#{}",
            self.day,
            percent_encode(&self.stream),
            percent_encode(&self.segment)
        )
    }
}

fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// A device that sent location this day.
#[derive(Debug, Clone, PartialEq)]
pub struct Device {
    /// Stable within the day: the paired client, else the stream.
    pub key: String,
    pub platform: Platform,
    /// "iphone", "ipad", "watch", "android phone", "android tablet" or "phone".
    pub word: &'static str,
    /// The device's own name, shown only to tell two of a kind apart.
    pub name: Option<String>,
    /// The setting an iPhone kept location at, the latest one seen.
    pub tier: Option<String>,
    pub readings: usize,
    pub visits: usize,
    /// Stays worked out from this device's readings.
    pub stays: usize,
    /// Pieces whose file the phone marked as interrupted.
    pub interrupted_segments: usize,
    pub unreadable_lines: u32,
    pub unrecognised_lines: u32,
    pub unreadable_files: usize,
    pub first: Option<LocalTime>,
    pub last: Option<LocalTime>,
}

/// An instant with the wall-clock reading the device was keeping.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LocalTime {
    pub utc: DateTime<Utc>,
    pub offset: FixedOffset,
}

impl LocalTime {
    pub fn clock(&self) -> String {
        self.utc
            .with_timezone(&self.offset)
            .format("%H:%M")
            .to_string()
    }

    /// The local calendar day, `YYYYMMDD`.
    pub fn day(&self) -> String {
        self.utc
            .with_timezone(&self.offset)
            .format("%Y%m%d")
            .to_string()
    }
}

/// One reading drawn on the map.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub latitude: f64,
    pub longitude: f64,
    pub accuracy_m: Option<f64>,
}

/// One line in the day's list.
#[derive(Debug, Clone, PartialEq)]
pub enum Entry {
    /// Readings in one area, in order, from one device.
    Readings {
        device: usize,
        area: AreaName,
        precision: Precision,
        first: LocalTime,
        last: LocalTime,
        points: Vec<Point>,
        sources: Vec<SourceRef>,
    },
    /// A visit the iPhone reported.
    Visit {
        device: usize,
        area: AreaName,
        /// What the location-accuracy download says about this stop, when the
        /// owner has it and the phone's margin is within the cap.
        place: Option<StopWords>,
        arrival: Option<LocalTime>,
        departure: Option<LocalTime>,
        point: Point,
        sources: Vec<SourceRef>,
    },
    /// A stay the journal worked out from precise readings that stayed put.
    /// The two times are its first and last reading.
    Stay {
        device: usize,
        area: AreaName,
        place: Option<StopWords>,
        first: LocalTime,
        last: LocalTime,
        /// How many readings it rests on.
        count: usize,
        /// Its centre, with the margin that holds every supporting reading.
        point: Point,
        sources: Vec<SourceRef>,
    },
    /// A file, or lines in it, that could not be read.
    Unreadable {
        device: usize,
        at: Option<LocalTime>,
        lines: u32,
        whole_file: bool,
        sources: Vec<SourceRef>,
    },
}

impl Entry {
    fn sort_time(&self) -> Option<DateTime<Utc>> {
        match self {
            Self::Readings { first, .. } | Self::Stay { first, .. } => Some(first.utc),
            Self::Visit {
                arrival, departure, ..
            } => arrival.or(*departure).map(|time| time.utc),
            Self::Unreadable { at, .. } => at.map(|time| time.utc),
        }
    }

    pub fn sources(&self) -> &[SourceRef] {
        match self {
            Self::Readings { sources, .. }
            | Self::Visit { sources, .. }
            | Self::Stay { sources, .. }
            | Self::Unreadable { sources, .. } => sources,
        }
    }
}

/// Everything the journal holds about one day's location.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DayLocation {
    pub day: String,
    pub devices: Vec<Device>,
    pub entries: Vec<Entry>,
}

impl DayLocation {
    pub fn is_empty(&self) -> bool {
        self.devices.is_empty()
    }

    pub fn readings(&self) -> usize {
        self.devices.iter().map(|device| device.readings).sum()
    }

    pub fn visits(&self) -> usize {
        self.devices.iter().map(|device| device.visits).sum()
    }

    /// The device phrase for a sentence: "your iphone", or "your iphone (work)"
    /// when two devices of the same kind sent location that day.
    pub fn device_phrase(&self, index: usize) -> String {
        let device = &self.devices[index];
        let twins = self
            .devices
            .iter()
            .filter(|other| other.word == device.word)
            .count();
        match (&device.name, twins > 1) {
            (Some(name), true) => format!("your {} ({name})", device.word),
            _ => format!("your {}", device.word),
        }
    }
}

/// What the journal knows about one segment's device and clock.
#[derive(Debug, Clone, Default)]
pub struct SegmentContext {
    pub cid: Option<String>,
    pub platform: Option<String>,
    pub tz: Option<String>,
    pub utc_offset_seconds: Option<i32>,
}

/// Where device names come from. The journal implementation reads the
/// segment's upload record and the paired client's description.
pub trait DeviceDirectory {
    fn segment_context(&self, segment_dir: &Path) -> SegmentContext;
    /// The client's own name and device type ("phone", "tablet", …).
    fn describe(&self, cid: &str) -> (Option<String>, Option<String>);
}

struct LocatedSegment {
    source: SourceRef,
    path: PathBuf,
    start_wall: Option<NaiveTime>,
}

fn located(day: &str, segment: &Segment) -> Option<LocatedSegment> {
    let identity = segment.record_identity().ok()?;
    let layout = match segment.stream() {
        StreamLocation::Direct => Layout::Direct,
        StreamLocation::Named(_) => Layout::Named,
    };
    let stream = match layout {
        Layout::Direct => DEFAULT_STREAM.to_owned(),
        Layout::Named => identity.stream.to_owned(),
    };
    let name = segment.name().to_str()?.to_owned();
    let start_wall = segment
        .key()
        .split('_')
        .next()
        .and_then(|hhmmss| NaiveTime::parse_from_str(hhmmss, "%H%M%S").ok());
    Some(LocatedSegment {
        source: SourceRef {
            day: day.to_owned(),
            layout,
            stream,
            segment: name,
            lines: Vec::new(),
        },
        path: segment.path().to_path_buf(),
        start_wall,
    })
}

fn word_for(platform: Platform, device_type: Option<&str>) -> &'static str {
    match (platform, device_type) {
        (Platform::Watchos, _) => "watch",
        (Platform::Ios, Some("tablet")) => "ipad",
        (Platform::Ios, _) => "iphone",
        (Platform::Android, Some("tablet")) => "android tablet",
        (Platform::Android, _) => "android phone",
        (Platform::Unknown, Some("tablet")) => "tablet",
        (Platform::Unknown, _) => "phone",
    }
}

fn offset_for(context: &SegmentContext, owner: Tz, at: DateTime<Utc>) -> FixedOffset {
    if let Some(zone) = context
        .tz
        .as_deref()
        .and_then(|name| name.parse::<Tz>().ok())
    {
        return zone.offset_from_utc_datetime(&at.naive_utc()).fix();
    }
    if let Some(offset) = context.utc_offset_seconds.and_then(FixedOffset::east_opt) {
        return offset;
    }
    owner.offset_from_utc_datetime(&at.naive_utc()).fix()
}

struct Observation {
    device: usize,
    precision: Precision,
    time: LocalTime,
    point: Point,
    source: usize,
    line: u32,
}

struct VisitObservation {
    device: usize,
    visit: Visit,
    arrival: Option<LocalTime>,
    departure: Option<LocalTime>,
    source: usize,
}

fn same_point(a: &Point, b: &Point) -> bool {
    a.latitude == b.latitude && a.longitude == b.longitude && a.accuracy_m == b.accuracy_m
}

fn add_line(sources: &mut Vec<SourceRef>, located: &[LocatedSegment], source: usize, line: u32) {
    let template = &located[source].source;
    match sources.iter_mut().find(|existing| {
        existing.stream == template.stream
            && existing.segment == template.segment
            && existing.layout == template.layout
    }) {
        Some(existing) => {
            if !existing.lines.contains(&line) {
                existing.lines.push(line);
            }
        }
        None => {
            let mut fresh = template.clone();
            fresh.lines.push(line);
            sources.push(fresh);
        }
    }
}

/// Assemble one day from its segments' location files, naming it from the
/// area tier and, when the owner downloaded them, the location-accuracy packs.
pub fn read_day(
    journal: &Path,
    day: &str,
    tier: &AreaTier,
    packs: Option<&Packs>,
    directory: &dyn DeviceDirectory,
    owner: Tz,
) -> DayLocation {
    read_days(journal, &[day.to_owned()], tier, packs, directory, owner)
        .pop()
        .unwrap_or_default()
}

/// Assemble several days, each read once. A day still holding an arrival-only
/// visit is completed from the day after it: the one listed next when it is
/// the following day, else that day read on its own.
pub fn read_days(
    journal: &Path,
    days: &[String],
    tier: &AreaTier,
    packs: Option<&Packs>,
    directory: &dyn DeviceDirectory,
    owner: Tz,
) -> Vec<DayLocation> {
    let read = |day: &str| {
        let (located_segments, files) = read_segments(journal, day, directory);
        assemble(day, located_segments, files, tier, packs, directory, owner)
    };
    let mut views: Vec<DayLocation> = days.iter().map(|day| read(day)).collect();
    for index in 0..views.len() {
        if !views[index]
            .entries
            .iter()
            .any(|entry| arrival_only(entry).is_some())
        {
            continue;
        }
        let Some(next) = chrono::NaiveDate::parse_from_str(&views[index].day, "%Y%m%d")
            .ok()
            .and_then(|date| date.succ_opt())
            .map(|date| date.format("%Y%m%d").to_string())
        else {
            continue;
        };
        let (head, tail) = views.split_at_mut(index + 1);
        match tail.first() {
            Some(later) if later.day == next => complete_from(&mut head[index], later),
            _ => complete_from(&mut head[index], &read(&next)),
        }
    }
    views
}

/// The arrival and position of a visit the phone has reported only on arrival.
fn arrival_only(entry: &Entry) -> Option<(usize, DateTime<Utc>, Point)> {
    match entry {
        Entry::Visit {
            device,
            arrival: Some(arrival),
            departure: None,
            point,
            ..
        } => Some((*device, arrival.utc, *point)),
        _ => None,
    }
}

/// Whether `entry` is the complete report of an arrival-only visit.
fn completes(entry: &Entry, arrived: DateTime<Utc>, at: &Point) -> bool {
    let Entry::Visit {
        arrival: Some(arrival),
        departure: Some(_),
        point,
        ..
    } = entry
    else {
        return false;
    };
    let later = (arrival.utc - arrived).num_seconds();
    (0..=TWIN_ARRIVAL_SECONDS).contains(&later)
        && crate::pack::distance_m(at.latitude, at.longitude, point.latitude, point.longitude)
            <= TWIN_DISTANCE_M
}

fn add_sources(into: &mut Vec<SourceRef>, from: &[SourceRef]) {
    for source in from {
        match into.iter_mut().find(|existing| {
            existing.day == source.day
                && existing.stream == source.stream
                && existing.segment == source.segment
                && existing.layout == source.layout
        }) {
            Some(existing) => {
                for line in &source.lines {
                    if !existing.lines.contains(line) {
                        existing.lines.push(*line);
                    }
                }
                existing.lines.sort_unstable();
            }
            None => into.push(source.clone()),
        }
    }
    into.sort();
}

/// Replace each arrival-only visit whose complete report landed on the next
/// day with that report. The complete report wins; the row keeps both sources,
/// and a second arrival-only report of the same visit joins that row.
fn complete_from(view: &mut DayLocation, later: &DayLocation) {
    let mut taken: Vec<(usize, usize)> = Vec::new();
    let mut index = 0;
    while index < view.entries.len() {
        let Some((device, arrived, at)) = arrival_only(&view.entries[index]) else {
            index += 1;
            continue;
        };
        let key = &view.devices[device].key;
        let Some(found) = later.entries.iter().position(|entry| {
            matches!(entry, Entry::Visit { device: other, .. } if &later.devices[*other].key == key)
                && completes(entry, arrived, &at)
        }) else {
            index += 1;
            continue;
        };
        if let Some(&(_, row)) = taken.iter().find(|(twin, _)| *twin == found) {
            let orphan = view.entries.remove(index);
            let row = if row > index { row - 1 } else { row };
            if let Entry::Visit { sources, .. } = &mut view.entries[row] {
                add_sources(sources, orphan.sources());
            }
            view.devices[device].visits = view.devices[device].visits.saturating_sub(1);
            for (_, other) in &mut taken {
                if *other > index {
                    *other -= 1;
                }
            }
            continue;
        }
        let mut twin = later.entries[found].clone();
        if let Entry::Visit {
            device: twin_device,
            sources,
            ..
        } = &mut twin
        {
            *twin_device = device;
            add_sources(sources, view.entries[index].sources());
        }
        view.entries[index] = twin;
        taken.push((found, index));
        index += 1;
    }
    sort_entries(&mut view.entries);
}

fn sort_entries(entries: &mut [Entry]) {
    entries.sort_by(|a, b| match (a.sort_time(), b.sort_time()) {
        (Some(left), Some(right)) => left.cmp(&right),
        (None, Some(_)) => std::cmp::Ordering::Less,
        (Some(_), None) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });
}

fn read_segments(
    journal: &Path,
    day: &str,
    directory: &dyn DeviceDirectory,
) -> (Vec<LocatedSegment>, Vec<(SegmentFile, SegmentContext)>) {
    let segments = iter_segments(journal, PathOrDay::Day(day)).unwrap_or_default();
    let mut located_segments = Vec::new();
    let mut files = Vec::new();
    for segment in &segments {
        let file_path = segment.path().join(LOCATION_FILE);
        let Ok(bytes) = fs::read(&file_path) else {
            continue;
        };
        let Some(segment) = located(day, segment) else {
            continue;
        };
        let file = parse_segment_file(&bytes);
        let context = directory.segment_context(&segment.path);
        located_segments.push(segment);
        files.push((file, context));
    }
    (located_segments, files)
}

fn assemble(
    day: &str,
    located_segments: Vec<LocatedSegment>,
    files: Vec<(SegmentFile, SegmentContext)>,
    tier: &AreaTier,
    packs: Option<&Packs>,
    directory: &dyn DeviceDirectory,
    owner: Tz,
) -> DayLocation {
    let mut devices: Vec<Device> = Vec::new();
    let mut observations = Vec::new();
    let mut visits = Vec::new();
    let mut unreadable = Vec::new();

    for (index, (file, context)) in files.iter().enumerate() {
        let segment = &located_segments[index];
        let platform = file
            .platform()
            .or_else(|| context.platform.as_deref().map(Platform::parse))
            .unwrap_or(Platform::Unknown);
        let key = context
            .cid
            .clone()
            .map(|cid| format!("{cid}/{}", platform.as_str()))
            .unwrap_or_else(|| format!("stream:{}", segment.source.stream));
        let device = match devices.iter().position(|device| device.key == key) {
            Some(position) => position,
            None => {
                let (name, device_type) = context
                    .cid
                    .as_deref()
                    .map(|cid| directory.describe(cid))
                    .unwrap_or((None, None));
                devices.push(Device {
                    key,
                    platform,
                    word: word_for(platform, device_type.as_deref()),
                    name,
                    tier: None,
                    readings: 0,
                    visits: 0,
                    stays: 0,
                    interrupted_segments: 0,
                    unreadable_lines: 0,
                    unrecognised_lines: 0,
                    unreadable_files: 0,
                    first: None,
                    last: None,
                });
                devices.len() - 1
            }
        };
        let precision = file.precision(platform);
        if let Some(header) = &file.header {
            if header.tier.is_some() {
                devices[device].tier.clone_from(&header.tier);
            }
            if header.gap {
                devices[device].interrupted_segments += 1;
            }
        }
        let local = |at: DateTime<Utc>| LocalTime {
            utc: at,
            offset: offset_for(context, owner, at),
        };
        for fix in &file.fixes {
            observations.push(Observation {
                device,
                precision,
                time: local(fix.time),
                point: Point {
                    latitude: fix.latitude,
                    longitude: fix.longitude,
                    accuracy_m: fix.accuracy_m,
                },
                source: index,
                line: fix.line,
            });
        }
        for visit in &file.visits {
            visits.push(VisitObservation {
                device,
                arrival: visit.arrival.map(local),
                departure: visit.departure.map(local),
                visit: visit.clone(),
                source: index,
            });
        }
        let bad_lines = file.unreadable_lines + file.unrecognised_lines;
        devices[device].unreadable_lines += file.unreadable_lines;
        devices[device].unrecognised_lines += file.unrecognised_lines;
        if file.unreadable_file {
            devices[device].unreadable_files += 1;
        }
        if file.unreadable_file || bad_lines > 0 {
            let at = segment.start_wall.and_then(|wall| {
                let date = chrono::NaiveDate::parse_from_str(day, "%Y%m%d").ok()?;
                let naive = date.and_time(wall);
                let offset = context
                    .utc_offset_seconds
                    .and_then(FixedOffset::east_opt)
                    .unwrap_or_else(|| {
                        owner
                            .offset_from_local_datetime(&naive)
                            .earliest()
                            .map_or_else(|| FixedOffset::east_opt(0).unwrap(), |o| o.fix())
                    });
                let utc = offset
                    .from_local_datetime(&naive)
                    .single()?
                    .with_timezone(&Utc);
                Some(LocalTime { utc, offset })
            });
            unreadable.push(Entry::Unreadable {
                device,
                at,
                lines: bad_lines,
                whole_file: file.unreadable_file,
                sources: vec![segment.source.clone()],
            });
        }
    }

    // One reading per device and instant: Android repeats a fix it already sent
    // in the next piece, and a Watch re-stamps its last fix at each new piece.
    observations.sort_by(|a, b| {
        (a.device, a.time.utc, a.source, a.line).cmp(&(b.device, b.time.utc, b.source, b.line))
    });
    let mut kept: Vec<Observation> = Vec::with_capacity(observations.len());
    for observation in observations {
        if let Some(previous) = kept.last()
            && previous.device == observation.device
            && (previous.time.utc == observation.time.utc
                || (devices[observation.device].platform == Platform::Watchos
                    && same_point(&previous.point, &observation.point)))
        {
            continue;
        }
        kept.push(observation);
    }

    for observation in &kept {
        let device = &mut devices[observation.device];
        device.readings += 1;
        if device.first.is_none() {
            device.first = Some(observation.time);
        }
        device.last = Some(observation.time);
    }

    let mut visit_entries: Vec<Entry> = Vec::new();
    // A visit the phone delivered again later (arrival first, then with a
    // departure) is one visit. Its position is the first delivery's: a repeat
    // is the same claim again, not more evidence, so nothing is averaged.
    let mut merged: BTreeMap<(usize, Option<i64>, i64, i64), Entry> = BTreeMap::new();
    let mut order: Vec<(usize, Option<i64>, i64, i64)> = Vec::new();
    visits.sort_by_key(|visit| (visit.device, visit.source, visit.visit.line));
    for observation in visits {
        let key = (
            observation.device,
            observation.visit.arrival.map(|arrival| arrival.timestamp()),
            (observation.visit.latitude * 1e5).round() as i64,
            (observation.visit.longitude * 1e5).round() as i64,
        );
        match merged.get_mut(&key) {
            Some(Entry::Visit {
                departure, sources, ..
            }) => {
                if observation.departure.is_some() {
                    *departure = observation.departure;
                }
                add_line(
                    sources,
                    &located_segments,
                    observation.source,
                    observation.visit.line,
                );
            }
            _ => {
                let mut sources = Vec::new();
                add_line(
                    &mut sources,
                    &located_segments,
                    observation.source,
                    observation.visit.line,
                );
                order.push(key);
                let (latitude, longitude) =
                    (observation.visit.latitude, observation.visit.longitude);
                let mut area = tier.name_point(latitude, longitude);
                let mut place = None;
                if let Some((within, words)) = packs
                    .and_then(|packs| packs.stop(latitude, longitude, observation.visit.accuracy_m))
                {
                    area.within = within;
                    place = Some(words).filter(|words| !words.is_empty());
                }
                merged.insert(
                    key,
                    Entry::Visit {
                        device: observation.device,
                        area,
                        place,
                        arrival: observation.arrival,
                        departure: observation.departure,
                        point: Point {
                            latitude: observation.visit.latitude,
                            longitude: observation.visit.longitude,
                            accuracy_m: observation.visit.accuracy_m,
                        },
                        sources,
                    },
                );
            }
        }
    }
    for key in order {
        if let Some(entry) = merged.remove(&key) {
            visit_entries.push(entry);
        }
    }
    // The complete report usually differs a little from the arrival-only one:
    // the phone refines the arrival and the position when it reports leaving.
    // The complete report wins and keeps the arrival-only report's lines.
    let mut index = 0;
    while index < visit_entries.len() {
        if let Some((device, arrived, at)) = arrival_only(&visit_entries[index])
            && let Some(twin) = visit_entries.iter().position(|entry| {
                matches!(entry, Entry::Visit { device: other, .. } if *other == device)
                    && completes(entry, arrived, &at)
            })
        {
            let orphan = visit_entries.remove(index);
            let twin = if twin > index { twin - 1 } else { twin };
            if let Entry::Visit { sources, .. } = &mut visit_entries[twin] {
                add_sources(sources, orphan.sources());
            }
            continue;
        }
        index += 1;
    }
    for entry in &visit_entries {
        if let Entry::Visit { device, .. } = entry {
            devices[*device].visits += 1;
        }
    }

    // Stays the journal works out: precise readings that stay put. A stay
    // that overlaps a visit the phone reported is that visit, counted once.
    let mut in_stay = vec![false; kept.len()];
    let mut stay_entries: Vec<Entry> = Vec::new();
    let device_count = devices.len();
    for device_index in 0..device_count {
        let indexes: Vec<usize> = (0..kept.len())
            .filter(|&i| kept[i].device == device_index && kept[i].precision == Precision::Precise)
            .collect();
        let samples: Vec<Sample> = indexes
            .iter()
            .map(|&i| Sample {
                seconds: kept[i].time.utc.timestamp(),
                latitude: kept[i].point.latitude,
                longitude: kept[i].point.longitude,
                accuracy_m: kept[i].point.accuracy_m,
            })
            .collect();
        for found in find_stays(&samples) {
            let members: Vec<usize> = found.members.iter().map(|&m| indexes[m]).collect();
            let first = kept[members[0]].time;
            let last = kept[*members.last().unwrap_or(&members[0])].time;
            // The same stop, reported: same device, overlapping times with at
            // least one end known, and positions within both margins.
            let overlaps_visit = visit_entries.iter().any(|visit| match visit {
                Entry::Visit {
                    device,
                    arrival,
                    departure,
                    point,
                    ..
                } => {
                    *device == device_index
                        && (arrival.is_some() || departure.is_some())
                        && arrival.is_none_or(|arrival| arrival.utc <= last.utc)
                        && departure.is_none_or(|departure| departure.utc >= first.utc)
                        && crate::pack::distance_m(
                            point.latitude,
                            point.longitude,
                            found.latitude,
                            found.longitude,
                        ) <= point.accuracy_m.unwrap_or(0.0) + found.margin_m
                }
                _ => false,
            });
            if overlaps_visit {
                continue;
            }
            let mut sources = Vec::new();
            for &member in &members {
                in_stay[member] = true;
                add_line(
                    &mut sources,
                    &located_segments,
                    kept[member].source,
                    kept[member].line,
                );
            }
            let mut area = tier.name_point(found.latitude, found.longitude);
            let mut place = None;
            if let Some((within, words)) = packs
                .and_then(|packs| packs.stop(found.latitude, found.longitude, Some(found.margin_m)))
            {
                area.within = within;
                place = Some(words).filter(|words| !words.is_empty());
            }
            stay_entries.push(Entry::Stay {
                device: device_index,
                area,
                place,
                first,
                last,
                count: members.len(),
                point: Point {
                    latitude: found.latitude,
                    longitude: found.longitude,
                    accuracy_m: Some(found.margin_m),
                },
                sources,
            });
        }
    }

    for entry in &stay_entries {
        if let Entry::Stay { device, .. } = entry {
            devices[*device].stays += 1;
        }
    }

    let mut entries = Vec::new();
    let mut run: Option<(Entry, String)> = None;
    for (index, observation) in kept.iter().enumerate() {
        if in_stay[index] {
            // A reading inside a stay closes the run before it.
            if let Some((entry, _)) = run.take() {
                entries.push(entry);
            }
            continue;
        }
        let mut area = tier.name_point(observation.point.latitude, observation.point.longitude);
        area.within = packs.and_then(|packs| {
            packs.within(
                observation.point.latitude,
                observation.point.longitude,
                observation.point.accuracy_m,
            )
        });
        let run_key = format!(
            "{}|{}|{:?}",
            observation.device,
            observation.precision.as_str(),
            area.label()
        );
        let extends = match &run {
            Some((Entry::Readings { last, .. }, key)) => {
                key == &run_key
                    && (observation.time.utc - last.utc).num_seconds() <= RUN_BREAK_SECONDS
            }
            _ => false,
        };
        if extends {
            if let Some((
                Entry::Readings {
                    last,
                    points,
                    sources,
                    ..
                },
                _,
            )) = &mut run
            {
                *last = observation.time;
                points.push(observation.point);
                add_line(
                    sources,
                    &located_segments,
                    observation.source,
                    observation.line,
                );
            }
            continue;
        }
        if let Some((entry, _)) = run.take() {
            entries.push(entry);
        }
        let mut sources = Vec::new();
        add_line(
            &mut sources,
            &located_segments,
            observation.source,
            observation.line,
        );
        run = Some((
            Entry::Readings {
                device: observation.device,
                area,
                precision: observation.precision,
                first: observation.time,
                last: observation.time,
                points: vec![observation.point],
                sources,
            },
            run_key,
        ));
    }
    if let Some((entry, _)) = run.take() {
        entries.push(entry);
    }

    entries.extend(visit_entries);
    entries.extend(stay_entries);
    entries.extend(unreadable);
    sort_entries(&mut entries);
    for entry in &mut entries {
        let (Entry::Readings { sources, .. }
        | Entry::Visit { sources, .. }
        | Entry::Stay { sources, .. }
        | Entry::Unreadable { sources, .. }) = entry;
        for source in sources.iter_mut() {
            source.lines.sort_unstable();
        }
        sources.sort();
    }
    DayLocation {
        day: day.to_owned(),
        devices,
        entries,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use super::*;

    #[derive(Default)]
    pub(crate) struct FixedDirectory {
        pub(crate) contexts: RefCell<HashMap<String, SegmentContext>>,
        pub(crate) names: HashMap<String, (Option<String>, Option<String>)>,
    }

    impl DeviceDirectory for FixedDirectory {
        fn segment_context(&self, segment_dir: &Path) -> SegmentContext {
            let stream = segment_dir
                .parent()
                .and_then(Path::file_name)
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .to_owned();
            self.contexts
                .borrow()
                .get(&stream)
                .cloned()
                .unwrap_or_default()
        }

        fn describe(&self, cid: &str) -> (Option<String>, Option<String>) {
            self.names.get(cid).cloned().unwrap_or((None, None))
        }
    }

    pub(crate) fn tier() -> &'static AreaTier {
        crate::asset::area_tier().unwrap()
    }

    pub(crate) fn write(root: &Path, day: &str, stream: &str, segment: &str, text: &str) {
        let dir = root.join("chronicle").join(day).join(stream).join(segment);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(LOCATION_FILE), text).unwrap();
    }

    pub(crate) fn iphone_header(accuracy: &str, tier: &str) -> String {
        format!(
            r#"{{"accuracy":"{accuracy}","fix_count":0,"gap":false,"kind":"location","platform":"ios","schema":"solstone.location.segment/1","source":"location","tier":"{tier}"}}"#
        )
    }

    pub(crate) fn fix(t: &str, lat: f64, lon: f64, acc: f64) -> String {
        format!(
            r#"{{"alt":null,"course":null,"h_acc":{acc},"lat":{lat},"lon":{lon},"schema":"solstone.location.fix/1","speed":null,"stationary":false,"t":"{t}","v_acc":null}}"#
        )
    }

    pub(crate) fn directory() -> FixedDirectory {
        let directory = FixedDirectory::default();
        directory.contexts.borrow_mut().insert(
            "phone_mobile-segment".to_owned(),
            SegmentContext {
                cid: Some("sha256:iphone".to_owned()),
                platform: Some("ios".to_owned()),
                tz: Some("America/Denver".to_owned()),
                utc_offset_seconds: Some(-6 * 3600),
            },
        );
        directory.contexts.borrow_mut().insert(
            "pixel_location".to_owned(),
            SegmentContext {
                cid: Some("sha256:pixel".to_owned()),
                platform: Some("android".to_owned()),
                tz: Some("America/Denver".to_owned()),
                utc_offset_seconds: Some(-6 * 3600),
            },
        );
        directory
    }

    fn day_of(root: &Path) -> DayLocation {
        read_day(
            root,
            "20261001",
            tier(),
            None,
            &directory(),
            chrono_tz::America::Denver,
        )
    }

    #[test]
    fn readings_in_one_area_become_one_run_with_its_first_and_last_times() {
        let root = tempfile::tempdir().unwrap();
        let header = iphone_header("full", "full");
        write(
            root.path(),
            "20261001",
            "phone_mobile-segment",
            "140000_300",
            &format!(
                "{header}\n{}\n{}\n",
                fix("2026-10-01T20:00:10Z", 39.7555, -105.2211, 10.0),
                fix("2026-10-01T20:04:10Z", 39.7556, -105.2212, 10.0)
            ),
        );
        write(
            root.path(),
            "20261001",
            "phone_mobile-segment",
            "140500_300",
            &format!(
                "{header}\n{}\n{}\n",
                fix("2026-10-01T20:09:50Z", 39.7557, -105.2213, 10.0),
                fix("2026-10-01T20:40:00Z", 39.7527, -105.0001, 10.0)
            ),
        );
        let day = day_of(root.path());
        assert_eq!(day.devices.len(), 1);
        assert_eq!(day.devices[0].word, "iphone");
        assert_eq!(day.readings(), 4);
        assert_eq!(day.entries.len(), 2);
        let Entry::Readings {
            area,
            first,
            last,
            sources,
            precision,
            ..
        } = &day.entries[0]
        else {
            panic!("expected readings");
        };
        assert_eq!(area.label().as_deref(), Some("near Golden, Colorado"));
        assert_eq!(*precision, Precision::Precise);
        assert_eq!(
            (first.clock().as_str(), last.clock().as_str()),
            ("14:00", "14:09")
        );
        assert_eq!(sources.len(), 2);
        assert_eq!(sources[0].segment, "140000_300");
        assert_eq!(sources[0].lines, vec![1, 2]);
        assert_eq!(sources[1].lines, vec![1]);
        assert_eq!(
            sources[0].transcripts_url(),
            "/app/transcripts/20261001?stream=phone_mobile-segment#140000_300"
        );
        let Entry::Readings { area, .. } = &day.entries[1] else {
            panic!("expected readings");
        };
        assert_eq!(area.label().as_deref(), Some("near Denver, Colorado"));
    }

    #[test]
    fn a_visit_delivered_twice_counts_once_and_keeps_its_first_position() {
        let root = tempfile::tempdir().unwrap();
        let header = iphone_header("full", "light");
        write(
            root.path(),
            "20261001",
            "phone_mobile-segment",
            "090000_300",
            &format!(
                "{header}\n{}\n",
                r#"{"arrival":"2026-10-01T14:58:00Z","departure":null,"h_acc":40,"lat":39.7527,"lon":-105.0001,"schema":"solstone.location.visit/1"}"#
            ),
        );
        write(
            root.path(),
            "20261001",
            "phone_mobile-segment",
            "120500_300",
            &format!(
                "{header}\n{}\n",
                r#"{"arrival":"2026-10-01T14:58:00Z","departure":"2026-10-01T18:06:00Z","h_acc":40,"lat":39.7527,"lon":-105.0001,"schema":"solstone.location.visit/1"}"#
            ),
        );
        write(
            root.path(),
            "20261001",
            "phone_mobile-segment",
            "183000_300",
            &format!(
                "{header}\n{}\n",
                r#"{"arrival":"0001-01-01T00:00:00Z","departure":null,"h_acc":60,"lat":39.7555,"lon":-105.2211,"schema":"solstone.location.visit/1"}"#
            ),
        );
        let day = day_of(root.path());
        assert_eq!(day.visits(), 2);
        let visit = day
            .entries
            .iter()
            .find(|entry| {
                matches!(
                    entry,
                    Entry::Visit {
                        arrival: Some(_),
                        ..
                    }
                )
            })
            .unwrap();
        let Entry::Visit {
            arrival,
            departure,
            sources,
            area,
            ..
        } = visit
        else {
            unreachable!()
        };
        assert_eq!(arrival.unwrap().clock(), "08:58");
        assert_eq!(departure.unwrap().clock(), "12:06");
        assert_eq!(sources.len(), 2);
        assert_eq!(area.label().as_deref(), Some("near Denver, Colorado"));
        assert!(day.entries.iter().any(|entry| matches!(
            entry,
            Entry::Visit {
                arrival: None,
                departure: None,
                ..
            }
        )));
    }

    fn visit(arrival: &str, departure: Option<&str>, lat: f64, lon: f64) -> String {
        let departure = departure.map_or_else(|| "null".to_owned(), |d| format!("\"{d}\""));
        format!(
            r#"{{"arrival":"{arrival}","departure":{departure},"h_acc":40,"lat":{lat},"lon":{lon},"schema":"solstone.location.visit/1"}}"#
        )
    }

    fn visits_of(day: &DayLocation) -> Vec<(Option<String>, Option<String>, usize)> {
        day.entries
            .iter()
            .filter_map(|entry| match entry {
                Entry::Visit {
                    arrival,
                    departure,
                    sources,
                    ..
                } => Some((
                    arrival.map(|time| time.clock()),
                    departure.map(|time| time.clock()),
                    sources.len(),
                )),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_complete_report_the_phone_refined_replaces_its_arrival_only_report() {
        let root = tempfile::tempdir().unwrap();
        let header = iphone_header("full", "light");
        // Arrival-only at 08:58, then the complete report: arrival two minutes
        // later and about 100 m away. A third arrival, in Golden, has no twin.
        write(
            root.path(),
            "20261001",
            "phone_mobile-segment",
            "090000_300",
            &format!(
                "{header}\n{}\n",
                visit("2026-10-01T14:58:00Z", None, 39.7527, -105.0001)
            ),
        );
        write(
            root.path(),
            "20261001",
            "phone_mobile-segment",
            "120500_300",
            &format!(
                "{header}\n{}\n",
                visit(
                    "2026-10-01T15:00:00Z",
                    Some("2026-10-01T18:06:00Z"),
                    39.7536,
                    -105.0001
                )
            ),
        );
        write(
            root.path(),
            "20261001",
            "phone_mobile-segment",
            "183000_300",
            &format!(
                "{header}\n{}\n",
                visit("2026-10-02T00:30:00Z", None, 39.7555, -105.2211)
            ),
        );
        let day = day_of(root.path());
        assert_eq!(
            visits_of(&day),
            vec![
                (Some("09:00".to_owned()), Some("12:06".to_owned()), 2),
                (Some("18:30".to_owned()), None, 1),
            ]
        );
        assert_eq!(day.visits(), 2);
    }

    #[test]
    fn a_visit_completed_after_midnight_shows_once_with_its_departure() {
        let root = tempfile::tempdir().unwrap();
        let header = iphone_header("full", "light");
        // Arrived 21:10 on October 1; the complete report lands on October 2,
        // the day of leaving, with the arrival a minute later.
        write(
            root.path(),
            "20261001",
            "phone_mobile-segment",
            "211000_300",
            &format!(
                "{header}\n{}\n",
                visit("2026-10-02T03:10:00Z", None, 39.7527, -105.0001)
            ),
        );
        write(
            root.path(),
            "20261002",
            "phone_mobile-segment",
            "080000_300",
            &format!(
                "{header}\n{}\n",
                visit(
                    "2026-10-02T03:11:00Z",
                    Some("2026-10-02T14:00:00Z"),
                    39.7528,
                    -105.0002
                )
            ),
        );
        let first = day_of(root.path());
        assert_eq!(
            visits_of(&first),
            vec![(Some("21:11".to_owned()), Some("08:00".to_owned()), 2)]
        );
        let Some(Entry::Visit {
            departure: Some(departure),
            sources,
            ..
        }) = first
            .entries
            .iter()
            .find(|entry| matches!(entry, Entry::Visit { .. }))
        else {
            panic!("expected a visit");
        };
        assert_eq!(departure.day(), "20261002");
        assert_eq!(
            sources.iter().map(|s| s.day.as_str()).collect::<Vec<_>>(),
            vec!["20261001", "20261002"]
        );
        let second = read_day(
            root.path(),
            "20261002",
            tier(),
            None,
            &directory(),
            chrono_tz::America::Denver,
        );
        assert_eq!(
            visits_of(&second),
            vec![(Some("21:11".to_owned()), Some("08:00".to_owned()), 1)]
        );
    }

    #[test]
    fn two_arrival_reports_completed_the_next_day_make_one_visit() {
        let root = tempfile::tempdir().unwrap();
        let header = iphone_header("full", "light");
        write(
            root.path(),
            "20261001",
            "phone_mobile-segment",
            "211000_300",
            &format!(
                "{header}\n{}\n{}\n",
                visit("2026-10-02T03:10:00Z", None, 39.7527, -105.0001),
                visit("2026-10-02T03:12:00Z", None, 39.7529, -105.0001)
            ),
        );
        write(
            root.path(),
            "20261002",
            "phone_mobile-segment",
            "080000_300",
            &format!(
                "{header}\n{}\n",
                visit(
                    "2026-10-02T03:12:00Z",
                    Some("2026-10-02T14:00:00Z"),
                    39.7528,
                    -105.0002
                )
            ),
        );
        let day = day_of(root.path());
        assert_eq!(
            visits_of(&day),
            vec![(Some("21:12".to_owned()), Some("08:00".to_owned()), 2)]
        );
        assert_eq!(day.visits(), 1);
        let Some(Entry::Visit { sources, .. }) = day
            .entries
            .iter()
            .find(|entry| matches!(entry, Entry::Visit { .. }))
        else {
            panic!("expected a visit");
        };
        assert_eq!(sources[0].lines, vec![1, 2]);
    }

    #[test]
    fn another_devices_complete_report_is_not_a_twin() {
        let root = tempfile::tempdir().unwrap();
        let header = iphone_header("full", "light");
        let directory = directory();
        directory.contexts.borrow_mut().insert(
            "ipad_mobile-segment".to_owned(),
            SegmentContext {
                cid: Some("sha256:ipad".to_owned()),
                platform: Some("ios".to_owned()),
                tz: Some("America/Denver".to_owned()),
                utc_offset_seconds: Some(-6 * 3600),
            },
        );
        write(
            root.path(),
            "20261001",
            "phone_mobile-segment",
            "090000_300",
            &format!(
                "{header}\n{}\n",
                visit("2026-10-01T14:58:00Z", None, 39.7527, -105.0001)
            ),
        );
        write(
            root.path(),
            "20261001",
            "ipad_mobile-segment",
            "120500_300",
            &format!(
                "{header}\n{}\n",
                visit(
                    "2026-10-01T14:59:00Z",
                    Some("2026-10-01T18:06:00Z"),
                    39.7527,
                    -105.0001
                )
            ),
        );
        let day = read_day(
            root.path(),
            "20261001",
            tier(),
            None,
            &directory,
            chrono_tz::America::Denver,
        );
        assert_eq!(day.visits(), 2);
    }

    #[test]
    fn android_readings_are_approximate_and_a_repeated_fix_counts_once() {
        let root = tempfile::tempdir().unwrap();
        let row = |ms: i64| {
            format!(
                r#"{{"provider":"network","timestamp":{ms},"lat":39.7392,"lon":-104.9903,"accuracy":2000.0,"fixAge":0}}"#
            )
        };
        write(
            root.path(),
            "20261001",
            "pixel_location",
            "060500_300",
            &format!("{}\n{}\n", row(1_759_320_300_000), row(1_759_320_360_000)),
        );
        write(
            root.path(),
            "20261001",
            "pixel_location",
            "061000_300",
            &format!("{}\n{}\n", row(1_759_320_360_000), row(1_759_320_660_000)),
        );
        let day = day_of(root.path());
        assert_eq!(day.devices[0].word, "android phone");
        assert_eq!(day.readings(), 3);
        let Entry::Readings {
            precision, area, ..
        } = &day.entries[0]
        else {
            panic!("expected readings");
        };
        assert_eq!(*precision, Precision::Approximate);
        assert_eq!(area.label().as_deref(), Some("near Denver, Colorado"));
    }

    #[test]
    fn what_could_not_be_read_is_an_entry_of_its_own() {
        let root = tempfile::tempdir().unwrap();
        write(
            root.path(),
            "20261001",
            "pixel_location",
            "070000_300",
            "garbage\n",
        );
        let day = day_of(root.path());
        assert_eq!(day.readings(), 0);
        assert!(matches!(
            day.entries[0],
            Entry::Unreadable {
                lines: 1,
                whole_file: false,
                ..
            }
        ));
        assert!(
            !day.is_empty(),
            "a device that sent something unreadable still shows"
        );
    }

    #[test]
    fn a_deleted_segment_is_simply_absent() {
        let root = tempfile::tempdir().unwrap();
        let header = iphone_header("reduced", "balanced");
        write(
            root.path(),
            "20261001",
            "phone_mobile-segment",
            "140000_300",
            &format!(
                "{header}\n{}\n",
                fix("2026-10-01T20:00:10Z", 39.7555, -105.2211, 900.0)
            ),
        );
        let segment = root
            .path()
            .join("chronicle/20261001/phone_mobile-segment/140000_300");
        fs::remove_file(segment.join(LOCATION_FILE)).unwrap();
        fs::write(segment.join("tombstone.json"), "{}").unwrap();
        assert!(day_of(root.path()).is_empty());
    }

    #[test]
    fn precise_readings_that_stay_put_become_one_stay_and_a_visit_wins_its_overlap() {
        let root = tempfile::tempdir().unwrap();
        let header = iphone_header("full", "full");
        let dwell: Vec<String> = (0..10)
            .map(|minute| {
                fix(
                    &format!("2026-10-01T20:{minute:02}:10Z"),
                    39.7527 + f64::from(minute % 3) * 0.00002,
                    -105.0001,
                    8.0,
                )
            })
            .collect();
        write(
            root.path(),
            "20261001",
            "phone_mobile-segment",
            "140000_300",
            &format!("{header}\n{}\n", dwell.join("\n")),
        );
        let day = day_of(root.path());
        let stays: Vec<&Entry> = day
            .entries
            .iter()
            .filter(|entry| matches!(entry, Entry::Stay { .. }))
            .collect();
        assert_eq!(stays.len(), 1);
        let Entry::Stay {
            count,
            first,
            last,
            area,
            ..
        } = stays[0]
        else {
            unreachable!()
        };
        assert_eq!(*count, 10);
        assert_eq!(
            (first.clock().as_str(), last.clock().as_str()),
            ("14:00", "14:09")
        );
        assert_eq!(area.label().as_deref(), Some("near Denver, Colorado"));
        assert!(
            !day.entries
                .iter()
                .any(|entry| matches!(entry, Entry::Readings { .. })),
            "readings inside the stay are not listed again"
        );
        assert_eq!(day.devices[0].readings, 10, "every reading still counts");

        // The same dwell with a visit the phone reported over it is that visit.
        write(
            root.path(),
            "20261001",
            "phone_mobile-segment",
            "141000_300",
            &format!(
                "{header}\n{}\n",
                r#"{"arrival":"2026-10-01T19:59:00Z","departure":"2026-10-01T20:30:00Z","h_acc":30,"lat":39.7527,"lon":-105.0001,"schema":"solstone.location.visit/1"}"#
            ),
        );
        let day = day_of(root.path());
        assert!(
            !day.entries
                .iter()
                .any(|entry| matches!(entry, Entry::Stay { .. }))
        );
        assert_eq!(day.visits(), 1);
    }

    #[test]
    fn a_visit_with_no_known_end_or_somewhere_else_leaves_a_stay_alone() {
        let root = tempfile::tempdir().unwrap();
        let header = iphone_header("full", "full");
        let dwell: Vec<String> = (0..10)
            .map(|minute| {
                fix(
                    &format!("2026-10-01T20:{minute:02}:10Z"),
                    39.7527,
                    -105.0001,
                    8.0,
                )
            })
            .collect();
        write(
            root.path(),
            "20261001",
            "phone_mobile-segment",
            "140000_300",
            &format!("{header}\n{}\n", dwell.join("\n")),
        );
        write(
            root.path(),
            "20261001",
            "phone_mobile-segment",
            "141000_300",
            &format!(
                "{header}\n{}\n{}\n",
                r#"{"arrival":"0001-01-01T00:00:00Z","departure":null,"h_acc":30,"lat":39.7527,"lon":-105.0001,"schema":"solstone.location.visit/1"}"#,
                r#"{"arrival":"2026-10-01T19:00:00Z","departure":null,"h_acc":30,"lat":39.7555,"lon":-105.2211,"schema":"solstone.location.visit/1"}"#
            ),
        );
        let day = day_of(root.path());
        assert_eq!(day.visits(), 2);
        assert!(
            day.entries
                .iter()
                .any(|entry| matches!(entry, Entry::Stay { .. }))
        );
    }

    #[test]
    fn approximate_readings_never_become_a_stay() {
        let root = tempfile::tempdir().unwrap();
        let rows: Vec<String> = (0..20)
            .map(|minute| {
                format!(
                    r#"{{"provider":"network","timestamp":{},"lat":39.7392,"lon":-104.9903,"accuracy":2000.0,"fixAge":0}}"#,
                    1_759_320_300_000_i64 + minute * 60_000
                )
            })
            .collect();
        write(
            root.path(),
            "20261001",
            "pixel_location",
            "060500_300",
            &rows.join("\n"),
        );
        let day = day_of(root.path());
        assert!(
            !day.entries
                .iter()
                .any(|entry| matches!(entry, Entry::Stay { .. }))
        );
    }

    #[test]
    fn two_phones_of_a_kind_are_told_apart_by_name() {
        let root = tempfile::tempdir().unwrap();
        let mut directory = directory();
        directory.contexts.borrow_mut().insert(
            "second_mobile-segment".to_owned(),
            SegmentContext {
                cid: Some("sha256:second".to_owned()),
                platform: Some("ios".to_owned()),
                ..SegmentContext::default()
            },
        );
        directory.names.insert(
            "sha256:second".to_owned(),
            (Some("work".to_owned()), Some("phone".to_owned())),
        );
        let header = iphone_header("full", "full");
        for stream in ["phone_mobile-segment", "second_mobile-segment"] {
            write(
                root.path(),
                "20261001",
                stream,
                "140000_300",
                &format!(
                    "{header}\n{}\n",
                    fix("2026-10-01T20:00:10Z", 39.7555, -105.2211, 10.0)
                ),
            );
        }
        let day = read_day(
            root.path(),
            "20261001",
            tier(),
            None,
            &directory,
            chrono_tz::America::Denver,
        );
        let phrases: Vec<String> = (0..day.devices.len())
            .map(|i| day.device_phrase(i))
            .collect();
        assert!(phrases.contains(&"your iphone (work)".to_owned()));
        assert!(phrases.contains(&"your iphone".to_owned()));
    }
}
