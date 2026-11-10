// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Reading one segment's `location.jsonl`, in either shape the phones write.
//!
//! - **iPhone and Watch** write an enveloped file: a `solstone.location.segment/1`
//!   header (`platform`, `tier`, `accuracy` permission, `gap`), then
//!   `solstone.location.fix/1` readings (`t` as ISO-8601 UTC, `lat`, `lon`,
//!   `h_acc` metres) and, on the iPhone, `solstone.location.visit/1` visits
//!   (`arrival`, `departure` or null, `lat`, `lon`, `h_acc`).
//! - **Android** writes bare rows with no header: `provider`, `timestamp` in
//!   epoch milliseconds, `lat`, `lon`, `accuracy` metres when it has one, and
//!   `fixAge`. Android only asks for approximate location.
//!
//! Nothing here guesses. A missing accuracy stays unknown, an arrival the phone
//! did not know stays unknown, and a line this reader does not recognise is
//! counted so the owner sees it rather than losing it silently.

use chrono::{DateTime, TimeZone, Utc};
use serde_json::{Map, Value};

pub const SEGMENT_SCHEMA: &str = "solstone.location.segment/1";
pub const FIX_SCHEMA: &str = "solstone.location.fix/1";
pub const VISIT_SCHEMA: &str = "solstone.location.visit/1";

/// Which device platform wrote the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Platform {
    Ios,
    Watchos,
    Wearos,
    Android,
    Unknown,
}

impl Platform {
    pub fn parse(value: &str) -> Self {
        match value.to_ascii_lowercase().as_str() {
            "ios" | "ipados" => Self::Ios,
            "watchos" => Self::Watchos,
            "wearos" => Self::Wearos,
            "android" => Self::Android,
            _ => Self::Unknown,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ios => "ios",
            Self::Watchos => "watchos",
            Self::Wearos => "wearos",
            Self::Android => "android",
            Self::Unknown => "unknown",
        }
    }
}

/// How finely a reading can place someone. Only `Precise` readings could ever
/// support anything finer than an area, and in this release nothing does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Precision {
    /// iPhone with precise location allowed.
    Precise,
    /// iPhone or Watch with precise location turned off.
    Reduced,
    /// Android approximate location, a few square kilometres.
    Approximate,
    /// The file did not say.
    Unknown,
}

impl Precision {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Precise => "precise",
            Self::Reduced => "reduced",
            Self::Approximate => "approximate",
            Self::Unknown => "unknown",
        }
    }
}

/// The header an iPhone or Watch file starts with.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Header {
    pub platform: Option<String>,
    /// `light`, `balanced` or `full`: the setting the phone kept location at.
    pub tier: Option<String>,
    /// `full` or `reduced`: whether precise location was allowed.
    pub accuracy: Option<String>,
    /// The phone said coverage was interrupted during this segment.
    pub gap: bool,
}

/// One reading.
#[derive(Debug, Clone, PartialEq)]
pub struct Fix {
    /// Zero-based line number in the file.
    pub line: u32,
    pub time: DateTime<Utc>,
    pub latitude: f64,
    pub longitude: f64,
    /// Horizontal accuracy in metres, when the phone reported a valid one.
    pub accuracy_m: Option<f64>,
}

/// One visit the iPhone reported (CLVisit).
#[derive(Debug, Clone, PartialEq)]
pub struct Visit {
    pub line: u32,
    /// `None` when the phone did not know when the visit began.
    pub arrival: Option<DateTime<Utc>>,
    /// `None` when the phone had not reported leaving.
    pub departure: Option<DateTime<Utc>>,
    pub latitude: f64,
    pub longitude: f64,
    pub accuracy_m: Option<f64>,
}

/// Everything read from one file.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SegmentFile {
    pub header: Option<Header>,
    pub fixes: Vec<Fix>,
    pub visits: Vec<Visit>,
    /// Lines that were not valid JSON objects, or held impossible values.
    pub unreadable_lines: u32,
    /// Valid JSON lines of a kind this reader does not know.
    pub unrecognised_lines: u32,
    /// The whole file could not be read as text.
    pub unreadable_file: bool,
}

impl SegmentFile {
    /// The platform the file says wrote it, when it says.
    pub fn platform(&self) -> Option<Platform> {
        let header = self.header.as_ref()?;
        header.platform.as_deref().map(Platform::parse)
    }

