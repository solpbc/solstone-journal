// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Value, json};

use crate::segments::{SegmentDirectory, origin, segment_key};

#[derive(Debug, Clone)]
struct AvailableSegment {
    origin: String,
    stream: String,
    key: String,
    has_audio: bool,
    has_screen: bool,
    has_browser: bool,
}

pub fn hours_avail(day: &str, segments: Vec<SegmentDirectory>) -> Value {
    let mut buckets = BTreeMap::<(u32, u32), Vec<AvailableSegment>>::new();
    for segment in segments {
        let Some(key) = segment_key(&segment.key) else {
            continue;
        };
        // `iter_segments` retains a directory name that can merely contain a key.
        // Extract and parse the ASCII key; malformed names are omitted rather than
        // becoming a phantom 00:00 availability bucket.
        let Ok(hour) = key[0..2].parse::<u32>() else {
            continue;
        };
        let Ok(minute) = key[2..4].parse::<u32>() else {
            continue;
        };
        buckets
            .entry((hour, minute / 5 * 5))
            .or_default()
            .push(available(day, segment));
    }
    let mut hours = serde_json::Map::new();
    for hour in 0..24 {
        let rows = (0..60)
            .step_by(5)
            .map(|minute| bucket(buckets.remove(&(hour, minute)), minute))
            .collect::<Vec<_>>();
        if rows.iter().any(|row| !row["best_origin"].is_null()) {
            hours.insert(format!("{hour:02}"), json!({"buckets": rows}));
        }
    }
    Value::Object(hours)
}

fn available(day: &str, segment: SegmentDirectory) -> AvailableSegment {
    AvailableSegment {
        origin: origin(day, &segment.stream, &segment.key),
        stream: segment.stream,
        key: segment.key,
        has_audio: segment.path.join("audio.jsonl").is_file(),
        has_screen: has_matching_file(&segment.path, |name| name.ends_with("screen.jsonl")),
        has_browser: has_matching_file(&segment.path, |name| {
            name.starts_with("browser_") && name.ends_with(".jsonl")
        }),
    }
}

fn has_matching_file(path: &Path, predicate: impl Fn(&str) -> bool) -> bool {
    std::fs::read_dir(path)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .any(|entry| predicate(&entry.file_name().to_string_lossy()))
}

fn rank(segment: &AvailableSegment) -> u8 {
    match (segment.has_audio, segment.has_screen, segment.has_browser) {
        (true, true, _) => 0,
        (_, true, _) => 1,
        (true, _, _) => 2,
        (_, _, true) => 3,
        _ => 4,
    }
}

