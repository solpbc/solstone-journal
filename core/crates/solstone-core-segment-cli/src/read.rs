// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use jiff::civil::Time;
use serde_json::{Value, json};
use solstone_core_cli_copy as copy;
use solstone_core_format::segment::{is_date_key, segment_parse};
use solstone_core_segment::{
    DEFAULT_STREAM, DirEntryKind, Segment, SegmentIdentityError, StreamLocation, list_days,
    list_dir_entries, list_segments, list_segments_in, read_stream_record, read_text,
};
use solstone_core_time::{day_key, parse_day};

use crate::index::{SegmentIndexStatus, read_segment_index};
use crate::location::SegmentLocation;

#[derive(Clone, Debug)]
pub(crate) struct Check {
    pub(crate) name: String,
    pub(crate) passed: bool,
    pub(crate) detail: String,
    pub(crate) unverified: bool,
}

pub(crate) fn split_path<'a>(
    value: &'a str,
    locale: &str,
) -> Result<(&'a str, &'a str, &'a str), String> {
    let mut parts = value.split('/');
    let (Some(day), Some(stream), Some(segment), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(copy::render_for(
            locale,
            "journal.cli.segment.path.syntax",
            &[],
        ));
    };
    Ok((day, stream, segment))
}

pub(crate) fn read_marker(path: &Path) -> Result<Option<Value>, String> {
    let marker = path.join("stream.json");
    let text = read_text(&marker, String::new()).map_err(|error| error.to_string())?;
    if text.is_empty() {
        return Ok(None);
    }
    let value: Value = serde_json::from_str(&text).map_err(|error| error.to_string())?;
    Ok(value.is_object().then_some(value))
}

