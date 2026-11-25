// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Unified creation-time reader for import files, filenames, and container metadata.

use std::collections::BTreeSet;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use jiff::civil::{Date, DateTime, Time};
use jiff::tz::{Offset, TimeZone};
use regex::Regex;
use solstone_core_time::{LocalResult, from_local, parse_rfc3339, strptime_fixed};

use crate::timestamp::{Timestamp, validate_timestamp};

/// QuickTime epoch offset: seconds between 1904-01-01 00:00:00 UTC and 1970-01-01 00:00:00 UTC.
const QUICKTIME_EPOCH_OFFSET_SECS: i64 = 2_082_844_800;

/// A photo's creation moment in the timezone where it was taken.
///
/// When the photo carries a UTC offset (EXIF `OffsetTimeOriginal` /
/// `OffsetTimeDigitized`, or an explicit numeric offset on a client-supplied
/// `captured_at`), `stamp` stays on that capture clock and `utc_offset_seconds`
/// preserves the zone rather than folding it into the journal's home zone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureMoment {
    pub stamp: Timestamp,
    pub utc_offset_seconds: Option<i32>,
    pub tz: Option<String>,
}

/// Verified owner-camera GPS sighting extracted from a photo's EXIF IFD.
///
/// Populated only when hardware camera provenance (`Make` or `Model`) is
/// present alongside valid finite non-(0,0) coordinates, and never on
/// screenshots.
#[derive(Debug, Clone, PartialEq)]
pub struct CaptureLocation {
    pub latitude: f64,
    pub longitude: f64,
    pub accuracy_m: Option<f64>,
    pub camera_make: Option<String>,
    pub camera_model: Option<String>,
}

/// Extract a verified owner-camera GPS sighting from embedded EXIF metadata.
pub fn detect_photo_capture_location(
    path: &Path,
    extra_exif: Option<&[u8]>,
) -> Option<CaptureLocation> {
    if let Some(exif_bytes) = extra_exif
        && let Some(loc) = exif_photo_capture_location(exif_bytes)
    {
        return Some(loc);
    }
    if path.is_file() {
        if let Some(jpeg_exif) = scan_jpeg_exif_bytes(path)
            && let Some(loc) = exif_photo_capture_location(&jpeg_exif)
        {
            return Some(loc);
        }
        if let Ok(file) = File::open(path) {
            let mut buf = std::io::BufReader::new(file);
            if let Ok(parsed) = exif::Reader::new().read_from_container(&mut buf)
                && let Some(loc) = capture_location_from_exif(&parsed)
            {
                return Some(loc);
            }
        }
    }
    None
}

/// Parse verified camera GPS coordinates from raw EXIF TIFF bytes (with or
/// without a leading `Exif\0\0` header).
pub fn exif_photo_capture_location(exif_bytes: &[u8]) -> Option<CaptureLocation> {
    let tiff = exif_bytes.strip_prefix(b"Exif\0\0").unwrap_or(exif_bytes);
    let parsed = exif::Reader::new().read_raw(tiff.to_vec()).ok()?;
    capture_location_from_exif(&parsed)
}

fn capture_location_from_exif(data: &exif::Exif) -> Option<CaptureLocation> {
    use exif::{Context, In, Tag, Value};
    let ascii_tag = |tag: Tag| -> Option<String> {
        let field = data.get_field(tag, In::PRIMARY)?;
        match &field.value {
            Value::Ascii(items) => items
                .iter()
                .filter_map(|bytes| std::str::from_utf8(bytes).ok())
                .map(|s| s.trim_matches(char::from(0)).trim())
                .find(|s| !s.is_empty())
                .map(ToOwned::to_owned),
            _ => None,
        }
    };
    let camera_make = ascii_tag(Tag::Make);
    let camera_model = ascii_tag(Tag::Model);
    if camera_make.is_none() && camera_model.is_none() {
        return None;
    }
    // Exclude screenshot EXIF comments.
    if let Some(comment) = data.get_field(Tag::UserComment, In::PRIMARY) {
        let is_shot = match &comment.value {
            Value::Undefined(bytes, _) => bytes
                .strip_prefix(b"ASCII\0\0\0")
                .is_some_and(|rest| rest.trim_ascii() == b"Screenshot"),
            Value::Ascii(items) => items.iter().any(|b| b.trim_ascii() == b"Screenshot"),
            _ => false,
        };
        if is_shot {
            return None;
        }
    }
    if ascii_tag(Tag::ImageDescription).as_deref() == Some("Screenshot") {
        return None;
    }

    let dms_degrees = |tag: Tag| -> Option<f64> {
        let field = data.get_field(tag, In::PRIMARY)?;
        match &field.value {
            Value::Rational(rats) if rats.len() >= 3 => {
                if rats[0].denom == 0 || rats[1].denom == 0 || rats[2].denom == 0 {
                    return None;
                }
                let deg = f64::from(rats[0].num) / f64::from(rats[0].denom);
                let min = f64::from(rats[1].num) / f64::from(rats[1].denom);
                let sec = f64::from(rats[2].num) / f64::from(rats[2].denom);
                let total = deg + (min / 60.0) + (sec / 3600.0);
                total.is_finite().then_some(total)
            }
            _ => None,
        }
    };

    let lat_abs = dms_degrees(Tag::GPSLatitude)?;
    let lon_abs = dms_degrees(Tag::GPSLongitude)?;
    let lat_ref = ascii_tag(Tag::GPSLatitudeRef)?;
    let lon_ref = ascii_tag(Tag::GPSLongitudeRef)?;
    let latitude = match lat_ref.chars().next()?.to_ascii_uppercase() {
        'N' => lat_abs,
        'S' => -lat_abs,
        _ => return None,
    };
    let longitude = match lon_ref.chars().next()?.to_ascii_uppercase() {
        'E' => lon_abs,
        'W' => -lon_abs,
        _ => return None,
    };
    if !(-90.0..=90.0).contains(&latitude) || !(-180.0..=180.0).contains(&longitude) {
        return None;
    }
    if latitude.abs() < 1e-6 && longitude.abs() < 1e-6 {
        return None;
    }

    // Tag 0x001f in GPS IFD is GPSHPositioningError (metres).
    let accuracy_m = data
        .get_field(Tag(Context::Gps, 31), In::PRIMARY)
        .and_then(|field| match &field.value {
            Value::Rational(rats) => rats
                .first()
                .and_then(|r| (r.denom != 0).then(|| f64::from(r.num) / f64::from(r.denom))),
            Value::SRational(rats) => rats
                .first()
                .and_then(|r| (r.denom != 0).then(|| f64::from(r.num) / f64::from(r.denom))),
            _ => field.value.get_uint(0).map(f64::from),
        })
        .filter(|v| v.is_finite() && *v >= 0.0);

    Some(CaptureLocation {
        latitude,
        longitude,
        accuracy_m,
        camera_make,
        camera_model,
    })
}

/// Detect a photo's creation moment from embedded EXIF metadata only.
///
/// A filename never dates a photo. When EXIF carries both `DateTimeOriginal`
/// and `DateTimeDigitized` (for instance differing by a second after editing or
/// burst save), `DateTimeOriginal` wins rather than cancelling both.
pub fn detect_photo_capture_moment(
    path: &Path,
    zone: &TimeZone,
    extra_exif: Option<&[u8]>,
) -> Option<CaptureMoment> {
    if let Some(exif) = extra_exif
        && let Some(moment) = exif_photo_capture_moment(exif, zone)
    {
        return Some(moment);
    }
    if path.is_file()
        && let Some(jpeg_exif) = scan_jpeg_exif_bytes(path)
        && let Some(moment) = exif_photo_capture_moment(&jpeg_exif, zone)
    {
        return Some(moment);
    }
    None
}

/// Parse a photo's creation moment from raw EXIF TIFF bytes (with or without
/// a leading `Exif\0\0` header), keeping the photo's own capture timezone when
/// an offset tag is present.
pub fn exif_photo_capture_moment(exif_bytes: &[u8], zone: &TimeZone) -> Option<CaptureMoment> {
    let tiff = exif_bytes.strip_prefix(b"Exif\0\0").unwrap_or(exif_bytes);
    let tags = scan_exif_date_tags(tiff)?;
    if let Some(d_orig) = tags.date_original.as_deref() {
        let off = tags.offset_original.as_deref().or_else(|| {
            (tags.date_digitized.as_deref() == Some(d_orig))
                .then_some(tags.offset_digitized.as_deref())
                .flatten()
        });
        if let Some(moment) = parse_exif_capture_moment(d_orig, off, zone) {
            return Some(moment);
        }
    }
    if let Some(d_digit) = tags.date_digitized.as_deref()
        && let Some(moment) =
            parse_exif_capture_moment(d_digit, tags.offset_digitized.as_deref(), zone)
    {
        return Some(moment);
    }
    None
}

