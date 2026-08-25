// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::Value;
use solstone_core_segment::{list_days, list_segments, set_stream_tail_unconditionally};

use super::history::pruned_records_by_stream;
use super::marker::{StreamMarker, read_segment_marker, write_segment_marker};
use super::types::Refusal;

type SegmentKey = (String, String);

pub struct AncestorGate {
    pub gate: &'static str,
    pub cycle_resolution: &'static str,
    pub dead_end_resolution: &'static str,
}

impl Default for AncestorGate {
    fn default() -> Self {
        Self {
            gate: "chain-repair",
            cycle_resolution: "break the predecessor cycle before pruning",
            dead_end_resolution: "restore the missing predecessor or append a valid pruned history record",
        }
    }
}

fn marker_prev_key(marker: &StreamMarker) -> Option<SegmentKey> {
    match (&marker.prev_day, &marker.prev_segment) {
        (Some(day), Some(segment)) => Some((day.clone(), segment.clone())),
        _ => None,
    }
}

/// Walk `prev_day`/`prev_segment` pointers from `start` until a segment that
/// still exists on disk (`existing`) is found, resolving through markers of
/// segments deleted earlier in this same run (`deleted_markers`) and, past
/// those, through `pruned` history's `duplicate_of` chain. Refuses on a
/// pointer cycle or a dead end that neither a deleted marker nor a pruned
/// record can explain.
pub fn nearest_surviving_ancestor(
    start: SegmentKey,
    stream: &str,
    existing: &BTreeSet<SegmentKey>,
    deleted_markers: &BTreeMap<SegmentKey, StreamMarker>,
    pruned: &BTreeMap<SegmentKey, Value>,
    gate: &AncestorGate,
) -> (Option<SegmentKey>, Option<Refusal>) {
    let mut current = Some(start);
    let mut seen: BTreeSet<SegmentKey> = BTreeSet::new();
    while let Some(key) = current.clone() {
        if existing.contains(&key) {
            return (Some(key), None);
        }
        if seen.contains(&key) {
            return (
                None,
                Some(Refusal::new(
                    format!("{}/{stream}/{}", key.0, key.1),
                    gate.gate,
                    Some("stream.json".to_owned()),
                    gate.cycle_resolution,
                )),
            );
        }
        seen.insert(key.clone());
        if let Some(marker) = deleted_markers.get(&key) {
            current = marker_prev_key(marker);
            continue;
        }
        if let Some(record) = pruned.get(&key) {
            let duplicate_of = record
                .get("duplicate_of")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty());
            current = duplicate_of.map(|duplicate_of| (key.0.clone(), duplicate_of.to_owned()));
            continue;
        }
        return (
            None,
            Some(Refusal::new(
                format!("{}/{stream}/{}", key.0, key.1),
                gate.gate,
                Some("stream.json".to_owned()),
                gate.dead_end_resolution,
            )),
        );
    }
    (None, None)
}

/// Every currently-on-disk segment `(day, basename)` -> path for one stream.
///
/// The parsed key is also recorded when it differs from the basename so marker
/// `prev_segment` lookups still resolve. Two same-key siblings stay distinct
/// under their exact names; the key alias is filled only when vacant.
pub fn stream_segments(journal: &Path, stream: &str) -> BTreeMap<SegmentKey, PathBuf> {
    let mut segments = BTreeMap::new();
    let Ok(days) = list_days(journal) else {
        return segments;
    };
    for (day, _) in days {
        let Ok(entries) = list_segments(journal, &day) else {
            continue;
        };
        for segment in entries {
            if !segment.stream().matches(stream) {
                continue;
            }
            let path = segment.path().to_path_buf();
            let Ok(identity) = segment.record_identity() else {
                continue;
            };
            segments.insert((day.clone(), identity.name.to_owned()), path.clone());
            if identity.name != identity.key {
                segments
                    .entry((day.clone(), identity.key.to_owned()))
                    .or_insert(path);
            }
        }
    }
    segments
}

/// Repair survivor `prev_day`/`prev_segment` pointers that point through
/// segments pruned in this run (`deleted_markers`) or an earlier one, for
/// every currently-on-disk segment of `stream`. Never renumbers `seq`.
pub fn repair_stream_chain(
    journal: &Path,
    stream: &str,
    deleted_markers: &BTreeMap<SegmentKey, StreamMarker>,
    dry_run: bool,
) -> (Vec<Refusal>, u64, BTreeSet<String>) {
    let pruned = match pruned_records_by_stream(journal, stream) {
        Ok(map) => map,
        Err(refusal) => return (vec![refusal], 0, BTreeSet::new()),
    };
    let segments = stream_segments(journal, stream);
    let existing: BTreeSet<SegmentKey> = segments.keys().cloned().collect();
    let mut refusals = Vec::new();
    let mut repaired = 0u64;
    let mut mutated_days = BTreeSet::new();
    for (key, path) in &segments {
        let Some(marker) = read_segment_marker(path) else {
            continue;
        };
        let Some(prev_key) = marker_prev_key(&marker) else {
            continue;
        };
        if existing.contains(&prev_key) {
            continue;
        }
        let (target, refusal) = nearest_surviving_ancestor(
            prev_key,
            stream,
            &existing,
            deleted_markers,
            &pruned,
            &AncestorGate::default(),
        );
        if let Some(refusal) = refusal {
            refusals.push(refusal);
            continue;
        }
        if dry_run {
            repaired += 1;
            continue;
        }
        let repaired_marker = StreamMarker {
            stream: marker.stream.clone(),
            prev_day: target.as_ref().map(|target| target.0.clone()),
            prev_segment: target.as_ref().map(|target| target.1.clone()),
            seq: marker.seq,
        };
        match write_segment_marker(path, &repaired_marker) {
            Ok(()) => {
                repaired += 1;
                mutated_days.insert(key.0.clone());
            }
            Err(error) => refusals.push(Refusal::new(
                format!("{}/{stream}/{}", key.0, key.1),
                "chain-repair-write",
                Some("stream.json"),
                format!(
                    "successor chain repair could not be published: {error}; fix the filesystem error and rerun prune"
                ),
            )),
        }
    }
    (refusals, repaired, mutated_days)
}

/// Recompute a stream's registry tail after deletions, only when the
/// recorded tail no longer exists on disk. Chooses the surviving segment
/// with the highest `seq` and never regresses the stored `seq`.
pub fn repair_stream_registry_state(journal: &Path, stream: &str) {
    let segments = stream_segments(journal, stream);
    if let Some(state) = solstone_core_segment::read_stream_record(journal, stream)
        .ok()
        .flatten()
        .and_then(|value| serde_json::from_value::<solstone_core_segment::StreamRecord>(value).ok())
        && let (Some(last_day), Some(last_segment)) = (&state.last_day, &state.last_segment)
        && segments.contains_key(&(last_day.clone(), last_segment.clone()))
    {
        return;
    }
    let mut max_seq = 0u64;
    let mut tail: Option<SegmentKey> = None;
    for (key, path) in &segments {
        let Some(marker) = read_segment_marker(path) else {
            continue;
        };
        if marker.seq >= max_seq {
            max_seq = marker.seq;
            tail = Some(key.clone());
        }
    }
    set_stream_tail_unconditionally(
        journal,
        stream,
        tail.as_ref().map(|key| key.0.as_str()),
        tail.as_ref().map(|key| key.1.as_str()),
        max_seq,
    );
}