fn marker_stream(marker: &Value) -> Option<&str> {
    marker
        .get("stream")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

pub(crate) fn format_size(size: u64) -> String {
    if size >= 1_000_000 {
        format!("{:.1}M", size as f64 / 1_000_000.0)
    } else if size >= 1_000 {
        format!("{:.1}K", size as f64 / 1_000.0)
    } else {
        format!("{size}B")
    }
}

fn segment_duration(segment: &str) -> u64 {
    segment
        .split_once('_')
        .and_then(|(_, duration)| duration.parse().ok())
        .unwrap_or(0)
}

fn times(segment: &str) -> (Option<String>, Option<String>) {
    let Some(time) = segment_parse(segment) else {
        return (None, None);
    };
    let seconds = time.hour as u64 * 3600 + time.minute as u64 * 60 + time.second as u64;
    let end = (seconds + segment_duration(segment)) % 86_400;
    let start = time_of_day(seconds).expect("parsed time is valid");
    let end = time_of_day(end).expect("bounded time is valid");
    (
        Some(start.strftime("%H:%M:%S").to_string()),
        Some(end.strftime("%H:%M:%S").to_string()),
    )
}

/// The time of day `seconds` after midnight; none for a day or more.
fn time_of_day(seconds: u64) -> Option<Time> {
    let hour = i8::try_from(seconds / 3600).ok()?;
    let minute = i8::try_from(seconds / 60 % 60).ok()?;
    let second = i8::try_from(seconds % 60).ok()?;
    Time::new(hour, minute, second, 0).ok()
}

#[derive(Default)]
struct Stats {
    files: u64,
    talents: u64,
    size: u64,
}

fn stats(path: &Path) -> Stats {
    let mut result = Stats::default();
    collect_stats(path, false, &mut result);
    result
}

fn collect_stats(path: &Path, in_talents: bool, result: &mut Stats) {
    let Ok(entries) = list_dir_entries(path) else {
        return;
    };
    for entry in entries {
        match entry.kind {
            DirEntryKind::File => {
                result.files += 1;
                result.size += std::fs::metadata(&entry.path)
                    .map(|meta| meta.len())
                    .unwrap_or(0);
                if in_talents {
                    result.talents += 1;
                }
            }
            DirEntryKind::Directory => collect_stats(
                &entry.path,
                in_talents || entry.name.to_string_lossy() == "talents",
                result,
            ),
            DirEntryKind::Other => {}
        }
    }
}

fn top_level_files(path: &Path) -> Vec<String> {
    let mut files = list_dir_entries(path)
        .unwrap_or_default()
        .into_iter()
        .filter(|entry| entry.kind == DirEntryKind::File)
        .map(|entry| entry.name.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    files.sort();
    files
}

fn talent_files(path: &Path) -> Vec<String> {
    let mut files = list_dir_entries(&path.join("talents"))
        .unwrap_or_default()
        .into_iter()
        .filter(|entry| entry.kind == DirEntryKind::File)
        .map(|entry| entry.name.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    files.sort();
    files
}

fn events_summary(path: &Path) -> Value {
    let text = read_text(path.join("events.jsonl"), String::new()).unwrap_or_default();
    let mut entries = 0_u64;
    let mut tracts = std::collections::BTreeSet::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        entries += 1;
        if let Ok(value) = serde_json::from_str::<Value>(line)
            && let Some(tract) = value.get("tract").and_then(Value::as_str)
        {
            tracts.insert(tract.to_owned());
        }
    }
    json!({"entries": entries, "tracts": tracts.into_iter().collect::<Vec<_>>()})
}

fn next_day(day: &str) -> Option<String> {
    parse_day(day)
        .and_then(|date| date.tomorrow().ok())
        .map(day_key)
}

fn identity_refusal(locale: &str, error: &SegmentIdentityError) -> String {
    match error {
        SegmentIdentityError::NotUtf8 { path } => format!(
            "{}\n",
            copy::display_for(
                locale,
                "journal.cli.segment.identity.encoding",
                [("path", &(path.display()))]
            )
        ),
        SegmentIdentityError::AmbiguousNamedDefault { path } => format!(
            "{}\n",
            copy::display_for(
                locale,
                "journal.cli.segment.identity.default",
                [("path", &(path.display()))]
            )
        ),
        SegmentIdentityError::DuplicateKey { stream, key } => format!(
            "{}\n",
            copy::display_for(
                locale,
                "journal.cli.segment.identity.duplicate",
                [
                    ("stream", &(format_args!("{stream:?}"))),
                    ("key", &(format_args!("{key:?}")))
                ]
            )
        ),
    }
}

fn require_discovered(
    day: &str,
    segment: &Segment,
    locale: &str,
) -> Result<SegmentLocation, String> {
    SegmentLocation::from_discovered(day, segment).map_err(|error| match error {
        solstone_core_segment::SegmentError::RecordIdentity(identity) => {
            identity_refusal(locale, &identity)
        }
        other => format!(
            "{}\n",
            copy::display_for(
                locale,
                "journal.cli.segment.identity.refused",
                [("detail", &other)]
            )
        ),
    })
}

pub(crate) fn successors(
    journal: &Path,
    day: &str,
    stream: &str,
    segment: &str,
    locale: &str,
) -> Result<Vec<SegmentLocation>, String> {
    let mut result = Vec::new();
    for (scan_day, _) in list_days(journal).unwrap_or_default() {
        for candidate in list_segments(journal, &scan_day).unwrap_or_default() {
            let Ok(Some(marker)) = read_marker(candidate.path()) else {
                continue;
            };
            if marker_stream(&marker) != Some(stream)
                || marker.get("prev_day").and_then(Value::as_str) != Some(day)
                || marker.get("prev_segment").and_then(Value::as_str) != Some(segment)
            {
                continue;
            }
            result.push(require_discovered(&scan_day, &candidate, locale)?);
        }
    }
    result.sort_by_key(|location| location.token());
    Ok(result)
}

fn next_segment(
    journal: &Path,
    day: &str,
    stream: &str,
    segment: &str,
    locale: &str,
) -> Result<Option<SegmentLocation>, String> {
    let days = std::iter::once(day.to_owned()).chain(next_day(day));
    for scan_day in days {
        for candidate in list_segments(journal, &scan_day).unwrap_or_default() {
            let Ok(Some(marker)) = read_marker(candidate.path()) else {
                continue;
            };
            if marker_stream(&marker) == Some(stream)
                && marker.get("prev_day").and_then(Value::as_str) == Some(day)
                && marker.get("prev_segment").and_then(Value::as_str) == Some(segment)
            {
                return Ok(Some(require_discovered(&scan_day, &candidate, locale)?));
            }
        }
    }
    Ok(None)
}

fn is_tail(journal: &Path, day: &str, stream: &str, segment: &str) -> bool {
    read_stream_record(journal, stream)
        .ok()
        .flatten()
        .is_some_and(|state| {
            state.get("last_day").and_then(Value::as_str) == Some(day)
                && state.get("last_segment").and_then(Value::as_str) == Some(segment)
        })
}

/// Resolves a chain pointer through its marker stream instead of assuming that
/// the owner-facing stream token is a directory component.
///
/// The default-stream token names a direct-under-day layout while markers keep
/// the real stream identity.  The physical layout therefore cannot be derived
/// from a chain marker and must be selected explicitly from discovered segments.
fn chain_location(
    journal: &Path,
    day: &str,
    stream: &str,
    segment: &str,
) -> Option<SegmentLocation> {
    list_segments(journal, day)
        .ok()?
        .into_iter()
        .find_map(|candidate| {
            if candidate.key() != segment {
                return None;
            }
            let Ok(Some(marker)) = read_marker(candidate.path()) else {
                return None;
            };
            (marker_stream(&marker) == Some(stream))
                .then(|| require_discovered(day, &candidate, copy::ENGLISH).ok())
                .flatten()
        })
}

enum PreviousLink {
    None,
    Present(String),
    Missing(String),
}

impl PreviousLink {
    fn wire(&self) -> String {
        match self {
            Self::None => "(none)".to_owned(),
            Self::Present(path) => path.clone(),
            Self::Missing(path) => format!("{path} [MISSING]"),
        }
    }

    fn line(&self, locale: &str) -> String {
        match self {
            Self::None => copy::render_for(locale, "journal.cli.segment.inspect.prev.none", &[]),
            Self::Present(path) => copy::display_for(
                locale,
                "journal.cli.segment.inspect.prev",
                [("previous", path)],
            ),
            Self::Missing(path) => copy::display_for(
                locale,
                "journal.cli.segment.inspect.prev.missing",
                [("path", &path)],
            ),
        }
    }
}

fn previous_link(
    journal: &Path,
    location: &SegmentLocation,
    marker: &Value,
    stream: &str,
) -> PreviousLink {
    let Some(previous) = marker.get("prev_segment").and_then(Value::as_str) else {
        return PreviousLink::None;
    };
    let day = marker
        .get("prev_day")
        .and_then(Value::as_str)
        .unwrap_or(&location.day);
    let path = format!("{day}/{stream}/{previous}");
    match chain_location(journal, day, stream, previous) {
        Some(previous_location) if previous_location.path.is_dir() => PreviousLink::Present(path),
        _ => PreviousLink::Missing(path),
    }
}

enum ForwardResolution {
    Successor(SegmentLocation),
    Tail,
    Missing,
}

fn capture_key<'a>(day: &'a str, segment: &str) -> Option<(&'a str, u8, u8, u8)> {
    if !is_date_key(day) {
        return None;
    }
    let time = segment_parse(segment)?;
    Some((day, time.hour, time.minute, time.second))
}

fn resolve_forward(
    journal: &Path,
    location: &SegmentLocation,
    stream: &str,
    marker: Option<&Value>,
    locale: &str,
) -> Result<ForwardResolution, String> {
    if let Some(next) = next_segment(journal, &location.day, stream, &location.segment, locale)? {
        return Ok(ForwardResolution::Successor(next));
    }
    if is_tail(journal, &location.day, stream, &location.segment) {
        return Ok(ForwardResolution::Tail);
    }
    // Only bound observed ingest deliberately starts a new capture-time chain.
    // A missing pointer in another writer, or on a forward/equal-time pair,
    // remains damage rather than evidence of a deliberate boundary.
    let bound_observed = read_stream_record(journal, stream)
        .ok()
        .flatten()
        .is_some_and(|record| {
            record
                .get("kind")
                .or_else(|| record.get("type"))
                .and_then(Value::as_str)
                == Some("observer")
                && record
                    .get("cid")
                    .and_then(Value::as_str)
                    .is_some_and(|cid| !cid.is_empty())
                && record.get("source").is_some_and(Value::is_string)
        });
    if !bound_observed {
        return Ok(ForwardResolution::Missing);
    }
    if let Some(target_seq) = marker
        .and_then(|m| m.get("seq"))
        .and_then(Value::as_u64)
        .and_then(|seq| seq.checked_add(1))
    {
        let mut candidates = Vec::new();
        for (scan_day, _) in list_days(journal).unwrap_or_default() {
            for candidate in list_segments(journal, &scan_day).unwrap_or_default() {
                let Ok(Some(candidate_marker)) = read_marker(candidate.path()) else {
                    continue;
                };
                if marker_stream(&candidate_marker) != Some(stream) {
                    continue;
                }
                if candidate_marker.get("seq").and_then(Value::as_u64) != Some(target_seq) {
                    continue;
                }
                let prev_day_null_or_absent =
                    matches!(candidate_marker.get("prev_day"), None | Some(Value::Null));
                let prev_segment_null_or_absent = matches!(
                    candidate_marker.get("prev_segment"),
                    None | Some(Value::Null)
                );
                let capture_boundary = match (
                    capture_key(&location.day, &location.segment),
                    capture_key(&scan_day, candidate.key()),
                ) {
                    (Some(prior), Some(next)) => next < prior,
                    _ => true,
                };
                if prev_day_null_or_absent && prev_segment_null_or_absent && capture_boundary {
                    candidates.push(require_discovered(&scan_day, &candidate, locale)?);
                }
            }
        }
        if !candidates.is_empty() {
            candidates.sort_by_key(|loc| loc.token());
            return Ok(ForwardResolution::Successor(candidates.remove(0)));
        }
    }
    Ok(ForwardResolution::Missing)
}

impl ForwardResolution {
    fn wire(&self, stream: &str) -> String {
        match self {
            Self::Successor(next) => format!("{}/{}/{}", next.day, stream, next.segment),
            Self::Tail => "(tail)".to_owned(),
            Self::Missing => "(none)".to_owned(),
        }
    }

    fn line(&self, locale: &str, stream: &str) -> String {
        match self {
            Self::Successor(next) => copy::display_for(
                locale,
                "journal.cli.segment.inspect.next",
                [(
                    "next",
                    &format_args!("{}/{}/{}", next.day, stream, next.segment),
                )],
            ),
            Self::Tail => copy::render_for(locale, "journal.cli.segment.inspect.next.tail", &[]),
            Self::Missing => copy::render_for(locale, "journal.cli.segment.inspect.next.none", &[]),
        }
    }
}
#[cfg(test)]
pub(crate) fn checks(journal: &Path, location: &SegmentLocation) -> Vec<Check> {
    checks_in(journal, location, copy::ENGLISH)
}
pub(crate) fn checks_in(journal: &Path, location: &SegmentLocation, locale: &str) -> Vec<Check> {
    let exists = location.path.is_dir();
    let marker = if exists {
        read_marker(&location.path).ok().flatten()
    } else {
        None
    };
    let stream = marker
        .as_ref()
        .and_then(marker_stream)
        .unwrap_or(location.stream.as_str());
    let stream_json = location.path.join("stream.json");
    let marker_valid = match &marker {
        Some(marker) if marker_stream(marker).is_some() => (
            true,
            copy::render_for(locale, "journal.cli.segment.check.marker.valid", &[]),
        ),
        Some(_) => (
            false,
            copy::render_for(locale, "journal.cli.segment.check.marker.no.stream", &[]),
        ),
        None if stream_json.exists() => (
            false,
            copy::render_for(locale, "journal.cli.segment.check.marker.invalid", &[]),
        ),
        None => (
            false,
            copy::render_for(locale, "journal.cli.segment.check.marker.missing", &[]),
        ),
    };
    let backward = match marker
        .as_ref()
        .and_then(|value| value.get("prev_segment"))
        .and_then(Value::as_str)
    {
        None => (
            true,
            copy::render_for(locale, "journal.cli.segment.check.chain.start", &[]),
        ),
        Some(_) => match (
            marker
                .as_ref()
                .and_then(|value| value.get("prev_day"))
                .and_then(Value::as_str),
            marker
                .as_ref()
                .and_then(|value| value.get("prev_segment"))
                .and_then(Value::as_str),
        ) {
            (Some(day), Some(segment)) => match chain_location(journal, day, stream, segment) {
                Some(previous) if previous.path.is_dir() => (
                    true,
                    copy::render_for(locale, "journal.cli.segment.check.chain.previous", &[]),
                ),
                _ => (
                    false,
                    copy::display_for(
                        locale,
                        "journal.cli.segment.check.previous.missing",
                        [("path", &(format_args!("{day}/{stream}/{segment}")))],
                    ),
                ),
            },
            _ => (
                false,
                copy::render_for(locale, "journal.cli.segment.check.chain.no.day", &[]),
            ),
        },
    };
    let forward = match resolve_forward(journal, location, stream, marker.as_ref(), locale) {
        Err(message) => (false, message.trim().to_owned()),
        Ok(ForwardResolution::Successor(next)) => (
            true,
            copy::display_for(
                locale,
                "journal.cli.segment.check.next",
                [(
                    "path",
                    &(format_args!("{}/{}/{}", next.day, stream, next.segment)),
                )],
            ),
        ),
        Ok(ForwardResolution::Tail) => (
            true,
            copy::render_for(locale, "journal.cli.segment.check.chain.tail", &[]),
        ),
        Ok(ForwardResolution::Missing) => (
            false,
            copy::render_for(locale, "journal.cli.segment.check.chain.no.successor", &[]),
        ),
    };
    let index = read_segment_index(journal, &location.index_rel);
    let (index_passed, index_unverified, index_detail) = match &index {
        SegmentIndexStatus::Absent => (
            true,
            true,
            copy::render_for(locale, "journal.cli.segment.check.index.absent", &[]),
        ),
        SegmentIndexStatus::Ready { indexed: true, .. } => (
            true,
            false,
            copy::render_for(locale, "journal.cli.segment.check.index.present", &[]),
        ),
        SegmentIndexStatus::Ready { indexed: false, .. } => (
            false,
            false,
            copy::render_for(locale, "journal.cli.segment.check.index.missing", &[]),
        ),
        SegmentIndexStatus::Unreadable { error } => (
            false,
            false,
            copy::display_for(
                locale,
                "journal.cli.segment.check.index.error",
                [("error", &error)],
            ),
        ),
    };
    let content = exists
        && (location.path.join("audio.jsonl").exists()
            || location.path.join("screen.jsonl").exists()
            || list_dir_entries(&location.path)
                .unwrap_or_default()
                .into_iter()
                .any(|entry| {
                    entry.kind == DirEntryKind::File
                        && entry.name.to_string_lossy().starts_with("browser_")
                        && entry.name.to_string_lossy().ends_with(".jsonl")
                }));
    vec![
        Check {
            name: copy::render_for(locale, "journal.cli.segment.check.directory.exists", &[]),
            passed: exists,
            detail: if exists {
                copy::render_for(locale, "journal.cli.segment.check.directory.exists", &[])
            } else {
                copy::render_for(locale, "journal.cli.segment.check.directory.missing", &[])
            },
            unverified: false,
        },
        Check {
            name: copy::render_for(locale, "journal.cli.segment.check.marker.exists", &[]),
            passed: stream_json.is_file(),
            detail: if stream_json.is_file() {
                copy::render_for(locale, "journal.cli.segment.check.marker.exists", &[])
            } else {
                copy::render_for(locale, "journal.cli.segment.check.marker.missing", &[])
            },
            unverified: false,
        },
        Check {
            name: copy::render_for(locale, "journal.cli.segment.check.marker.valid", &[]),
            passed: marker_valid.0,
            detail: marker_valid.1,
            unverified: false,
        },
        Check {
            name: copy::render_for(locale, "journal.cli.segment.check.content.present", &[]),
            passed: content,
            detail: if content {
                copy::render_for(locale, "journal.cli.segment.check.content.present", &[])
            } else if exists {
                copy::render_for(locale, "journal.cli.segment.check.content.empty", &[])
            } else {
                copy::render_for(locale, "journal.cli.segment.check.directory.absent", &[])
            },
            unverified: false,
        },
        Check {
            name: copy::render_for(locale, "journal.cli.segment.check.backward", &[]),
            passed: backward.0,
            detail: backward.1,
            unverified: false,
        },
        Check {
            name: copy::render_for(locale, "journal.cli.segment.check.forward", &[]),
            passed: forward.0,
            detail: forward.1,
            unverified: false,
        },
        Check {
            name: copy::render_for(locale, "journal.cli.segment.check.index", &[]),
            passed: index_passed,
            detail: index_detail,
            unverified: index_unverified,
        },
    ]
}

pub(crate) fn render_checks(checks: &[Check], locale: &str) -> String {
    let mut output = String::new();
    for check in checks {
        if check.passed {
            output.push_str(&format!(
                "{:<5} {}\n",
                copy::render_for(locale, "journal.cli.segment.status.pass", &[]),
                check.name
            ));
        } else {
            output.push_str(&format!(
                "{:<5} {}: {}\n",
                copy::render_for(locale, "journal.cli.doctor.status.fail", &[]),
                check.name,
                check.detail
            ));
        }
    }
    output
}

fn record_stream(segment: &Segment) -> Option<&str> {
    segment
        .record_identity()
        .ok()
        .map(|identity| identity.stream)
}

fn display_stream(segment: &Segment) -> String {
    match segment.stream() {
        StreamLocation::Direct => DEFAULT_STREAM.to_owned(),
        StreamLocation::Named(name) => {
            // Diagnostic table only; never fed back into a path or key.
            #[cfg(unix)]
            {
                use std::os::unix::ffi::OsStrExt;
                solstone_core_system_health::sanitize_os_bytes_for_terminal(name.as_bytes())
            }
            #[cfg(not(unix))]
            {
                solstone_core_system_health::sanitize_for_terminal(&name.to_string_lossy())
            }
        }
    }
}

pub(crate) fn list_output(
    journal: &Path,
    day: &str,
    stream_filter: Option<&str>,
    json_output: bool,
) -> Result<String, String> {
    let locale = crate::output_locale(json_output);
    let mut segments = list_segments(journal, day).unwrap_or_default();
    segments.retain(|segment| stream_filter.is_none_or(|stream| segment.stream().matches(stream)));
    // The reference leaves equal segment keys in filesystem order.  Make that
    // unspecified cross-stream case deterministic with the stream tie-breaker.
    segments.sort_by(|left, right| {
        left.key()
            .cmp(right.key())
            .then_with(|| record_stream(left).cmp(&record_stream(right)))
    });
    if segments.is_empty() {
        return Ok(format!(
            "{}\n",
            copy::display_for(locale, "journal.cli.segment.day.empty", [("day", &day)])
        ));
    }
    if json_output {
        let mut rows = Vec::new();
        for segment in &segments {
            let identity = segment
                .record_identity()
                .map_err(|error| identity_refusal(locale, &error))?;
            let (start, end) = times(identity.key);
            let stats = stats(segment.path());
            rows.push(json!({"stream": identity.stream, "segment": identity.key, "start": start, "end": end,
                "duration": segment_duration(identity.key), "files": stats.files, "talents": stats.talents, "size": stats.size}));
        }
        return Ok(serde_json::to_string_pretty(&rows).expect("rows serialize") + "\n");
    }
    let mut rows = Vec::new();
    for segment in &segments {
        let stream = match segment.record_identity() {
            Ok(identity) => identity.stream.to_owned(),
            Err(SegmentIdentityError::NotUtf8 { .. }) => display_stream(segment),
            Err(error) => return Err(identity_refusal(locale, &error)),
        };
        let key = segment.key();
        let (start, end) = times(key);
        let stats = stats(segment.path());
        rows.push(json!({"stream": stream, "segment": key, "start": start, "end": end,
            "duration": segment_duration(key), "files": stats.files, "talents": stats.talents, "size": stats.size}));
    }
    let mut output = format!(
        "{:<20} {:<14} {:<15} {:>5} {:>5} {:>7} {:>8}\n{}\n",
        copy::render_for(locale, "journal.cli.segment.list.heading.stream", &[]),
        copy::render_for(locale, "journal.cli.segment.list.heading.segment", &[]),
        copy::render_for(locale, "journal.cli.segment.list.heading.time", &[]),
        copy::render_for(locale, "journal.cli.segment.list.heading.dur", &[]),
        copy::render_for(locale, "journal.cli.segment.list.heading.files", &[]),
        copy::render_for(locale, "journal.cli.segment.list.heading.talents", &[]),
        copy::render_for(locale, "journal.cli.segment.list.heading.size", &[]),
        "-".repeat(78)
    );
    for row in rows {
        let start = row["start"].as_str();
        let end = row["end"].as_str();
        let time = start
            .zip(end)
            .map(|(start, end)| format!("{start}-{end}"))
            .unwrap_or_else(|| "?".to_owned());
        output.push_str(&format!(
            "{:<20} {:<14} {:<15} {:>5} {:>5} {:>7} {:>8}\n",
            row["stream"].as_str().unwrap_or(""),
            row["segment"].as_str().unwrap_or(""),
            time,
            format!("{}s", row["duration"].as_u64().unwrap_or(0)),
            row["files"].as_u64().unwrap_or(0),
            row["talents"].as_u64().unwrap_or(0),
            format_size(row["size"].as_u64().unwrap_or(0))
        ));
    }
    Ok(output)
}

pub(crate) fn inspect_output(
    journal: &Path,
    location: &SegmentLocation,
    json_output: bool,
) -> String {
    let locale = crate::output_locale(json_output);
    let marker = read_marker(&location.path)
        .ok()
        .flatten()
        .unwrap_or_else(|| json!({}));
    let stream = marker_stream(&marker).unwrap_or(&location.stream);
    let (start, end) = times(&location.segment);
    let files = top_level_files(&location.path);
    let talents = talent_files(&location.path);
    let stats = stats(&location.path);
    let events = events_summary(&location.path);
    let index = read_segment_index(journal, &location.index_rel);
    let (available, indexed, chunks, error) = index.fields();
    let previous = previous_link(journal, location, &marker, stream);
    let next_marker = read_marker(&location.path).ok().flatten();
    let next = resolve_forward(journal, location, stream, next_marker.as_ref(), locale);
    if json_output {
        let payload = json!({
            "path": location.token(), "stream": stream, "segment": location.segment,
            "seq": marker.get("seq"), "prev_day": marker.get("prev_day"), "prev_segment": marker.get("prev_segment"),
            "start": start, "end": end, "duration": segment_duration(&location.segment),
            "chain": {"prev": previous.wire(), "next": next.as_ref().map(|value| value.wire(stream)).unwrap_or_else(|err|err.trim().to_owned())}, "files": files, "talents": talents,
            "stats": {"files": stats.files, "talents": stats.talents, "size": stats.size}, "events": events,
            "index": {"available": available, "indexed": indexed, "chunks": chunks, "error": error},
        });
        return serde_json::to_string_pretty(&payload).expect("payload serializes") + "\n";
    }
    let range = start
        .zip(end)
        .map(|(start, end)| format!("{start} - {end}"))
        .unwrap_or_else(|| "?".to_owned());
    let mut output = format!(
        "{}\n",
        copy::display_for(
            locale,
            "journal.cli.segment.inspect.path",
            [("path", &(location.token()))]
        )
    );
    if let Some(seq) = marker.get("seq") {
        output.push_str(&format!(
            "{}\n",
            copy::display_for(
                locale,
                "journal.cli.segment.inspect.stream.seq",
                [("stream", &stream), ("sequence", &seq)]
            )
        ));
    } else {
        output.push_str(&format!(
            "{}\n",
            copy::display_for(
                locale,
                "journal.cli.segment.inspect.stream",
                [("stream", &stream)]
            )
        ));
    }
    output.push_str(&format!(
        "{}\n\n",
        copy::display_for(
            locale,
            "journal.cli.segment.inspect.time",
            [
                ("range", &range),
                ("seconds", &(segment_duration(&location.segment)))
            ]
        )
    ));
    output.push_str(&format!(
        "{}\n",
        copy::render_for(locale, "journal.cli.segment.inspect.chain", &[])
    ));
    output.push_str(&format!("  {}\n", previous.line(locale)));
    let next = match next {
        Ok(next) => next.line(locale, stream),
        Err(error) => copy::display_for(
            locale,
            "journal.cli.segment.inspect.next",
            [("next", &error.trim())],
        ),
    };
    output.push_str(&format!("  {next}\n\n"));
    output.push_str(&format!(
        "{}\n",
        copy::display_for(
            locale,
            "journal.cli.segment.inspect.files",
            [("count", &(files.len()))]
        )
    ));
    if !files.is_empty() {
        output.push_str(&format!("  {}\n", files.join(", ")));
    }
    output.push_str(&format!(
        "\n{}\n",
        copy::display_for(
            locale,
            "journal.cli.segment.inspect.talents",
            [("count", &(talents.len()))]
        )
    ));
    if !talents.is_empty() {
        output.push_str(&format!("  {}\n", talents.join(", ")));
    }
    output.push_str(&format!(
        "\n{}\n",
        copy::display_for(
            locale,
            "journal.cli.segment.inspect.size",
            [("size", &(format_size(stats.size)))]
        )
    ));
    match index {
        SegmentIndexStatus::Unreadable { error } => output.push_str(&format!(
            "{}\n",
            copy::display_for(
                locale,
                "journal.cli.segment.inspect.index.error",
                [("error", &error)]
            )
        )),
        SegmentIndexStatus::Ready {
            indexed: true,
            chunks,
        } => output.push_str(&format!(
            "{}\n",
            copy::render_for(
                locale,
                "journal.cli.segment.inspect.indexed",
                &[
                    (
                        "quantity",
                        copy::Value::Cardinal(i64::try_from(chunks).unwrap_or(i64::MAX))
                    ),
                    ("chunks", copy::Value::String(&chunks.to_string()))
                ]
            )
        )),
        SegmentIndexStatus::Ready { indexed: false, .. } => output.push_str(&format!(
            "{}\n",
            copy::render_for(locale, "journal.cli.segment.inspect.index.missing", &[])
        )),
        SegmentIndexStatus::Absent => output.push_str(&format!(
            "{}\n",
            copy::render_for(locale, "journal.cli.segment.inspect.index.unavailable", &[])
        )),
    }
    let tracts = events["tracts"]
        .as_array()
        .expect("events tracts")
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>();
    let entries = events["entries"].as_u64().unwrap_or(0);
    if tracts.is_empty() {
        output.push_str(&format!(
            "{}\n",
            copy::render_for(
                locale,
                "journal.cli.segment.inspect.events",
                &[
                    (
                        "quantity",
                        copy::Value::Cardinal(i64::try_from(entries).unwrap_or(i64::MAX))
                    ),
                    ("entries", copy::Value::String(&entries.to_string()))
                ]
            )
        ));
    } else {
        output.push_str(&format!(
            "{}\n",
            copy::render_for(
                locale,
                "journal.cli.segment.inspect.events.tracts",
                &[
                    (
                        "quantity",
                        copy::Value::Cardinal(i64::try_from(entries).unwrap_or(i64::MAX))
                    ),
                    ("entries", copy::Value::String(&entries.to_string())),
                    (
                        "tracts",
                        copy::Value::String(&(tracts.join(", ")).to_string())
                    )
                ]
            )
        ));
    }
    output
}

pub(crate) fn day_segments(
    journal: &Path,
    day: &str,
    locale: &str,
) -> Result<Vec<SegmentLocation>, String> {
    let day_path = solstone_core_segment::day_path(journal, day, false).ok();
    let Some(day_path) = day_path else {
        return Ok(Vec::new());
    };
    let mut segments = Vec::new();
    for segment in list_segments_in(journal, &day_path).unwrap_or_default() {
        segments.push(require_discovered(day, &segment, locale)?);
    }
    segments.sort_by(|left, right| {
        left.segment
            .cmp(&right.segment)
            .then(left.stream.cmp(&right.stream))
    });
    Ok(segments)
}

#[cfg(test)]
mod tests {
    use super::{next_day, times};

    #[test]
    fn times_wrap_past_midnight() {
        assert_eq!(
            times("235930_60"),
            (Some("23:59:30".to_owned()), Some("00:00:30".to_owned()))
        );
        assert_eq!(
            times("080000_300"),
            (Some("08:00:00".to_owned()), Some("08:05:00".to_owned()))
        );
        assert_eq!(times("not-a-segment"), (None, None));
    }

    #[test]
    fn next_day_steps_the_calendar() {
        assert_eq!(next_day("20261231").as_deref(), Some("20270101"));
        assert_eq!(next_day("20240228").as_deref(), Some("20240229"));
        assert_eq!(next_day("20230228").as_deref(), Some("20230301"));
        assert_eq!(next_day("2026-11-17"), None);
    }
}