/// Parse a client-reported photo capture timestamp (such as RFC 3339
/// `2026-11-12T21:14:00+09:00` or offsetless civil `2026-11-12T21:14:00`).
///
/// An explicit numeric offset (`+09:00`, `-07:00`, `+00:00`) keeps the capture
/// wall clock and records `utc_offset_seconds`. `Z` or `-00:00` (unknown local
/// offset per RFC 3339 §4.3) folds the instant into `zone` without asserting a
/// separate capture zone.
pub fn parse_client_captured_at(raw: &str, zone: &TimeZone) -> Option<CaptureMoment> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(parsed) = parse_rfc3339(trimmed) {
        let raw_instant = parsed.timestamp();
        let second = raw_instant.as_second() - i64::from(raw_instant.subsec_nanosecond() < 0);
        let instant = jiff::Timestamp::new(second, 0).ok()?;
        let suffix = trimmed.rsplit_once(['T', 't', ' ']).map(|(_, time)| time)?;
        if suffix.ends_with('Z') || suffix.ends_with('z') || suffix.ends_with("-00:00") {
            let stamp = utc_stamp(instant, zone)?;
            return Some(CaptureMoment {
                stamp,
                utc_offset_seconds: None,
                tz: None,
            });
        }
        let offset_start = suffix.rfind(['+', '-'])?;
        let offset = parse_offset_string(&suffix[offset_start..])?;
        let civil = offset.to_datetime(instant);
        if placeholder(civil) {
            return None;
        }
        let stamp = validate_timestamp(&civil.strftime("%Y%m%d_%H%M%S").to_string())
            .ok()?
            .with_source_instant(instant);
        return Some(CaptureMoment {
            stamp,
            utc_offset_seconds: Some(offset.seconds()),
            tz: None,
        });
    }
    let naive = DateTime::strptime("%Y-%m-%dT%H:%M:%S", trimmed)
        .or_else(|_| DateTime::strptime("%Y:%m:%d %H:%M:%S", trimmed))
        .ok()?;
    if placeholder(naive) || leap_second(trimmed, naive) {
        return None;
    }
    let stamp = wall_stamp(naive, zone)?;
    Some(CaptureMoment {
        stamp,
        utc_offset_seconds: None,
        tz: None,
    })
}

struct ExifDateTags {
    date_original: Option<String>,
    date_digitized: Option<String>,
    offset_original: Option<String>,
    offset_digitized: Option<String>,
}

fn scan_exif_date_tags(exif_bytes: &[u8]) -> Option<ExifDateTags> {
    if exif_bytes.len() < 8 {
        return None;
    }

    let is_little_endian = match &exif_bytes[0..4] {
        b"II*\0" => true,
        b"MM\0*" => false,
        _ => return None,
    };

    let read_u16 = |bytes: &[u8], offset: usize| -> Option<u16> {
        let slice = bytes.get(offset..offset + 2)?;
        Some(if is_little_endian {
            u16::from_le_bytes(slice.try_into().ok()?)
        } else {
            u16::from_be_bytes(slice.try_into().ok()?)
        })
    };

    let read_u32 = |bytes: &[u8], offset: usize| -> Option<u32> {
        let slice = bytes.get(offset..offset + 4)?;
        Some(if is_little_endian {
            u32::from_le_bytes(slice.try_into().ok()?)
        } else {
            u32::from_be_bytes(slice.try_into().ok()?)
        })
    };

    let read_ascii = |bytes: &[u8], offset: usize, count: usize| -> Option<String> {
        let slice = bytes.get(offset..offset + count)?;
        let trimmed = slice
            .split(|&b| b == 0)
            .next()?
            .iter()
            .map(|&b| b as char)
            .collect::<String>();
        Some(trimmed.trim().to_owned())
    };

    let ifd0_offset = read_u32(exif_bytes, 4)? as usize;

    let mut exif_ifd_offset = None;
    let mut tags = ExifDateTags {
        date_original: None,
        date_digitized: None,
        offset_original: None,
        offset_digitized: None,
    };

    let scan_ifd = |offset: usize, exif_offset: &mut Option<usize>, out: &mut ExifDateTags| {
        let count = match read_u16(exif_bytes, offset) {
            Some(c) => c as usize,
            None => return,
        };
        let mut entry_pos = offset + 2;
        for _ in 0..count {
            if entry_pos + 12 > exif_bytes.len() {
                break;
            }
            let tag = match read_u16(exif_bytes, entry_pos) {
                Some(t) => t,
                None => break,
            };
            let kind = match read_u16(exif_bytes, entry_pos + 2) {
                Some(k) => k,
                None => break,
            };
            let count_val = match read_u32(exif_bytes, entry_pos + 4) {
                Some(c) => c as usize,
                None => break,
            };

            let str_val = if kind == 2 {
                if count_val <= 4 {
                    read_ascii(exif_bytes, entry_pos + 8, count_val)
                } else if let Some(ptr) = read_u32(exif_bytes, entry_pos + 8) {
                    read_ascii(exif_bytes, ptr as usize, count_val)
                } else {
                    None
                }
            } else {
                None
            };

            match tag {
                0x8769 => {
                    if let Some(ptr) = read_u32(exif_bytes, entry_pos + 8) {
                        *exif_offset = Some(ptr as usize);
                    }
                }
                0x9003 => {
                    if out.date_original.is_none() {
                        out.date_original = str_val;
                    }
                }
                0x9004 => {
                    if out.date_digitized.is_none() {
                        out.date_digitized = str_val;
                    }
                }
                0x9011 => {
                    if out.offset_original.is_none() {
                        out.offset_original = str_val;
                    }
                }
                0x9012 if out.offset_digitized.is_none() => {
                    out.offset_digitized = str_val;
                }
                _ => {}
            }
            entry_pos += 12;
        }
    };

    scan_ifd(ifd0_offset, &mut exif_ifd_offset, &mut tags);
    if let Some(sub_offset) = exif_ifd_offset {
        scan_ifd(sub_offset, &mut None, &mut tags);
    }
    Some(tags)
}

/// Detect the single consensus creation timestamp for a file, its filename, and container metadata.
pub fn detect_creation(path: &Path, filename: Option<&str>, zone: &TimeZone) -> Option<Timestamp> {
    detect_creation_with_exif(path, filename, zone, None)
}

/// Detect creation timestamp, including caller-supplied Exif metadata bytes (for PNG/WebP).
pub fn detect_creation_with_exif(
    path: &Path,
    filename: Option<&str>,
    zone: &TimeZone,
    extra_exif: Option<&[u8]>,
) -> Option<Timestamp> {
    let mut candidates = Vec::new();

    if let Some(name) = filename {
        candidates.extend(parse_filename_candidates(name, zone));
    }

    if let Some(exif) = extra_exif {
        candidates.extend(exif_creation_candidates(exif, zone));
    }

    if path.is_file() {
        if let Some(jpeg_candidates) = scan_jpeg_creation_candidates(path, zone) {
            candidates.extend(jpeg_candidates);
        }
        if let Some(isobmff_candidates) = scan_isobmff_creation_candidates(path, zone) {
            candidates.extend(isobmff_candidates);
        }
    }

    let timestamp = decide(candidates)?;
    // A bare repeated wall clock is ambiguous. Offset-bearing evidence can
    // disambiguate it without choosing an occurrence on the owner's behalf.
    timestamp.source_instant()?;
    Some(timestamp)
}

/// Parse creation candidates from raw Exif TIFF segment bytes.
pub fn exif_creation_candidates(exif_bytes: &[u8], zone: &TimeZone) -> Vec<Timestamp> {
    let Some(tags) = scan_exif_date_tags(exif_bytes) else {
        return Vec::new();
    };
    let mut candidates = Vec::new();

    if let Some(d_orig) = tags.date_original {
        let off = tags.offset_original.as_deref();
        if let Some(ts) = parse_exif_date_string(&d_orig, off, zone) {
            candidates.push(ts);
        }
    }

    if let Some(d_digit) = tags.date_digitized {
        let off = tags.offset_digitized.as_deref();
        if let Some(ts) = parse_exif_date_string(&d_digit, off, zone) {
            candidates.push(ts);
        }
    }

    candidates
}

/// Choose one timestamp among candidates, normalizing encodings and rejecting disagreements.
pub fn decide(candidates: impl IntoIterator<Item = Timestamp>) -> Option<Timestamp> {
    let mut chosen: Option<Timestamp> = None;
    for candidate in candidates {
        if let Some(previous) = &chosen {
            if previous.as_str() != candidate.as_str() {
                return None;
            }
            if let (Some(first), Some(next)) =
                (previous.source_instant(), candidate.source_instant())
                && first != next
            {
                return None;
            }
        }
        if chosen.is_none() || candidate.source_instant().is_some() {
            chosen = Some(candidate);
        }
    }
    chosen
}

/// Read an owner's timestamp given as an instant with its offset, such as
/// `2025-11-02T01:30:00-07:00`. The stamp is that instant's wall clock in `zone`, and the
/// instant is kept as an instant read from a file is, so the second occurrence of a repeated
/// hour stays the second occurrence.
pub fn owner_instant(raw: &str, zone: &TimeZone) -> Option<Timestamp> {
    utc_stamp(parse_rfc3339(raw).ok()?.timestamp(), zone)
}

fn utc_stamp(instant: jiff::Timestamp, zone: &TimeZone) -> Option<Timestamp> {
    // Whole seconds, rounded toward the past.
    let second = instant.as_second() - i64::from(instant.subsec_nanosecond() < 0);
    let instant = jiff::Timestamp::new(second, 0).ok()?;
    validate_timestamp(
        &instant
            .to_zoned_datetime(zone.clone())
            .strftime("%Y%m%d_%H%M%S")
            .to_string(),
    )
    .ok()
    .map(|stamp| stamp.with_source_instant(instant))
}

fn placeholder(naive: DateTime) -> bool {
    naive.year() == 0
        || (matches!(
            (naive.year(), naive.month(), naive.day()),
            (1904, 1, 1) | (1970, 1, 1)
        ) && naive.time() == Time::MIN)
}

/// A wall clock written with a leap second (`:60`) names no owner-local second.
/// The parser reads it as `:59`, so the text decides.
fn leap_second(text: &str, naive: DateTime) -> bool {
    naive.second() == 59 && text.ends_with("60")
}

fn wall_stamp(naive: DateTime, zone: &TimeZone) -> Option<Timestamp> {
    if placeholder(naive) {
        return None;
    }
    let stamp = validate_timestamp(&naive.strftime("%Y%m%d_%H%M%S").to_string()).ok()?;
    match from_local(zone, naive) {
        LocalResult::Single(instant) => Some(stamp.with_source_instant(instant.timestamp())),
        LocalResult::Ambiguous(_, _) => Some(stamp),
        LocalResult::None => None,
    }
}

