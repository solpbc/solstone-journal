// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! A record's stream is the one stream all of its segments resolve to, or
//! none; two rows ran alongside when they share a facet, differ in id,
//! overlap in time, and come from different named streams. `_default` is the
//! direct layout and is not a named stream. A hidden row is not a participant.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::{Map, Value};
use solstone_core_journal_io::{PathOrDay, iter_segments};

use crate::activity_value_truthy;

pub struct ResolvedSegment {
    pub key: String,
    /// Seconds from local midnight. None when the key is not `HHMMSS_<digits>`.
    pub start_seconds: Option<u32>,
    /// The length written in the key, before `max(1)`. None when the key does not parse.
    pub length_seconds: Option<u32>,
    /// Resolved stream, including `_default`. None when this key does not resolve.
    pub stream: Option<String>,
}

pub struct ActivityCourse {
    /// Parseable keys by start, then unparseable keys, stable.
    pub segments: Vec<ResolvedSegment>,
    /// The single resolved stream of every segment, or None. `_default` is a real value here.
    pub stream: Option<String>,
    /// Half-open `[start, end)` seconds. End uses `length.max(1)`. None when no key parses.
    pub span: Option<(u32, u32)>,
    /// True only when the resolved segment streams include two or more distinct names.
    /// None is not a name. `_default` and `s1` are two names.
    pub mixed: bool,
    /// True only under the pairing rule below.
    pub concurrent: bool,
    /// Indexes into the same input slice: same facet, different non-empty id, span overlap,
    /// and the other row is a participant. A hidden row is never listed and has an empty vec.
    pub partners: Vec<usize>,
}

pub struct PlacedRecord<'a> {
    pub facet: &'a str,
    pub record: &'a Map<String, Value>,
}

fn parse_segment_key(key: &str) -> (Option<u32>, Option<u32>) {
    let Some((clock, length_str)) = key.split_once('_') else {
        return (None, None);
    };
    if clock.len() != 6 || !clock.bytes().all(|byte| byte.is_ascii_digit()) {
        return (None, None);
    }
    if length_str.is_empty() || !length_str.bytes().all(|byte| byte.is_ascii_digit()) {
        return (None, None);
    }
    let Ok(length) = length_str.parse::<u32>() else {
        return (None, None);
    };
    let Ok(hours) = clock[0..2].parse::<u32>() else {
        return (None, None);
    };
    let Ok(minutes) = clock[2..4].parse::<u32>() else {
        return (None, None);
    };
    let Ok(seconds) = clock[4..6].parse::<u32>() else {
        return (None, None);
    };
    if hours > 23 || minutes > 59 || seconds > 59 {
        return (None, None);
    }
    let start = hours * 3600 + minutes * 60 + seconds;
    (Some(start), Some(length))
}

/// When a record ran on its day, as half-open `[start, end)` seconds from
/// midnight, from its segment keys alone. None when no key parses.
pub fn activity_span(record: &Map<String, Value>) -> Option<(u32, u32)> {
    record
        .get("segments")
        .and_then(Value::as_array)?
        .iter()
        .filter_map(Value::as_str)
        .filter_map(|key| match parse_segment_key(key) {
            (Some(start), Some(length)) => Some((start, start + length.max(1))),
            _ => None,
        })
        .reduce(|(start, end), (next_start, next_end)| (start.min(next_start), end.max(next_end)))
}