    /// How precise this file's readings are, from what the file itself says.
    /// `fallback` is the platform known from elsewhere (the upload record).
    pub fn precision(&self, fallback: Platform) -> Precision {
        if let Some(header) = &self.header {
            return match header.accuracy.as_deref() {
                Some("full") => match self.platform() {
                    Some(Platform::Watchos) => Precision::Reduced,
                    _ => Precision::Precise,
                },
                Some("reduced") => Precision::Reduced,
                _ => Precision::Unknown,
            };
        }
        match fallback {
            Platform::Android => Precision::Approximate,
            _ => Precision::Unknown,
        }
    }
}

fn number(object: &Map<String, Value>, key: &str) -> Option<f64> {
    object
        .get(key)
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite())
}

fn coordinates(object: &Map<String, Value>) -> Option<(f64, f64)> {
    let latitude = number(object, "lat")?;
    let longitude = number(object, "lon")?;
    ((-90.0..=90.0).contains(&latitude) && (-180.0..=180.0).contains(&longitude))
        .then_some((latitude, longitude))
}

fn accuracy(object: &Map<String, Value>, key: &str) -> Option<f64> {
    // CoreLocation reports a negative accuracy for an invalid fix.
    number(object, key).filter(|value| *value >= 0.0)
}

fn iso_time(object: &Map<String, Value>, key: &str) -> Result<Option<DateTime<Utc>>, ()> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => {
            let parsed = DateTime::parse_from_rfc3339(text).map_err(|_| ())?;
            let utc = parsed.with_timezone(&Utc);
            // CLVisit writes its "distant past" for an unknown arrival, and its
            // "distant future" for a visit still going on.
            if utc.timestamp() < 946_684_800 || utc.timestamp() > 32_503_680_000 {
                return Ok(None);
            }
            Ok(Some(utc))
        }
        Some(_) => Err(()),
    }
}

fn read_header(object: &Map<String, Value>) -> Header {
    let text = |key: &str| object.get(key).and_then(Value::as_str).map(str::to_owned);
    Header {
        platform: text("platform"),
        tier: text("tier"),
        accuracy: text("accuracy"),
        gap: object.get("gap").and_then(Value::as_bool).unwrap_or(false),
    }
}