fn parse_filename_candidates(filename: &str, zone: &TimeZone) -> Vec<Timestamp> {
    let limitless = parse_limitless_filename(filename, zone);
    let range = Regex::new(r"limitless_pendant_\d{4}-\d{2}-\d{2}T\d{2}-\d{2}-\d{2}_to_\d{4}-\d{2}-\d{2}T\d{2}-\d{2}-\d{2}").expect("valid regex");
    let remainder = limitless.as_ref().map(|_| range.replace(filename, ""));
    let filename = remainder.as_deref().unwrap_or(filename);

    let re_ymd_hms = Regex::new(r"(\d{4})[-_](\d{2})[-_](\d{2})[ _](\d{2})[:_](\d{2})[:_](\d{2})")
        .expect("valid regex");

    let re_ymd_hms_dash =
        Regex::new(r"(\d{4})-(\d{2})-(\d{2})_(\d{2})-(\d{2})-(\d{2})").expect("valid regex");

    let re_compact =
        Regex::new(r"(\d{4})(\d{2})(\d{2})_(\d{2})(\d{2})(\d{2})").expect("valid regex");

    let re_continuous =
        Regex::new(r"(\d{4})(\d{2})(\d{2})(\d{2})(\d{2})(\d{2})").expect("valid regex");

    let mut found = BTreeSet::new();
    found.extend(limitless);

    let check_boundary = |m: regex::Match<'_>| -> bool {
        let bytes = filename.as_bytes();
        if m.start() > 0 && bytes[m.start() - 1].is_ascii_digit() {
            return false;
        }
        if m.end() < bytes.len() && bytes[m.end()].is_ascii_digit() {
            return false;
        }
        true
    };

    for caps in re_ymd_hms.captures_iter(filename) {
        if let Some(m) = caps.get(0)
            && check_boundary(m)
            && let Some(ts) = make_filename_stamp(
                &caps[1], &caps[2], &caps[3], &caps[4], &caps[5], &caps[6], zone,
            )
        {
            found.insert(ts);
        }
    }

    for caps in re_ymd_hms_dash.captures_iter(filename) {
        if let Some(m) = caps.get(0)
            && check_boundary(m)
            && let Some(ts) = make_filename_stamp(
                &caps[1], &caps[2], &caps[3], &caps[4], &caps[5], &caps[6], zone,
            )
        {
            found.insert(ts);
        }
    }

    for caps in re_compact.captures_iter(filename) {
        if let Some(m) = caps.get(0)
            && check_boundary(m)
            && let Some(ts) = make_filename_stamp(
                &caps[1], &caps[2], &caps[3], &caps[4], &caps[5], &caps[6], zone,
            )
        {
            found.insert(ts);
        }
    }

    for caps in re_continuous.captures_iter(filename) {
        if let Some(m) = caps.get(0)
            && check_boundary(m)
            && let Some(ts) = make_filename_stamp(
                &caps[1], &caps[2], &caps[3], &caps[4], &caps[5], &caps[6], zone,
            )
        {
            found.insert(ts);
        }
    }

    found.into_iter().collect()
}

fn parse_limitless_filename(filename: &str, zone: &TimeZone) -> Option<Timestamp> {
    let re = Regex::new(r"limitless_pendant_(\d{4})-(\d{2})-(\d{2})T(\d{2})-(\d{2})-(\d{2})_to_")
        .expect("valid regex");

    let caps = re.captures(filename)?;
    let year: i16 = caps[1].parse().ok()?;
    let month: i8 = caps[2].parse().ok()?;
    let day: i8 = caps[3].parse().ok()?;
    let hour: i8 = caps[4].parse().ok()?;
    let minute: i8 = caps[5].parse().ok()?;
    let second: i8 = caps[6].parse().ok()?;

    let date = Date::new(year, month, day).ok()?;
    let time = Time::new(hour, minute, second, 0).ok()?;
    let utc_dt = Offset::UTC.to_timestamp(date.to_datetime(time)).ok()?;
    utc_stamp(utc_dt, zone)
}

fn make_filename_stamp(
    y: &str,
    m: &str,
    d: &str,
    h: &str,
    min: &str,
    s: &str,
    zone: &TimeZone,
) -> Option<Timestamp> {
    let year: i16 = y.parse().ok()?;
    let month: i8 = m.parse().ok()?;
    let day: i8 = d.parse().ok()?;
    let hour: i8 = h.parse().ok()?;
    let minute: i8 = min.parse().ok()?;
    let second: i8 = s.parse().ok()?;

    if year == 0 {
        return None;
    }

    let date = Date::new(year, month, day).ok()?;
    let time = Time::new(hour, minute, second, 0).ok()?;
    let naive = date.to_datetime(time);

    wall_stamp(naive, zone)
}

fn parse_exif_capture_moment(
    date_str: &str,
    offset_str: Option<&str>,
    zone: &TimeZone,
) -> Option<CaptureMoment> {
    let trimmed = date_str.trim();
    let naive = DateTime::strptime("%Y:%m:%d %H:%M:%S", trimmed).ok()?;
    if placeholder(naive) || leap_second(trimmed, naive) {
        return None;
    }
    if let Some(off) = offset_str {
        let offset = parse_offset_string(off)?;
        let instant = offset.to_timestamp(naive).ok()?;
        let stamp = validate_timestamp(&naive.strftime("%Y%m%d_%H%M%S").to_string())
            .ok()?
            .with_source_instant(instant);
        return Some(CaptureMoment {
            stamp,
            utc_offset_seconds: Some(offset.seconds()),
            tz: None,
        });
    }
    let stamp = wall_stamp(naive, zone)?;
    Some(CaptureMoment {
        stamp,
        utc_offset_seconds: None,
        tz: None,
    })
}

fn parse_exif_date_string(
    date_str: &str,
    offset_str: Option<&str>,
    zone: &TimeZone,
) -> Option<Timestamp> {
    let trimmed = date_str.trim();
    let naive = DateTime::strptime("%Y:%m:%d %H:%M:%S", trimmed).ok()?;
    if placeholder(naive) {
        return None;
    }

    if let Some(off) = offset_str {
        let offset = parse_offset_string(off)?;
        return utc_stamp(offset.to_timestamp(naive).ok()?, zone);
    }

    if leap_second(trimmed, naive) {
        return None;
    }
    wall_stamp(naive, zone)
}

fn parse_offset_string(off: &str) -> Option<Offset> {
    let trimmed = off.trim();
    if !trimmed.is_ascii() {
        return None;
    }
    if trimmed == "Z" || trimmed == "z" {
        return Some(Offset::UTC);
    }
    let (sign, rest) = if let Some(stripped) = trimmed.strip_prefix('+') {
        (1, stripped)
    } else {
        let stripped = trimmed.strip_prefix('-')?;
        (-1, stripped)
    };

    let (hours, mins) = if let Some((h, m)) = rest.split_once(':') {
        (h.parse::<i32>().ok()?, m.parse::<i32>().ok()?)
    } else if rest.len() == 4 {
        (
            rest[0..2].parse::<i32>().ok()?,
            rest[2..4].parse::<i32>().ok()?,
        )
    } else if rest.len() == 2 {
        (rest.parse::<i32>().ok()?, 0)
    } else {
        return None;
    };

    if !(0..24).contains(&hours) || !(0..60).contains(&mins) {
        return None;
    }
    let total_secs = sign * (hours * 3600 + mins * 60);
    Offset::from_seconds(total_secs).ok()
}

fn scan_jpeg_exif_bytes(path: &Path) -> Option<Vec<u8>> {
    let mut file = File::open(path).ok()?;
    let mut header = [0u8; 2];
    file.read_exact(&mut header).ok()?;
    if header != [0xFF, 0xD8] {
        return None;
    }

    let mut byte = [0u8; 1];
    while file.read_exact(&mut byte).ok().is_some() {
        if byte[0] != 0xFF {
            break;
        }
        while byte[0] == 0xFF {
            file.read_exact(&mut byte).ok()?;
        }
        let marker = byte[0];
        if marker == 0xD8 || (0xD0..=0xD7).contains(&marker) || marker == 0x01 {
            continue;
        }
        if marker == 0xD9 || marker == 0xDA {
            break;
        }

        let mut len_buf = [0u8; 2];
        file.read_exact(&mut len_buf).ok()?;
        let seg_len = u16::from_be_bytes(len_buf) as usize;
        if seg_len < 2 {
            break;
        }
        let payload_len = seg_len - 2;

        if marker == 0xE1 {
            let mut payload = vec![0u8; payload_len];
            file.read_exact(&mut payload).ok()?;
            if payload.starts_with(b"Exif\0\0") && payload.len() >= 6 {
                return Some(payload[6..].to_vec());
            }
        } else {
            file.seek(SeekFrom::Current(payload_len as i64)).ok()?;
        }
    }
    None
}

fn scan_jpeg_creation_candidates(path: &Path, zone: &TimeZone) -> Option<Vec<Timestamp>> {
    let exif = scan_jpeg_exif_bytes(path)?;
    Some(exif_creation_candidates(&exif, zone))
}

fn scan_isobmff_creation_candidates(path: &Path, zone: &TimeZone) -> Option<Vec<Timestamp>> {
    let mut file = File::open(path).ok()?;
    let file_len = file.metadata().ok()?.len();
    if file_len < 8 {
        return None;
    }

    let mut candidates = Vec::new();
    walk_isobmff_boxes(&mut file, 0, file_len, zone, &mut candidates, 0);

    if candidates.is_empty() {
        None
    } else {
        Some(candidates)
    }
}