pub fn place_activity_records(
    journal: &Path,
    day: &str,
    placed: &[PlacedRecord<'_>],
) -> Vec<ActivityCourse> {
    let mut key_streams: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for segment in iter_segments(journal, PathOrDay::Day(day)).unwrap_or_default() {
        if let Ok(identity) = segment.record_identity() {
            key_streams
                .entry(identity.key.to_owned())
                .or_default()
                .insert(identity.stream.to_owned());
        }
    }

    struct Intermediate {
        segments: Vec<ResolvedSegment>,
        stream: Option<String>,
        span: Option<(u32, u32)>,
        mixed: bool,
        is_participant: bool,
        id: String,
    }

    let intermediates = placed
        .iter()
        .map(|entry| {
            let record = entry.record;
            let record_stream_attr = record
                .get("stream")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty());
            let is_participant = !record.get("hidden").is_some_and(activity_value_truthy);
            let id = record
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();

            let raw_keys = record
                .get("segments")
                .and_then(Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();

            let mut segments = raw_keys
                .into_iter()
                .map(|key| {
                    let (start_seconds, length_seconds) = parse_segment_key(&key);
                    let seg_stream = if let Some(target_stream) = record_stream_attr {
                        if key_streams
                            .get(&key)
                            .is_some_and(|streams| streams.contains(target_stream))
                        {
                            Some(target_stream.to_owned())
                        } else {
                            None
                        }
                    } else {
                        match key_streams.get(&key) {
                            Some(streams) if streams.len() == 1 => streams.iter().next().cloned(),
                            _ => None,
                        }
                    };
                    ResolvedSegment {
                        key,
                        start_seconds,
                        length_seconds,
                        stream: seg_stream,
                    }
                })
                .collect::<Vec<_>>();

            segments.sort_by(|a, b| match (a.start_seconds, b.start_seconds) {
                (Some(sa), Some(sb)) => sa.cmp(&sb),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            });

            let stream = if segments.is_empty() {
                None
            } else if segments
                .iter()
                .all(|s| s.stream.is_some() && s.stream == segments[0].stream)
            {
                segments[0].stream.clone()
            } else {
                None
            };

            let span = segments
                .iter()
                .filter_map(|s| match (s.start_seconds, s.length_seconds) {
                    (Some(start), Some(len)) => Some((start, start + len.max(1))),
                    _ => None,
                })
                .reduce(|(min_start, max_end), (start, end)| {
                    (min_start.min(start), max_end.max(end))
                });

            let distinct_streams: BTreeSet<&str> = segments
                .iter()
                .filter_map(|s| s.stream.as_deref())
                .collect();
            let mixed = distinct_streams.len() >= 2;

            Intermediate {
                segments,
                stream,
                span,
                mixed,
                is_participant,
                id,
            }
        })
        .collect::<Vec<_>>();

    let mut courses = Vec::with_capacity(intermediates.len());

    for (index, item) in intermediates.iter().enumerate() {
        if !item.is_participant {
            courses.push(ActivityCourse {
                segments: item.segments.clone(),
                stream: item.stream.clone(),
                span: item.span,
                mixed: item.mixed,
                concurrent: false,
                partners: Vec::new(),
            });
            continue;
        }

        let mut partners = Vec::new();
        if let Some((start, end)) = item.span {
            for (other, candidate) in intermediates.iter().enumerate() {
                if other == index || !candidate.is_participant {
                    continue;
                }
                if placed[other].facet != placed[index].facet {
                    continue;
                }
                if !item.id.is_empty() && candidate.id == item.id {
                    continue;
                }
                if let Some((other_start, other_end)) = candidate.span
                    && start < other_end
                    && other_start < end
                {
                    partners.push(other);
                }
            }
        }

        let is_named_stream = item
            .stream
            .as_deref()
            .is_some_and(|st| st != solstone_core_journal_io::DEFAULT_STREAM);

        let concurrent = if is_named_stream {
            let own_stream = item.stream.as_deref().unwrap();
            partners.iter().any(|&other| {
                let other_segments = &intermediates[other].segments;
                !other_segments.is_empty()
                    && other_segments.iter().all(|s| {
                        s.stream
                            .as_deref()
                            .is_some_and(|st| st != solstone_core_journal_io::DEFAULT_STREAM)
                    })
                    && other_segments
                        .iter()
                        .any(|s| s.stream.as_deref() != Some(own_stream))
            })
        } else {
            false
        };

        courses.push(ActivityCourse {
            segments: item.segments.clone(),
            stream: item.stream.clone(),
            span: item.span,
            mixed: item.mixed,
            concurrent,
            partners,
        });
    }

    courses
}

impl Clone for ResolvedSegment {
    fn clone(&self) -> Self {
        Self {
            key: self.key.clone(),
            start_seconds: self.start_seconds,
            length_seconds: self.length_seconds,
            stream: self.stream.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;
    use tempfile::TempDir;

    use super::*;

    const DAY: &str = "20261003";

    fn chronicle_segment(root: &Path, stream: Option<&str>, key: &str) {
        let dir = match stream {
            Some(stream) => root.join("chronicle").join(DAY).join(stream).join(key),
            None => root.join("chronicle").join(DAY).join(key),
        };
        fs::create_dir_all(dir).unwrap();
    }

    fn record(
        id: &str,
        stream: Option<&str>,
        segments: &[&str],
        hidden: bool,
    ) -> Map<String, Value> {
        let mut obj = json!({
            "id": id,
            "segments": segments,
        })
        .as_object()
        .unwrap()
        .clone();
        if let Some(stream) = stream {
            obj.insert("stream".to_owned(), Value::String(stream.to_owned()));
        }
        if hidden {
            obj.insert("hidden".to_owned(), Value::Bool(true));
        }
        obj
    }

    #[test]
    fn segment_and_course_stream_resolution() {
        let root = TempDir::new_in("/var/tmp").unwrap();
        chronicle_segment(root.path(), Some("s1"), "100000_300");
        chronicle_segment(root.path(), Some("s2"), "100000_300");
        chronicle_segment(root.path(), Some("s2"), "100500_300");
        chronicle_segment(root.path(), None, "110000_300");
        chronicle_segment(root.path(), Some("_default"), "120000_300");

        // 1. Record with stream "s1" whose key also exists under s2 -> resolves to s1
        let r1 = record("r1", Some("s1"), &["100000_300"], false);
        // 2. Same record without stream -> resolves to null (exists under s1 and s2)
        let r2 = record("r2", None, &["100000_300"], false);
        // 3. Record with stream "s1" whose key is absent under s1 and present under s2 -> resolves to null
        let r3 = record("r3", Some("s1"), &["100500_300"], false);
        // 4. Direct layout segment -> resolves to _default
        let r4 = record("r4", None, &["110000_300"], false);
        // 5. Directory literally named _default -> contributes no stream
        let r5 = record("r5", None, &["120000_300"], false);

        let placed = [
            PlacedRecord {
                facet: "work",
                record: &r1,
            },
            PlacedRecord {
                facet: "work",
                record: &r2,
            },
            PlacedRecord {
                facet: "work",
                record: &r3,
            },
            PlacedRecord {
                facet: "work",
                record: &r4,
            },
            PlacedRecord {
                facet: "work",
                record: &r5,
            },
        ];
        let courses = place_activity_records(root.path(), DAY, &placed);

        assert_eq!(courses[0].stream.as_deref(), Some("s1"));
        assert_eq!(courses[0].segments[0].stream.as_deref(), Some("s1"));

        assert_eq!(courses[1].stream, None);
        assert_eq!(courses[1].segments[0].stream, None);

        assert_eq!(courses[2].stream, None);
        assert_eq!(courses[2].segments[0].stream, None);

        assert_eq!(courses[3].stream.as_deref(), Some("_default"));
        assert_eq!(courses[3].segments[0].stream.as_deref(), Some("_default"));

        assert_eq!(courses[4].stream, None);
        assert_eq!(courses[4].segments[0].stream, None);
    }

    #[test]
    fn activity_span_calculation() {
        let root = TempDir::new_in("/var/tmp").unwrap();
        // 10:00:00 (36000) to 10:05:00 (36300), then 10:05:00 (36300) to 10:10:00 (36600)
        let r1 = record("r1", None, &["100000_300", "100500_300"], false);
        // Earlier longer segment: 10:00:00 length 1200 ends at 37200, later segment 10:05:00 length 300 ends at 36600
        let r2 = record("r2", None, &["100000_1200", "100500_300"], false);
        // Segment running past midnight: 23:55:00 (86100) length 600 ends at 86700 > 86400
        let r3 = record("r3", None, &["235500_600"], false);
        // Zero length segment: 10:00:00_0 uses max(1) to end at 36001
        let r4 = record("r4", None, &["100000_0"], false);

        let placed = [
            PlacedRecord {
                facet: "work",
                record: &r1,
            },
            PlacedRecord {
                facet: "work",
                record: &r2,
            },
            PlacedRecord {
                facet: "work",
                record: &r3,
            },
            PlacedRecord {
                facet: "work",
                record: &r4,
            },
        ];
        let courses = place_activity_records(root.path(), DAY, &placed);

        assert_eq!(courses[0].span, Some((36000, 36600)));
        assert_eq!(courses[1].span, Some((36000, 37200)));
        assert_eq!(courses[2].span, Some((86100, 86700)));
        assert!(courses[2].span.unwrap().1 > 86400);
        assert_eq!(courses[3].span, Some((36000, 36001)));
    }

    #[test]
    fn activity_concurrent_pairing() {
        let root = TempDir::new_in("/var/tmp").unwrap();
        chronicle_segment(root.path(), Some("s1"), "100000_300");
        chronicle_segment(root.path(), Some("s2"), "100000_300");
        chronicle_segment(root.path(), Some("s1"), "110000_300");
        chronicle_segment(root.path(), Some("s1"), "110100_300");
        chronicle_segment(root.path(), None, "120000_300");
        chronicle_segment(root.path(), Some("s1"), "120000_300");
        chronicle_segment(root.path(), Some("s1"), "130000_300");
        chronicle_segment(root.path(), Some("s2"), "130000_300");
        chronicle_segment(root.path(), Some("s1"), "140000_300");
        chronicle_segment(root.path(), Some("s2"), "140200_300");
        chronicle_segment(root.path(), None, "150000_300");
        chronicle_segment(root.path(), Some("s1"), "150100_300");

        // Pair 1: Same keys under s1 and s2, both carry stream -> both concurrent
        let r1 = record("r1", Some("s1"), &["100000_300"], false);
        let r2 = record("r2", Some("s2"), &["100000_300"], false);

        // Pair 2: Overlapping in one stream s1 -> not concurrent
        let r3 = record("r3", Some("s1"), &["110000_300"], false);
        let r4 = record("r4", Some("s1"), &["110100_300"], false);

        // Pair 3: Multiple stream matches -> stream is None, neither concurrent
        let r5 = record("r5", None, &["120000_300"], false); // stream is None because 120000_300 exists under multiple streams
        let r6 = record("r6", Some("s1"), &["120000_300"], false);

        // Pair 4: Hidden row on s2 overlapping visible row on s1 -> neither concurrent, hidden has empty partners
        let r7 = record("r7", Some("s1"), &["130000_300"], false);
        let r8 = record("r8", Some("s2"), &["130000_300"], true); // hidden

        // Pair 5: Mixed partner (spans s1 and s2) -> mixed partner not concurrent (mixed), single-stream partner is concurrent
        let r9 = record("r9", None, &["140000_300", "140200_300"], false); // mixed
        let r10 = record("r10", Some("s1"), &["140000_300"], false);

        // Pair 6: _default stream (root segment only) overlapping named stream s1 -> neither concurrent
        let r11 = record("r11", None, &["150000_300"], false); // resolves to _default
        let r12 = record("r12", Some("s1"), &["150100_300"], false);

        let placed = [
            PlacedRecord {
                facet: "work",
                record: &r1,
            },
            PlacedRecord {
                facet: "work",
                record: &r2,
            },
            PlacedRecord {
                facet: "work",
                record: &r3,
            },
            PlacedRecord {
                facet: "work",
                record: &r4,
            },
            PlacedRecord {
                facet: "work",
                record: &r5,
            },
            PlacedRecord {
                facet: "work",
                record: &r6,
            },
            PlacedRecord {
                facet: "work",
                record: &r7,
            },
            PlacedRecord {
                facet: "work",
                record: &r8,
            },
            PlacedRecord {
                facet: "work",
                record: &r9,
            },
            PlacedRecord {
                facet: "work",
                record: &r10,
            },
            PlacedRecord {
                facet: "work",
                record: &r11,
            },
            PlacedRecord {
                facet: "work",
                record: &r12,
            },
        ];
        let courses = place_activity_records(root.path(), DAY, &placed);

        // Pair 1
        assert!(courses[0].concurrent);
        assert!(courses[1].concurrent);
        assert_eq!(courses[0].partners, vec![1]);
        assert_eq!(courses[1].partners, vec![0]);

        // Pair 2
        assert!(!courses[2].concurrent);
        assert!(!courses[3].concurrent);

        // Pair 3
        assert!(!courses[4].concurrent);
        assert!(!courses[5].concurrent);

        // Pair 4
        assert!(!courses[6].concurrent);
        assert!(!courses[7].concurrent);
        assert!(courses[6].partners.is_empty());
        assert!(courses[7].partners.is_empty());

        // Pair 5
        assert!(courses[8].mixed);
        assert!(!courses[8].concurrent);
        assert!(courses[9].concurrent);
        assert_eq!(courses[9].partners, vec![8]);

        // Pair 6
        assert_eq!(courses[10].stream.as_deref(), Some("_default"));
        assert_eq!(courses[11].stream.as_deref(), Some("s1"));
        assert!(!courses[10].concurrent);
        assert!(!courses[11].concurrent);
    }
}