/// Parse one `location.jsonl`. Never fails: what cannot be read is counted.
pub fn parse_segment_file(bytes: &[u8]) -> SegmentFile {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return SegmentFile {
            unreadable_file: true,
            ..SegmentFile::default()
        };
    };
    let mut file = SegmentFile::default();
    for (index, line) in text.lines().enumerate() {
        let line_number = u32::try_from(index).unwrap_or(u32::MAX);
        if line.trim().is_empty() {
            continue;
        }
        let Ok(Value::Object(object)) = serde_json::from_str::<Value>(line) else {
            file.unreadable_lines += 1;
            continue;
        };
        match object.get("schema").and_then(Value::as_str) {
            Some(SEGMENT_SCHEMA) if file.header.is_none() => {
                file.header = Some(read_header(&object));
            }
            Some(FIX_SCHEMA) => match (coordinates(&object), iso_time(&object, "t")) {
                (Some((latitude, longitude)), Ok(Some(time))) => file.fixes.push(Fix {
                    line: line_number,
                    time,
                    latitude,
                    longitude,
                    accuracy_m: accuracy(&object, "h_acc"),
                }),
                _ => file.unreadable_lines += 1,
            },
            Some(VISIT_SCHEMA) => match (
                coordinates(&object),
                iso_time(&object, "arrival"),
                iso_time(&object, "departure"),
            ) {
                (Some((latitude, longitude)), Ok(arrival), Ok(departure)) => {
                    file.visits.push(Visit {
                        line: line_number,
                        arrival,
                        departure,
                        latitude,
                        longitude,
                        accuracy_m: accuracy(&object, "h_acc"),
                    });
                }
                _ => file.unreadable_lines += 1,
            },
            Some(_) => file.unrecognised_lines += 1,
            None if object.contains_key("timestamp") => {
                let time = object
                    .get("timestamp")
                    .and_then(Value::as_i64)
                    .and_then(|millis| Utc.timestamp_millis_opt(millis).single());
                match (coordinates(&object), time) {
                    (Some((latitude, longitude)), Some(time)) => file.fixes.push(Fix {
                        line: line_number,
                        time,
                        latitude,
                        longitude,
                        accuracy_m: accuracy(&object, "accuracy"),
                    }),
                    _ => file.unreadable_lines += 1,
                }
            }
            None => file.unrecognised_lines += 1,
        }
    }
    file
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wear_location_preserves_its_distinct_platform() {
        let file = parse_segment_file(
            br#"{"accuracy":"reduced","fix_count":0,"gap":false,"kind":"location","platform":"wearos","schema":"solstone.location.segment/1","source":"location","tier":"light"}
"#,
        );
        assert_eq!(file.platform(), Some(Platform::Wearos));
        assert_eq!(Platform::parse(Platform::Wearos.as_str()), Platform::Wearos);
        assert_ne!(file.platform(), Some(Platform::Android));
        assert_ne!(file.platform(), Some(Platform::Watchos));
        assert_eq!(file.precision(Platform::Unknown), Precision::Reduced);
    }

    #[test]
    fn an_iphone_file_reads_its_header_readings_and_visits() {
        let file = parse_segment_file(
            br#"{"accuracy":"full","fix_count":2,"gap":false,"kind":"location","platform":"ios","schema":"solstone.location.segment/1","source":"location","tier":"balanced"}
{"alt":null,"course":null,"h_acc":12,"lat":39.7527,"lon":-105.0001,"schema":"solstone.location.fix/1","speed":null,"stationary":false,"t":"2026-10-01T16:05:35Z","v_acc":null}
{"alt":1600.5,"course":90,"h_acc":-1,"lat":39.7528,"lon":-105.0002,"schema":"solstone.location.fix/1","speed":1.5,"stationary":false,"t":"2026-10-01T16:06:35Z","v_acc":4}
{"arrival":"2026-10-01T15:00:00Z","departure":null,"h_acc":40,"lat":39.7527,"lon":-105.0001,"schema":"solstone.location.visit/1"}
"#,
        );
        let header = file.header.as_ref().unwrap();
        assert_eq!(header.tier.as_deref(), Some("balanced"));
        assert_eq!(file.platform(), Some(Platform::Ios));
        assert_eq!(file.precision(Platform::Unknown), Precision::Precise);
        assert_eq!(file.fixes.len(), 2);
        assert_eq!(file.fixes[0].accuracy_m, Some(12.0));
        assert_eq!(
            file.fixes[1].accuracy_m, None,
            "a negative accuracy is unknown"
        );
        assert_eq!(file.visits.len(), 1);
        assert_eq!(file.visits[0].departure, None);
        assert_eq!(file.unreadable_lines + file.unrecognised_lines, 0);
    }

    #[test]
    fn an_android_file_reads_bare_rows_as_approximate_readings() {
        let file = parse_segment_file(
            br#"{"provider":"network","timestamp":1759334735000,"lat":39.7392,"lon":-104.9903,"accuracy":2000.0,"fixAge":60000}
{"provider":"fused","timestamp":1759334795000,"lat":39.74,"lon":-104.99,"fixAge":0}
"#,
        );
        assert!(file.header.is_none());
        assert_eq!(file.precision(Platform::Android), Precision::Approximate);
        assert_eq!(file.fixes.len(), 2);
        assert_eq!(file.fixes[0].time.timestamp(), 1_759_334_735);
        assert_eq!(
            file.fixes[1].accuracy_m, None,
            "an omitted accuracy is unknown"
        );
    }

    #[test]
    fn a_watch_with_precise_location_on_still_reads_as_reduced() {
        let file = parse_segment_file(
            br#"{"accuracy":"full","fix_count":0,"gap":true,"kind":"location","platform":"watchos","schema":"solstone.location.segment/1","source":"location","tier":"light"}
"#,
        );
        assert_eq!(file.precision(Platform::Unknown), Precision::Reduced);
        assert!(file.header.as_ref().unwrap().gap);
    }

    #[test]
    fn unknown_visit_ends_stay_unknown() {
        let file = parse_segment_file(
            br#"{"arrival":"0001-01-01T00:00:00Z","departure":"4001-01-01T00:00:00Z","h_acc":40,"lat":1,"lon":2,"schema":"solstone.location.visit/1"}
"#,
        );
        assert_eq!(file.visits[0].arrival, None);
        assert_eq!(file.visits[0].departure, None);
    }

    #[test]
    fn what_cannot_be_read_is_counted_not_dropped() {
        let file = parse_segment_file(
            br#"not json
{"schema":"solstone.location.fix/1","t":"yesterday","lat":1,"lon":2}
{"schema":"solstone.location.fix/1","t":"2026-10-01T16:05:35Z","lat":95,"lon":2}
{"schema":"solstone.location.other/9"}
{"something":"else"}
"#,
        );
        assert_eq!(file.unreadable_lines, 3);
        assert_eq!(file.unrecognised_lines, 2);
        assert!(file.fixes.is_empty());
        assert!(parse_segment_file(&[0xff, 0xfe]).unreadable_file);
    }
}