fn walk_isobmff_boxes(
    file: &mut File,
    start: u64,
    end: u64,
    zone: &TimeZone,
    out: &mut Vec<Timestamp>,
    depth: usize,
) {
    if depth > 10 {
        return;
    }

    let mut pos = start;
    while pos + 8 <= end {
        if file.seek(SeekFrom::Start(pos)).is_err() {
            break;
        }
        let mut head = [0u8; 8];
        if file.read_exact(&mut head).is_err() {
            break;
        }

        let size32 = u32::from_be_bytes([head[0], head[1], head[2], head[3]]) as u64;
        let box_type = &head[4..8];
        let (box_size, header_len) = if size32 == 1 {
            let mut size64_buf = [0u8; 8];
            if file.read_exact(&mut size64_buf).is_err() {
                break;
            }
            (u64::from_be_bytes(size64_buf), 16u64)
        } else if size32 == 0 {
            (end - pos, 8u64)
        } else {
            (size32, 8u64)
        };

        let Some(content_end) = pos.checked_add(box_size).filter(|value| *value <= end) else {
            break;
        };
        if box_size < header_len {
            break;
        }
        let content_start = pos + header_len;

        match box_type {
            b"moov" | b"trak" | b"mdia" | b"minf" | b"stbl" | b"udta" | b"ilst" => {
                walk_isobmff_boxes(file, content_start, content_end, zone, out, depth + 1);
            }
            b"meta" => {
                let children = if metadata_box(file, content_start, content_end)
                    .is_some_and(|(kind, _, _)| matches!(&kind, b"hdlr" | b"keys" | b"ilst"))
                {
                    content_start
                } else {
                    content_start + 4
                };
                scan_keyed_metadata(file, children, content_end, zone, out);
                walk_isobmff_boxes(file, children, content_end, zone, out, depth + 1);
            }
            b"mvhd" | b"mdhd" => {
                if let Some(ts) = parse_mvhd_mdhd_creation(file, content_start, content_end, zone) {
                    out.push(ts);
                }
            }
            b"\xa9day" => {
                if let Some(ts) = parse_apple_ilst_date(file, content_start, content_end, zone) {
                    out.push(ts);
                }
            }
            b"----" => {
                if let Some(ts) =
                    parse_apple_custom_key_date(file, content_start, content_end, zone)
                {
                    out.push(ts);
                }
            }
            _ => {}
        }

        pos = content_end;
    }
}

// Metadata keys are 1-based indexes into numeric ilst entries. Only the small
// creation-date value is read; media payload and unrelated metadata are skipped.
fn metadata_box(file: &mut File, pos: u64, end: u64) -> Option<([u8; 4], u64, u64)> {
    if pos.checked_add(8)? > end {
        return None;
    }
    file.seek(SeekFrom::Start(pos)).ok()?;
    let mut header = [0; 8];
    file.read_exact(&mut header).ok()?;
    let size = u32::from_be_bytes(header[..4].try_into().ok()?);
    let (size, header_len) = match size {
        0 => (end - pos, 8),
        1 => {
            let mut wide = [0; 8];
            file.read_exact(&mut wide).ok()?;
            (u64::from_be_bytes(wide), 16)
        }
        size => (u64::from(size), 8),
    };
    let next = pos.checked_add(size)?;
    if size < header_len || next > end {
        return None;
    }
    Some((header[4..].try_into().ok()?, pos + header_len, next))
}

fn scan_keyed_metadata(
    file: &mut File,
    start: u64,
    end: u64,
    zone: &TimeZone,
    out: &mut Vec<Timestamp>,
) {
    let mut pos = start;
    let mut keys = BTreeSet::new();
    let mut lists = Vec::new();
    while let Some((kind, content, next)) = metadata_box(file, pos, end) {
        if &kind == b"keys" && next >= content + 8 {
            if file.seek(SeekFrom::Start(content + 4)).is_err() {
                return;
            }
            let mut count = [0; 4];
            if file.read_exact(&mut count).is_err() {
                return;
            }
            let count = u32::from_be_bytes(count);
            if count > 4096 {
                return;
            }
            let mut entry = content + 8;
            for index in 1..=count {
                let Some((namespace, text_start, text_end)) = metadata_box(file, entry, next)
                else {
                    return;
                };
                let len = text_end - text_start;
                if &namespace == b"mdta" && len <= 128 {
                    if file.seek(SeekFrom::Start(text_start)).is_err() {
                        return;
                    }
                    let mut name = vec![0; len as usize];
                    if file.read_exact(&mut name).is_err() {
                        return;
                    }
                    if name == b"com.apple.quicktime.creationdate" || name == b"creation_time" {
                        keys.insert(index);
                    }
                }
                entry = text_end;
            }
        } else if &kind == b"ilst" {
            lists.push((content, next));
        }
        pos = next;
    }
    for (start, end) in lists {
        let mut pos = start;
        while let Some((kind, content, next)) = metadata_box(file, pos, end) {
            if keys.contains(&u32::from_be_bytes(kind))
                && let Some(stamp) = parse_apple_ilst_date(file, content, next, zone)
            {
                out.push(stamp);
            }
            pos = next;
        }
    }
}

fn parse_mvhd_mdhd_creation(
    file: &mut File,
    start: u64,
    end: u64,
    zone: &TimeZone,
) -> Option<Timestamp> {
    if end < start + 12 || file.seek(SeekFrom::Start(start)).is_err() {
        return None;
    }

    let mut ver_buf = [0u8; 4];
    file.read_exact(&mut ver_buf).ok()?;
    let version = ver_buf[0];

    let qt_secs = if version == 0 {
        let mut time_buf = [0u8; 4];
        file.read_exact(&mut time_buf).ok()?;
        u32::from_be_bytes(time_buf) as i64
    } else if version == 1 {
        let mut time_buf = [0u8; 8];
        file.read_exact(&mut time_buf).ok()?;
        u64::from_be_bytes(time_buf) as i64
    } else {
        return None;
    };

    if qt_secs <= 0 {
        return None;
    }

    let unix_secs = qt_secs - QUICKTIME_EPOCH_OFFSET_SECS;
    if unix_secs <= 0 {
        return None;
    }

    let utc_dt = jiff::Timestamp::new(unix_secs, 0).ok()?;
    utc_stamp(utc_dt, zone)
}

fn parse_apple_ilst_date(
    file: &mut File,
    start: u64,
    end: u64,
    zone: &TimeZone,
) -> Option<Timestamp> {
    let mut pos = start;
    while pos + 8 <= end {
        if file.seek(SeekFrom::Start(pos)).is_err() {
            break;
        }
        let mut head = [0u8; 8];
        if file.read_exact(&mut head).is_err() {
            break;
        }
        let size = u32::from_be_bytes([head[0], head[1], head[2], head[3]]) as u64;
        let box_type = &head[4..8];
        let Some(box_end) = pos.checked_add(size).filter(|value| *value <= end) else {
            break;
        };
        if size < 8 {
            break;
        }

        if box_type == b"data" && size >= 16 {
            let data_len = (size - 16) as usize;
            if data_len > 128 {
                break;
            }
            if file.seek(SeekFrom::Start(pos + 16)).is_err() {
                break;
            }
            let mut text_buf = vec![0u8; data_len];
            if file.read_exact(&mut text_buf).is_ok()
                && let Ok(text) = std::str::from_utf8(&text_buf)
                && let Some(ts) = parse_iso_or_wall_string(text.trim(), zone)
            {
                return Some(ts);
            }
        }
        pos = box_end;
    }
    None
}

fn parse_apple_custom_key_date(
    file: &mut File,
    start: u64,
    end: u64,
    zone: &TimeZone,
) -> Option<Timestamp> {
    let mut pos = start;
    let mut has_quicktime_mean = false;
    let mut is_creationdate_tag = false;
    let mut data_str = None;

    while pos + 8 <= end {
        if file.seek(SeekFrom::Start(pos)).is_err() {
            break;
        }
        let mut head = [0u8; 8];
        if file.read_exact(&mut head).is_err() {
            break;
        }
        let size = u32::from_be_bytes([head[0], head[1], head[2], head[3]]) as u64;
        let box_type = &head[4..8];
        let Some(box_end) = pos.checked_add(size).filter(|value| *value <= end) else {
            break;
        };
        if size < 8 {
            break;
        }

        if box_type == b"mean" && size >= 12 {
            let mean_len = (size - 12) as usize;
            if mean_len > 128 {
                break;
            }
            if file.seek(SeekFrom::Start(pos + 12)).is_ok() {
                let mut mean_buf = vec![0u8; mean_len];
                if file.read_exact(&mut mean_buf).is_ok() {
                    let mean = String::from_utf8_lossy(&mean_buf);
                    if mean.trim() == "com.apple.quicktime" {
                        has_quicktime_mean = true;
                    }
                }
            }
        } else if box_type == b"name" && size >= 12 {
            let name_len = (size - 12) as usize;
            if name_len > 128 {
                break;
            }
            if file.seek(SeekFrom::Start(pos + 12)).is_ok() {
                let mut name_buf = vec![0u8; name_len];
                if file.read_exact(&mut name_buf).is_ok() {
                    let name = String::from_utf8_lossy(&name_buf);
                    if name.trim().eq_ignore_ascii_case("creationdate") {
                        is_creationdate_tag = true;
                    }
                }
            }
        } else if box_type == b"data" && size >= 16 {
            let data_len = (size - 16) as usize;
            if data_len <= 128 && file.seek(SeekFrom::Start(pos + 16)).is_ok() {
                let mut text_buf = vec![0u8; data_len];
                if file.read_exact(&mut text_buf).is_ok()
                    && let Ok(text) = std::str::from_utf8(&text_buf)
                {
                    data_str = Some(text.trim().to_owned());
                }
            }
        }
        pos = box_end;
    }

    if has_quicktime_mean
        && is_creationdate_tag
        && let Some(text) = data_str
    {
        parse_iso_or_wall_string(&text, zone)
    } else {
        None
    }
}