fn bucket(segments: Option<Vec<AvailableSegment>>, minute: u32) -> Value {
    let Some(mut segments) = segments else {
        return json!({"minute": minute, "best_origin": null, "has_audio": false, "has_screen": false, "has_browser": false, "browser_origin": null, "segment_count": 0});
    };
    // Deterministic refinement: when Python's rank-only stable ordering ties, choose (key, stream, origin); it matches every reference-determinate result and removes read_dir-order variance.
    segments.sort_by_key(|segment| {
        (
            rank(segment),
            segment.key.clone(),
            segment.stream.clone(),
            segment.origin.clone(),
        )
    });
    let best = &segments[0];
    let browser = segments
        .iter()
        .filter(|segment| segment.has_browser)
        .min_by_key(|segment| {
            (
                segment.key.clone(),
                segment.stream.clone(),
                segment.origin.clone(),
            )
        });
    json!({"minute": minute, "best_origin": best.origin, "has_audio": best.has_audio, "has_screen": best.has_screen, "has_browser": segments.iter().any(|segment| segment.has_browser), "browser_origin": browser.map(|segment| segment.origin.clone()), "segment_count": segments.len()})
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::Value;
    use tempfile::TempDir;

    use crate::segments::{DEFAULT_STREAM, SegmentDirectory};

    use super::hours_avail;

    fn segment(root: &TempDir, stream: &str, key: &str, files: &[&str]) -> SegmentDirectory {
        let path = if stream == DEFAULT_STREAM {
            root.path().join(key)
        } else {
            root.path().join(stream).join(key)
        };
        fs::create_dir_all(&path).expect("segment");
        for file in files {
            fs::write(path.join(file), "").expect("file");
        }
        SegmentDirectory {
            stream: stream.to_owned(),
            key: key.to_owned(),
            path,
        }
    }

    fn first<'a>(payload: &'a Value, hour: &str, minute: usize) -> &'a Value {
        &payload[hour]["buckets"][minute / 5]
    }

    #[test]
    fn ac9b_browser_origin_uses_key_before_origin() {
        let root = TempDir::new().expect("root");
        // Default origins sort first because `20260510/1` precedes `20260510/w`; key and
        // origin disagree only when default has the later key in the same five-minute bucket.
        let later_default = segment(
            &root,
            DEFAULT_STREAM,
            "100100_300",
            &["audio.jsonl", "a.screen.jsonl", "browser_default.jsonl"],
        );
        let earlier_named = segment(
            &root,
            "workstation.browser",
            "100000_300",
            &["browser_named.jsonl"],
        );
        let payload = hours_avail("20260510", vec![later_default, earlier_named]);
        let bucket = first(&payload, "10", 0);
        assert_eq!(bucket["segment_count"], 2);
        assert_eq!(bucket["has_audio"], true);
        assert_eq!(bucket["has_screen"], true);
        assert_eq!(bucket["has_browser"], true);
        assert_eq!(
            bucket["browser_origin"],
            "20260510/workstation.browser/100000_300"
        );
    }

    #[test]
    fn ac9b_all_rank_levels_flow_through_availability_reduction() {
        let root = TempDir::new().expect("root");
        let mut segments = Vec::new();
        for (minute, winner, loser) in [
            (
                0,
                &["audio.jsonl", "a.screen.jsonl"][..],
                &["a.screen.jsonl"][..],
            ),
            (5, &["a.screen.jsonl"][..], &["audio.jsonl"][..]),
            (10, &["audio.jsonl"][..], &["browser_loser.jsonl"][..]),
            (15, &["browser_winner.jsonl"][..], &[][..]),
            (20, &[][..], &[][..]),
        ] {
            segments.push(segment(
                &root,
                DEFAULT_STREAM,
                &format!("10{minute:02}00_300"),
                winner,
            ));
            segments.push(segment(
                &root,
                "other",
                &format!("10{minute:02}01_300"),
                loser,
            ));
        }
        let payload = hours_avail("20260510", segments);
        for (minute, suffix) in [
            (0, "100000_300"),
            (5, "100500_300"),
            (10, "101000_300"),
            (15, "101500_300"),
            (20, "102000_300"),
        ] {
            assert!(
                first(&payload, "10", minute)["best_origin"]
                    .as_str()
                    .expect("origin")
                    .ends_with(suffix)
            );
        }
    }

    #[test]
    fn ac9b_tied_rank_uses_total_key_stream_origin_order_and_handles_embedded_keys() {
        let root = TempDir::new().expect("root");
        let named = segment(&root, "z", "100000_300", &["audio.jsonl"]);
        let default = segment(&root, DEFAULT_STREAM, "100000_300", &["audio.jsonl"]);
        let embedded = segment(&root, "stream", "—100600_300", &["audio.jsonl"]);
        let payload = hours_avail("20260510", vec![named, default, embedded]);
        assert_eq!(
            first(&payload, "10", 0)["best_origin"],
            "20260510/100000_300"
        );
        assert_eq!(first(&payload, "10", 0)["segment_count"], 2);
        assert_eq!(
            first(&payload, "10", 0)["best_origin"],
            "20260510/100000_300"
        );
        assert_eq!(
            first(&payload, "10", 5)["best_origin"],
            "20260510/stream/—100600_300"
        );
    }
}