fn parse_iso_or_wall_string(text: &str, zone: &TimeZone) -> Option<Timestamp> {
    if let Ok(instant) = parse_rfc3339(text) {
        if placeholder(instant.datetime()) {
            return None;
        }
        return utc_stamp(instant.timestamp(), zone);
    }
    for format in ["%Y-%m-%dT%H:%M:%S%z", "%Y-%m-%d %H:%M:%S%z"] {
        if let Ok(instant) = strptime_fixed(format, text) {
            if placeholder(instant.datetime()) {
                return None;
            }
            return utc_stamp(instant.timestamp(), zone);
        }
    }
    for format in ["%Y-%m-%dT%H:%M:%S", "%Y-%m-%d %H:%M:%S"] {
        if let Ok(naive) = DateTime::strptime(format, text) {
            if leap_second(text, naive) {
                return None;
            }
            return wall_stamp(naive, zone);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_york() -> TimeZone {
        solstone_core_time::zone("America/New_York").unwrap()
    }

    fn denver() -> TimeZone {
        solstone_core_time::zone("America/Denver").unwrap()
    }
    use std::fs::{self, File, FileTimes};
    use std::time::{Duration, UNIX_EPOCH};

    /// Build a minimal valid JPEG image containing an APP1 Exif marker with TIFF data.
    /// Byte layout:
    /// - 0xFF 0xD8 (SOI)
    /// - 0xFF 0xE1 (APP1 marker)
    /// - 2 bytes length (big endian)
    /// - b"Exif\0\0" (header)
    /// - TIFF header: "II" (little-endian), 0x002A, offset to IFD0 (0x00000008)
    /// - IFD0: 1 entry: Exif IFD pointer (0x8769) -> offset 26
    /// - Exif IFD at 26: Tag 0x0132 (ModifyDate, ignored), Tag 0x9003 (DateTimeOriginal),
    ///   and optional Tag 0x9011 (OffsetTimeOriginal).
    /// - 0xFF 0xD9 (EOI)
    fn make_jpeg_app1(datetime_str: &str, offset_str: Option<&str>) -> Vec<u8> {
        let mut data = vec![0xFF, 0xD8];
        data.extend_from_slice(&[0xFF, 0xE1]);

        let mut tiff = Vec::new();
        tiff.extend_from_slice(&[b'I', b'I', 0x2A, 0x00, 0x08, 0x00, 0x00, 0x00]);

        let exif_ifd_offset = 26u32;
        tiff.extend_from_slice(&1u16.to_le_bytes()); // 1 entry in IFD0
        tiff.extend_from_slice(&0x8769u16.to_le_bytes());
        tiff.extend_from_slice(&4u16.to_le_bytes()); // LONG
        tiff.extend_from_slice(&1u32.to_le_bytes()); // count 1
        tiff.extend_from_slice(&exif_ifd_offset.to_le_bytes());
        tiff.extend_from_slice(&0u32.to_le_bytes()); // next IFD offset 0

        let dt_bytes = format!("{datetime_str}\0").into_bytes();
        let num_entries = if offset_str.is_some() { 3u16 } else { 2u16 };
        let ifd_len = 2 + (num_entries as usize * 12) + 4;
        let mut string_data_offset = (26 + ifd_len) as u32;

        tiff.extend_from_slice(&num_entries.to_le_bytes());

        // Entry 1: Tag 0x0132 (ModifyDate) - to verify it is completely ignored
        let mod_bytes = b"1999:01:01 00:00:00\0";
        tiff.extend_from_slice(&0x0132u16.to_le_bytes());
        tiff.extend_from_slice(&2u16.to_le_bytes()); // ASCII
        tiff.extend_from_slice(&(mod_bytes.len() as u32).to_le_bytes());
        tiff.extend_from_slice(&string_data_offset.to_le_bytes());
        string_data_offset += mod_bytes.len() as u32;

        // Entry 2: Tag 0x9003 (DateTimeOriginal)
        tiff.extend_from_slice(&0x9003u16.to_le_bytes());
        tiff.extend_from_slice(&2u16.to_le_bytes()); // ASCII
        tiff.extend_from_slice(&(dt_bytes.len() as u32).to_le_bytes());
        tiff.extend_from_slice(&string_data_offset.to_le_bytes());
        string_data_offset += dt_bytes.len() as u32;

        let off_bytes = offset_str.map(|s| format!("{s}\0").into_bytes());
        if let Some(ref ob) = off_bytes {
            // Entry 3: Tag 0x9011 (OffsetTimeOriginal)
            tiff.extend_from_slice(&0x9011u16.to_le_bytes());
            tiff.extend_from_slice(&2u16.to_le_bytes()); // ASCII
            tiff.extend_from_slice(&(ob.len() as u32).to_le_bytes());
            tiff.extend_from_slice(&string_data_offset.to_le_bytes());
        }
        tiff.extend_from_slice(&0u32.to_le_bytes()); // next IFD 0

        tiff.extend_from_slice(mod_bytes);
        tiff.extend_from_slice(&dt_bytes);
        if let Some(ref ob) = off_bytes {
            tiff.extend_from_slice(ob);
        }

        let app1_len = (2 + 6 + tiff.len()) as u16;
        data.extend_from_slice(&app1_len.to_be_bytes());
        data.extend_from_slice(b"Exif\0\0");
        data.extend_from_slice(&tiff);
        data.extend_from_slice(&[0xFF, 0xD9]); // EOI
        data
    }

    /// Build a JPEG APP1 segment with both DateTimeOriginal (0x9003) and DateTimeDigitized (0x9004).
    fn make_jpeg_app1_with_two_dates(orig_str: &str, digit_str: &str) -> Vec<u8> {
        let mut data = vec![0xFF, 0xD8];
        data.extend_from_slice(&[0xFF, 0xE1]);

        let mut tiff = Vec::new();
        tiff.extend_from_slice(&[b'I', b'I', 0x2A, 0x00, 0x08, 0x00, 0x00, 0x00]);

        let exif_ifd_offset = 26u32;
        tiff.extend_from_slice(&1u16.to_le_bytes()); // 1 entry in IFD0
        tiff.extend_from_slice(&0x8769u16.to_le_bytes());
        tiff.extend_from_slice(&4u16.to_le_bytes());
        tiff.extend_from_slice(&1u32.to_le_bytes());
        tiff.extend_from_slice(&exif_ifd_offset.to_le_bytes());
        tiff.extend_from_slice(&0u32.to_le_bytes());

        let orig_bytes = format!("{orig_str}\0").into_bytes();
        let digit_bytes = format!("{digit_str}\0").into_bytes();
        let ifd_len = 2 + (2 * 12) + 4;
        let mut string_data_offset = (26 + ifd_len) as u32;

        tiff.extend_from_slice(&2u16.to_le_bytes()); // 2 entries in Exif IFD

        // Entry 1: Tag 0x9003 (DateTimeOriginal)
        tiff.extend_from_slice(&0x9003u16.to_le_bytes());
        tiff.extend_from_slice(&2u16.to_le_bytes()); // ASCII
        tiff.extend_from_slice(&(orig_bytes.len() as u32).to_le_bytes());
        tiff.extend_from_slice(&string_data_offset.to_le_bytes());
        string_data_offset += orig_bytes.len() as u32;

        // Entry 2: Tag 0x9004 (DateTimeDigitized)
        tiff.extend_from_slice(&0x9004u16.to_le_bytes());
        tiff.extend_from_slice(&2u16.to_le_bytes()); // ASCII
        tiff.extend_from_slice(&(digit_bytes.len() as u32).to_le_bytes());
        tiff.extend_from_slice(&string_data_offset.to_le_bytes());
        tiff.extend_from_slice(&0u32.to_le_bytes());

        tiff.extend_from_slice(&orig_bytes);
        tiff.extend_from_slice(&digit_bytes);

        let app1_len = (2 + 6 + tiff.len()) as u16;
        data.extend_from_slice(&app1_len.to_be_bytes());
        data.extend_from_slice(b"Exif\0\0");
        data.extend_from_slice(&tiff);
        data.extend_from_slice(&[0xFF, 0xD9]);
        data
    }

    /// Build a minimal MP4 with moov -> mvhd containing creation_time.
    /// Byte layout:
    /// - ftyp box: 16 bytes (size: 16, type: "ftyp", major_brand: "mp42", minor_version: 0)
    /// - moov box containing mvhd:
    ///   - mvhd: 28 bytes (size: 28, type: "mvhd", version: 0, flags: 0,
    ///     creation_time: 4 bytes, mod_time: 4 bytes, timescale: 1000, duration: 0)
    fn make_mp4_mvhd(creation_secs_since_1904: u32) -> Vec<u8> {
        let mut data = Vec::new();
        // ftyp box
        data.extend_from_slice(&16u32.to_be_bytes());
        data.extend_from_slice(b"ftypmp42");
        data.extend_from_slice(&0u32.to_be_bytes());

        // mvhd box
        let mut mvhd = Vec::new();
        mvhd.extend_from_slice(&28u32.to_be_bytes());
        mvhd.extend_from_slice(b"mvhd");
        mvhd.push(0); // version 0
        mvhd.extend_from_slice(&[0, 0, 0]); // flags
        mvhd.extend_from_slice(&creation_secs_since_1904.to_be_bytes());
        mvhd.extend_from_slice(&999999u32.to_be_bytes()); // mod time (ignored)
        mvhd.extend_from_slice(&1000u32.to_be_bytes()); // timescale
        mvhd.extend_from_slice(&0u32.to_be_bytes()); // duration

        // moov box
        let moov_len = (8 + mvhd.len()) as u32;
        data.extend_from_slice(&moov_len.to_be_bytes());
        data.extend_from_slice(b"moov");
        data.extend_from_slice(&mvhd);
        data
    }

    /// Build a minimal MP4/M4A with moov -> mvhd (version 1, 64-bit creation_time).
    /// Byte layout:
    /// - ftyp box: 16 bytes (size: 16, type: "ftyp", major_brand: "M4A ", minor_version: 0)
    /// - moov box:
    ///   - mvhd: 36 bytes (size: 36, type: "mvhd", version: 1, flags: 0,
    ///     creation_time: 8 bytes u64, mod_time: 8 bytes u64, timescale: 4 bytes u32, duration: 8 bytes u64)
    fn make_mp4_mvhd_v1(creation_secs_since_1904: u64) -> Vec<u8> {
        let mut data = Vec::new();
        // ftyp box
        data.extend_from_slice(&16u32.to_be_bytes());
        data.extend_from_slice(b"ftypM4A ");
        data.extend_from_slice(&0u32.to_be_bytes());

        // mvhd box version 1
        let mut mvhd = Vec::new();
        let mvhd_len = 40u32;
        mvhd.extend_from_slice(&mvhd_len.to_be_bytes());
        mvhd.extend_from_slice(b"mvhd");
        mvhd.push(1); // version 1
        mvhd.extend_from_slice(&[0, 0, 0]); // flags
        mvhd.extend_from_slice(&creation_secs_since_1904.to_be_bytes()); // u64 creation time
        mvhd.extend_from_slice(&999999u64.to_be_bytes()); // u64 mod time (ignored)
        mvhd.extend_from_slice(&1000u32.to_be_bytes()); // timescale
        mvhd.extend_from_slice(&0u64.to_be_bytes()); // duration

        let moov_len = (8 + mvhd.len()) as u32;
        data.extend_from_slice(&moov_len.to_be_bytes());
        data.extend_from_slice(b"moov");
        data.extend_from_slice(&mvhd);
        data
    }

    /// Build a minimal M4A with Apple custom key '----' inside moov -> udta -> meta -> ilst.
    /// Byte layout:
    /// - ftyp box: 16 bytes (size 16, type "ftyp", brand "M4A ", version 0)
    /// - moov box:
    ///   - udta box:
    ///     - meta box: header 8 bytes + 4 bytes version/flags (0)
    ///       - ilst box:
    ///         - ---- box:
    ///           - mean box: 8 bytes header + 4 bytes version/flags (0) + mean string
    ///           - name box: 8 bytes header + 4 bytes version/flags (0) + name string
    ///           - data box: 8 bytes header + 4 bytes type (1=UTF-8) + 4 bytes locale (0) + data string
    fn make_m4a_apple_custom_key(mean: &str, name: &str, data_val: &str) -> Vec<u8> {
        let mut mean_box = Vec::new();
        let mean_len = (12 + mean.len()) as u32;
        mean_box.extend_from_slice(&mean_len.to_be_bytes());
        mean_box.extend_from_slice(b"mean");
        mean_box.extend_from_slice(&[0, 0, 0, 0]);
        mean_box.extend_from_slice(mean.as_bytes());

        let mut name_box = Vec::new();
        let name_len = (12 + name.len()) as u32;
        name_box.extend_from_slice(&name_len.to_be_bytes());
        name_box.extend_from_slice(b"name");
        name_box.extend_from_slice(&[0, 0, 0, 0]);
        name_box.extend_from_slice(name.as_bytes());

        let mut data_box = Vec::new();
        let data_len = (16 + data_val.len()) as u32;
        data_box.extend_from_slice(&data_len.to_be_bytes());
        data_box.extend_from_slice(b"data");
        data_box.extend_from_slice(&[0, 0, 0, 1]); // type indicator 1 = UTF-8 text
        data_box.extend_from_slice(&[0, 0, 0, 0]); // locale
        data_box.extend_from_slice(data_val.as_bytes());

        let mut custom_box = Vec::new();
        let custom_len = (8 + mean_box.len() + name_box.len() + data_box.len()) as u32;
        custom_box.extend_from_slice(&custom_len.to_be_bytes());
        custom_box.extend_from_slice(b"----");
        custom_box.extend_from_slice(&mean_box);
        custom_box.extend_from_slice(&name_box);
        custom_box.extend_from_slice(&data_box);

        let mut ilst_box = Vec::new();
        let ilst_len = (8 + custom_box.len()) as u32;
        ilst_box.extend_from_slice(&ilst_len.to_be_bytes());
        ilst_box.extend_from_slice(b"ilst");
        ilst_box.extend_from_slice(&custom_box);

        let mut meta_box = Vec::new();
        let meta_len = (12 + ilst_box.len()) as u32;
        meta_box.extend_from_slice(&meta_len.to_be_bytes());
        meta_box.extend_from_slice(b"meta");
        meta_box.extend_from_slice(&[0, 0, 0, 0]); // version/flags
        meta_box.extend_from_slice(&ilst_box);

        let mut udta_box = Vec::new();
        let udta_len = (8 + meta_box.len()) as u32;
        udta_box.extend_from_slice(&udta_len.to_be_bytes());
        udta_box.extend_from_slice(b"udta");
        udta_box.extend_from_slice(&meta_box);

        let mut moov_box = Vec::new();
        let moov_len = (8 + udta_box.len()) as u32;
        moov_box.extend_from_slice(&moov_len.to_be_bytes());
        moov_box.extend_from_slice(b"moov");
        moov_box.extend_from_slice(&udta_box);

        let mut file_data = Vec::new();
        file_data.extend_from_slice(&16u32.to_be_bytes());
        file_data.extend_from_slice(b"ftypM4A ");
        file_data.extend_from_slice(&0u32.to_be_bytes());
        file_data.extend_from_slice(&moov_box);
        file_data
    }

    #[test]
    fn a_leap_second_wall_clock_names_no_local_second() {
        assert_eq!(
            parse_exif_date_string("2026:06:23 18:30:60", None, &TimeZone::UTC),
            None
        );
        assert_eq!(
            parse_iso_or_wall_string("2026-06-23T18:30:60", &TimeZone::UTC),
            None
        );
        // With its offset it names an instant, read as the second before.
        assert_eq!(
            parse_exif_date_string("2026:06:23 18:30:60", Some("+00:00"), &TimeZone::UTC)
                .unwrap()
                .as_str(),
            "20260623_183059"
        );
    }

    #[test]
    fn epoch_placeholders_are_not_creation_evidence() {
        for date in [
            "0000:01:01 00:00:00",
            "1904:01:01 00:00:00",
            "1970:01:01 00:00:00",
        ] {
            for offset in [None, Some("-07:00")] {
                assert_eq!(parse_exif_date_string(date, offset, &TimeZone::UTC), None);
            }
        }
        assert_eq!(
            parse_iso_or_wall_string("1970-01-01T00:00:00-07:00", &TimeZone::UTC),
            None
        );
        assert!(parse_filename_candidates("19700101_000000.txt", &TimeZone::UTC).is_empty());
    }

    #[test]
    fn headerless_quicktime_metadata_retains_conflicting_evidence() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("voice.m4a");
        fs::write(
            &path,
            include_bytes!("../tests/fixtures/creation-mdta-headerless-conflict.m4a"),
        )
        .unwrap();
        assert_eq!(detect_creation(&path, None, &TimeZone::UTC), None);
    }

    #[test]
    fn apple_keyed_metadata_is_read_and_conflicts_are_rejected() {
        for (bytes, expected) in [
            (
                include_bytes!("../tests/fixtures/creation-mdta-only.m4a").as_slice(),
                Some("20260510_120000"),
            ),
            (
                include_bytes!("../tests/fixtures/creation-mdta-conflict.m4a").as_slice(),
                None,
            ),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("voice.m4a");
            fs::write(&path, bytes).unwrap();
            assert_eq!(
                detect_creation(&path, None, &TimeZone::UTC)
                    .as_ref()
                    .map(Timestamp::as_str),
                expected
            );
        }
    }

    #[test]
    fn modification_offset_does_not_apply_to_creation_time() {
        let mut jpeg = make_jpeg_app1("2026:05:10 23:30:00", Some("-07:00"));
        let at = jpeg
            .windows(2)
            .position(|pair| pair == [0x11, 0x90])
            .unwrap();
        jpeg[at] = 0x10; // OffsetTime belongs to modification time only.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("photo.jpg");
        fs::write(&path, jpeg).unwrap();
        assert_eq!(
            detect_creation(&path, None, &TimeZone::UTC)
                .unwrap()
                .as_str(),
            "20260510_233000"
        );
    }

    #[test]
    fn limitless_range_does_not_hide_a_conflicting_suffix() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("voice.wav");
        fs::write(&path, b"source").unwrap();
        assert_eq!(
            detect_creation(
                &path,
                Some(
                    "limitless_pendant_2026-01-15T03-30-00_to_2026-01-15T04-00-00__20260116_120000.wav"
                ),
                &TimeZone::UTC
            ),
            None
        );
    }

    #[test]
    fn jpeg_exif_creation_detection_ignores_mtime_and_modify_date() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("photo.jpg");
        // DateTimeOriginal: 2026-05-10 14:30:00, ModifyDate: 1999-01-01
        let jpeg_bytes = make_jpeg_app1("2026:05:10 14:30:00", None);
        fs::write(&path, jpeg_bytes).unwrap();

        // Set file mtime to 2024-01-01 so an mtime answer fails
        let old_time = UNIX_EPOCH + Duration::from_secs(1_704_067_200);
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(FileTimes::new().set_modified(old_time))
            .unwrap();

        let detected = detect_creation(&path, Some("photo.jpg"), &TimeZone::UTC);
        assert_eq!(
            detected.as_ref().map(|t| t.as_str()),
            Some("20260510_143000")
        );
    }

    #[test]
    fn mp4_mvhd_creation_detection_ignores_mtime() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("video.mp4");
        // 2026-05-10 12:00:00 UTC = 1_778_414_400 Unix epoch secs
        // + 2_082_844_800 QuickTime epoch offset = 3_861_259_200
        let mp4_bytes = make_mp4_mvhd(3_861_259_200);
        fs::write(&path, mp4_bytes).unwrap();

        // Set mtime to 2023-01-01
        let old_time = UNIX_EPOCH + Duration::from_secs(1_672_531_200);
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(FileTimes::new().set_modified(old_time))
            .unwrap();

        let detected = detect_creation(&path, Some("video.mp4"), &TimeZone::UTC);
        assert_eq!(
            detected.as_ref().map(|t| t.as_str()),
            Some("20260510_120000")
        );
    }

    #[test]
    fn invalid_calendar_date_returns_none() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("2026-02-30_12_00_00.jpg");
        fs::write(&path, b"dummy").unwrap();

        assert_eq!(
            detect_creation(&path, Some("2026-02-30_12_00_00.jpg"), &TimeZone::UTC),
            None
        );
    }

    #[test]
    fn date_only_filename_returns_none() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("2026-05-10.jpg");
        fs::write(&path, b"dummy").unwrap();

        assert_eq!(
            detect_creation(&path, Some("2026-05-10.jpg"), &TimeZone::UTC),
            None
        );
    }

    #[test]
    fn two_different_timestamps_in_filename_returns_none() {
        let temp = tempfile::tempdir().unwrap();
        let name = "20260101_100000_20260102_120000.jpg";
        let path = temp.path().join(name);
        fs::write(&path, b"dummy").unwrap();

        assert_eq!(detect_creation(&path, Some(name), &TimeZone::UTC), None);
    }

    #[test]
    fn limitless_pendant_prior_day_in_new_york() {
        let temp = tempfile::tempdir().unwrap();
        let name = "limitless_pendant_2026-01-15T03-30-00_to_2026-01-15T04-00-00.wav";
        let path = temp.path().join(name);
        fs::write(&path, b"dummy").unwrap();

        // 2026-01-15 03:30:00 UTC in America/New_York (UTC-5) is 2026-01-14 22:30:00
        let detected = detect_creation(&path, Some(name), &new_york());
        assert_eq!(
            detected.as_ref().map(|t| t.as_str()),
            Some("20260114_223000")
        );
    }

    #[test]
    fn spring_forward_gap_in_new_york_is_not_a_detection() {
        let temp = tempfile::tempdir().unwrap();
        // 2026-03-08 02:30:00 does not exist in America/New_York (clocks jump 02:00 -> 03:00)
        let name = "2026-03-08 02:30:00.jpg";
        let path = temp.path().join("source.jpg");
        fs::write(&path, b"dummy").unwrap();

        assert_eq!(detect_creation(&path, Some(name), &new_york()), None);
    }

    #[test]
    fn zone_free_wall_time_preserved() {
        let temp = tempfile::tempdir().unwrap();
        let name = "2026-06-15 15:45:00.jpg";
        let path = temp.path().join("source.jpg");
        fs::write(&path, b"dummy").unwrap();

        let detected = detect_creation(&path, Some(name), &new_york());
        assert_eq!(
            detected.as_ref().map(|t| t.as_str()),
            Some("20260615_154500")
        );
    }

    #[test]
    fn offset_conversion_to_owner_zone() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("photo.jpg");
        // 2026:06:15 15:45:00 with offset +02:00 -> 13:45:00 UTC
        let jpeg_bytes = make_jpeg_app1("2026:06:15 15:45:00", Some("+02:00"));
        fs::write(&path, jpeg_bytes).unwrap();

        let detected = detect_creation(&path, Some("photo.jpg"), &TimeZone::UTC);
        assert_eq!(
            detected.as_ref().map(|t| t.as_str()),
            Some("20260615_134500")
        );
    }

    #[test]
    fn metadata_disagreement_with_filename_returns_none() {
        let temp = tempfile::tempdir().unwrap();
        let name = "20260101_100000.jpg";
        let path = temp.path().join(name);
        // Exif contains DateTimeOriginal: 2026-01-01 10:00:00 and DateTimeDigitized: 2026-01-02 12:00:00
        let jpeg_bytes =
            make_jpeg_app1_with_two_dates("2026:01:01 10:00:00", "2026:01:02 12:00:00");
        fs::write(&path, jpeg_bytes).unwrap();

        assert_eq!(detect_creation(&path, Some(name), &TimeZone::UTC), None);
    }

    #[test]
    fn equivalent_encodings_collapse_to_single_stamp() {
        let temp = tempfile::tempdir().unwrap();
        let name = "20260615_134500.jpg";
        let path = temp.path().join(name);
        // Exif: 2026:06:15 15:45:00 +02:00 -> 13:45:00 UTC
        // Filename: 20260615_134500 -> 13:45:00 UTC
        let jpeg_bytes = make_jpeg_app1("2026:06:15 15:45:00", Some("+02:00"));
        fs::write(&path, jpeg_bytes).unwrap();

        let detected = detect_creation(&path, Some(name), &TimeZone::UTC);
        assert_eq!(
            detected.as_ref().map(|t| t.as_str()),
            Some("20260615_134500")
        );
    }

    #[test]
    fn limitless_dst_march_8_in_new_york() {
        let temp = tempfile::tempdir().unwrap();
        let name = "limitless_pendant_2026-03-08T07-00-00_to_2026-03-08T08-00-00.m4a";
        let path = temp.path().join(name);
        fs::write(&path, b"dummy").unwrap();

        // 2026-03-08 07:00:00 UTC in America/New_York (spring forward to EDT UTC-4 at 2:00 AM) is 2026-03-08 03:00:00 EDT
        let detected = detect_creation(&path, Some(name), &new_york());
        assert_eq!(
            detected.as_ref().map(|t| t.as_str()),
            Some("20260308_030000")
        );
    }

    #[test]
    fn mp4_mvhd_v1_u64_creation_time() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("video.m4a");
        // 2026-05-10 12:00:00 UTC = 3_861_259_200 QuickTime epoch secs
        let m4a_bytes = make_mp4_mvhd_v1(3_861_259_200);
        fs::write(&path, m4a_bytes).unwrap();

        let detected = detect_creation(&path, Some("video.m4a"), &TimeZone::UTC);
        assert_eq!(
            detected.as_ref().map(|t| t.as_str()),
            Some("20260510_120000")
        );
    }

    #[test]
    fn apple_custom_key_creationdate_vs_modificationdate() {
        let temp = tempfile::tempdir().unwrap();

        // Valid creationdate tag
        let path_create = temp.path().join("created.m4a");
        let bytes_create = make_m4a_apple_custom_key(
            "com.apple.quicktime",
            "creationdate",
            "2026-05-10T12:00:00Z",
        );
        fs::write(&path_create, bytes_create).unwrap();

        let detected = detect_creation(&path_create, Some("created.m4a"), &TimeZone::UTC);
        assert_eq!(
            detected.as_ref().map(|t| t.as_str()),
            Some("20260510_120000")
        );

        // Modificationdate tag should be ignored
        let path_mod = temp.path().join("modified.m4a");
        let bytes_mod = make_m4a_apple_custom_key(
            "com.apple.quicktime",
            "modificationdate",
            "2026-05-10T12:00:00Z",
        );
        fs::write(&path_mod, bytes_mod).unwrap();

        let detected_mod = detect_creation(&path_mod, Some("modified.m4a"), &TimeZone::UTC);
        assert_eq!(detected_mod, None);
    }

    #[test]
    fn placeholder_timestamps_ignored() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("photo.jpg");
        let jpeg_bytes = make_jpeg_app1("0000:00:00 00:00:00", None);
        fs::write(&path, jpeg_bytes).unwrap();

        assert_eq!(
            detect_creation(&path, Some("photo.jpg"), &TimeZone::UTC),
            None
        );
    }

    #[test]
    fn malformed_bytes_do_not_panic() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("corrupt.jpg");
        fs::write(&path, [0xFF, 0xD8, 0xFF, 0xE1, 0x00, 0x10, 0x12, 0x34]).unwrap();

        assert_eq!(
            detect_creation(&path, Some("corrupt.jpg"), &TimeZone::UTC),
            None
        );
    }

    #[test]
    fn quicktime_epoch_zero_ignored() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("zero.mp4");
        let mp4_bytes = make_mp4_mvhd(0);
        fs::write(&path, mp4_bytes).unwrap();

        assert_eq!(
            detect_creation(&path, Some("zero.mp4"), &TimeZone::UTC),
            None
        );
    }

    #[test]
    fn metadata_cannot_erase_conflicting_filename_candidates() {
        let temporary = tempfile::tempdir().unwrap();
        let name = "2026-06-23_18_30_00_2026-06-24_18_30_00.jpg";
        let path = temporary.path().join(name);
        fs::write(&path, make_jpeg_app1("2026:06:23 18:30:00", Some("-06:00"))).unwrap();
        assert_eq!(detect_creation(&path, Some(name), &denver()), None);
    }

    #[test]
    fn conflicting_utc_instants_in_a_repeated_hour_remain_a_conflict() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/creation-conflicting-fold.m4a");
        assert_eq!(detect_creation(&path, None, &denver()), None);
    }

    #[test]
    fn an_owner_instant_names_either_occurrence_of_a_repeated_hour() {
        let zone = &denver();
        let first = owner_instant("2025-11-02T01:30:00-06:00", zone).unwrap();
        let second = owner_instant("2025-11-02T01:30:00-07:00", zone).unwrap();
        assert_eq!(first.as_str(), "20251102_013000");
        assert_eq!(second.as_str(), "20251102_013000");
        assert_eq!(
            second.source_instant().unwrap() - first.source_instant().unwrap(),
            jiff::SignedDuration::from_hours(1)
        );
        // Another offset names the same instant on the journal's own clock.
        let utc = owner_instant("2025-07-15T14:03:22Z", zone).unwrap();
        assert_eq!(utc.as_str(), "20250715_080322");
        for raw in ["20251102_013000", "2025-11-02T01:30:00", "2025-11-02"] {
            assert_eq!(owner_instant(raw, zone), None, "{raw}");
        }
    }

    #[test]
    fn malformed_exif_text_and_offset_are_not_local_time_guesses() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("photo.jpg");
        for (date, offset) in [
            ("202é:06:23 18:30:00", None),
            ("2026:06:23 18:30:00", Some("+01:99")),
            ("2026:06:23 18:30:00", Some("invalid")),
        ] {
            fs::write(&path, make_jpeg_app1(date, offset)).unwrap();
            assert_eq!(detect_creation(&path, None, &TimeZone::UTC), None);
            assert_eq!(
                detect_photo_capture_moment(&path, &TimeZone::UTC, None),
                None
            );
        }
    }

    #[test]
    fn photo_capture_moment_keeps_capture_zone_day_and_clock() {
        let temporary = tempfile::tempdir().unwrap();
        // Taken 2026-11-12 08:30 in Tokyo (+09:00): still 2026-11-11 16:30 in Denver (-07:00).
        // The photo must land on 2026-11-12 at 08:30 with offset +32400, not Denver's Nov 11.
        let path = temporary.path().join("PXL_20261111_233000.jpg");
        fs::write(&path, make_jpeg_app1("2026:11:12 08:30:00", Some("+09:00"))).unwrap();
        let moment = detect_photo_capture_moment(&path, &denver(), None).expect("Tokyo photo");
        assert_eq!(moment.stamp.as_str(), "20261112_083000");
        assert_eq!(moment.utc_offset_seconds, Some(32400));
        assert_eq!(moment.tz, None);
    }

    #[test]
    fn photo_capture_moment_prefers_date_time_original_over_one_second_digitized_skew() {
        // Build an Exif TIFF block where DateTimeOriginal is 2026:11:12 21:14:00 (+09:00)
        // and DateTimeDigitized is 2026:11:12 21:14:01 (+09:00).
        let mut tiff = Vec::new();
        tiff.extend_from_slice(b"II*\0");
        tiff.extend_from_slice(&8u32.to_le_bytes());
        tiff.extend_from_slice(&1u16.to_le_bytes());
        tiff.extend_from_slice(&0x8769u16.to_le_bytes());
        tiff.extend_from_slice(&4u16.to_le_bytes());
        tiff.extend_from_slice(&1u32.to_le_bytes());
        tiff.extend_from_slice(&26u32.to_le_bytes());
        tiff.extend_from_slice(&0u32.to_le_bytes());
        assert_eq!(tiff.len(), 26);

        let val_start = 26 + 2 + 4 * 12 + 4; // 80
        let s1 = b"2026:11:12 21:14:00\0";
        let s2 = b"2026:11:12 21:14:01\0";
        let off = b"+09:00\0";
        let o1 = val_start as u32;
        let o2 = o1 + s1.len() as u32;
        let o3 = o2 + s2.len() as u32;
        let o4 = o3 + off.len() as u32;

        tiff.extend_from_slice(&4u16.to_le_bytes());
        for (tag, len, ptr) in [
            (0x9003u16, s1.len() as u32, o1),
            (0x9004u16, s2.len() as u32, o2),
            (0x9011u16, off.len() as u32, o3),
            (0x9012u16, off.len() as u32, o4),
        ] {
            tiff.extend_from_slice(&tag.to_le_bytes());
            tiff.extend_from_slice(&2u16.to_le_bytes());
            tiff.extend_from_slice(&len.to_le_bytes());
            tiff.extend_from_slice(&ptr.to_le_bytes());
        }
        tiff.extend_from_slice(&0u32.to_le_bytes());
        tiff.extend_from_slice(s1);
        tiff.extend_from_slice(s2);
        tiff.extend_from_slice(off);
        tiff.extend_from_slice(off);

        let moment = exif_photo_capture_moment(&tiff, &denver()).expect("original wins");
        assert_eq!(moment.stamp.as_str(), "20261112_211400");
        assert_eq!(moment.utc_offset_seconds, Some(32400));
    }

    #[test]
    fn client_captured_at_preserves_explicit_offset_and_folds_z() {
        let tokyo = parse_client_captured_at("2026-11-12T08:30:00+09:00", &denver()).unwrap();
        assert_eq!(tokyo.stamp.as_str(), "20261112_083000");
        assert_eq!(tokyo.utc_offset_seconds, Some(32400));

        let london = parse_client_captured_at("2026-11-12T08:30:00+00:00", &denver()).unwrap();
        assert_eq!(london.stamp.as_str(), "20261112_083000");
        assert_eq!(london.utc_offset_seconds, Some(0));

        let utc_only = parse_client_captured_at("2026-11-12T08:30:00Z", &denver()).unwrap();
        assert_eq!(utc_only.stamp.as_str(), "20261112_013000");
        assert_eq!(utc_only.utc_offset_seconds, None);

        let civil = parse_client_captured_at("2026-11-12T08:30:00", &denver()).unwrap();
        assert_eq!(civil.stamp.as_str(), "20261112_083000");
        assert_eq!(civil.utc_offset_seconds, None);
    }

    #[test]
    fn exif_photo_capture_location_requires_camera_provenance_and_rejects_null_island() {
        use exif::experimental::Writer;
        use exif::{Field, In, Rational, Tag, Value};

        fn build_tiff(fields: Vec<Field>) -> Vec<u8> {
            let mut writer = Writer::new();
            for f in &fields {
                writer.push_field(f);
            }
            let mut cursor = std::io::Cursor::new(Vec::new());
            writer.write(&mut cursor, false).unwrap();
            cursor.into_inner()
        }

        let gps_fields = |lat_deg: u32, lon_deg: u32| -> Vec<Field> {
            vec![
                Field {
                    tag: Tag::GPSLatitudeRef,
                    ifd_num: In::PRIMARY,
                    value: Value::Ascii(vec![b"N".to_vec()]),
                },
                Field {
                    tag: Tag::GPSLatitude,
                    ifd_num: In::PRIMARY,
                    value: Value::Rational(vec![
                        Rational {
                            num: lat_deg,
                            denom: 1,
                        },
                        Rational { num: 39, denom: 1 },
                        Rational {
                            num: 3089,
                            denom: 100,
                        },
                    ]),
                },
                Field {
                    tag: Tag::GPSLongitudeRef,
                    ifd_num: In::PRIMARY,
                    value: Value::Ascii(vec![b"W".to_vec()]),
                },
                Field {
                    tag: Tag::GPSLongitude,
                    ifd_num: In::PRIMARY,
                    value: Value::Rational(vec![
                        Rational {
                            num: lon_deg,
                            denom: 1,
                        },
                        Rational { num: 0, denom: 1 },
                        Rational { num: 0, denom: 1 },
                    ]),
                },
            ]
        };

        // 1. Missing Make/Model -> rejected (downloaded web/meme image).
        let no_cam = build_tiff(gps_fields(39, 105));
        assert!(exif_photo_capture_location(&no_cam).is_none());

        // 2. Camera Make + Model + valid coordinates -> accepted.
        let mut valid_fields = gps_fields(39, 105);
        valid_fields.push(Field {
            tag: Tag::Make,
            ifd_num: In::PRIMARY,
            value: Value::Ascii(vec![b"Apple".to_vec()]),
        });
        valid_fields.push(Field {
            tag: Tag::Model,
            ifd_num: In::PRIMARY,
            value: Value::Ascii(vec![b"iPhone 15 Pro".to_vec()]),
        });
        let valid_tiff = build_tiff(valid_fields);
        let loc = exif_photo_capture_location(&valid_tiff).expect("verified camera GPS");
        assert!((loc.latitude - 39.65858).abs() < 1e-3);
        assert!((loc.longitude - (-105.0)).abs() < 1e-5);
        assert_eq!(loc.camera_make.as_deref(), Some("Apple"));
        assert_eq!(loc.camera_model.as_deref(), Some("iPhone 15 Pro"));

        // 3. Screenshot UserComment -> rejected even with Make.
        let mut shot_fields = gps_fields(39, 105);
        shot_fields.push(Field {
            tag: Tag::Make,
            ifd_num: In::PRIMARY,
            value: Value::Ascii(vec![b"Apple".to_vec()]),
        });
        shot_fields.push(Field {
            tag: Tag::UserComment,
            ifd_num: In::PRIMARY,
            value: Value::Undefined(b"ASCII\0\0\0Screenshot".to_vec(), 0),
        });
        assert!(exif_photo_capture_location(&build_tiff(shot_fields)).is_none());

        // 4. Null island (0, 0) -> rejected.
        let null_fields = vec![
            Field {
                tag: Tag::Make,
                ifd_num: In::PRIMARY,
                value: Value::Ascii(vec![b"Apple".to_vec()]),
            },
            Field {
                tag: Tag::GPSLatitudeRef,
                ifd_num: In::PRIMARY,
                value: Value::Ascii(vec![b"N".to_vec()]),
            },
            Field {
                tag: Tag::GPSLatitude,
                ifd_num: In::PRIMARY,
                value: Value::Rational(vec![
                    Rational { num: 0, denom: 1 },
                    Rational { num: 0, denom: 1 },
                    Rational { num: 0, denom: 1 },
                ]),
            },
            Field {
                tag: Tag::GPSLongitudeRef,
                ifd_num: In::PRIMARY,
                value: Value::Ascii(vec![b"E".to_vec()]),
            },
            Field {
                tag: Tag::GPSLongitude,
                ifd_num: In::PRIMARY,
                value: Value::Rational(vec![
                    Rational { num: 0, denom: 1 },
                    Rational { num: 0, denom: 1 },
                    Rational { num: 0, denom: 1 },
                ]),
            },
        ];
        assert!(exif_photo_capture_location(&build_tiff(null_fields)).is_none());
    }
}
