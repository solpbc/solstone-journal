// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::Path;

use serde_json::Value;
use solstone_core_journal_io::{DirEntry, DirEntryKind, list_dir_entries};
use solstone_core_retention::receipt::Target;
use solstone_core_segment::{is_reserved_name, list_days, list_segments};

const LOCATION_FILE: &str = "location.jsonl";
// Nothing in the tree writes `item.json`, and it is not in
// RESERVED_SEGMENT_FILENAMES. Excluded so a future reserved-set expansion is
// not silently implied.
const ITEM_FILE: &str = "item.json";

pub(crate) struct Selected {
    pub target: Target,
    pub mixed: bool,
    pub cid: String,
}

pub(crate) struct LocationScan {
    pub targets: Vec<Selected>,
    pub complete: bool,
}

/// A segment is mixed iff a direct child is client content other than its
/// location data. Directories count (`talents/` makes a segment mixed).
pub(crate) fn segment_is_mixed(entries: &[DirEntry]) -> bool {
    entries.iter().any(|entry| {
        let Some(name) = entry.name.to_str() else {
            return true;
        };
        name != LOCATION_FILE && name != ITEM_FILE && !is_reserved_name(name)
    })
}

fn holds_location_file(entries: &[DirEntry]) -> bool {
    entries
        .iter()
        .any(|entry| entry.kind == DirEntryKind::File && entry.name.to_str() == Some(LOCATION_FILE))
}

fn holds_tombstone(entries: &[DirEntry]) -> bool {
    entries.iter().any(|entry| {
        entry
            .name
            .to_str()
            .is_some_and(|name| name.eq_ignore_ascii_case("tombstone.json"))
    })
}

fn device_json_fingerprint(value: &Value) -> Option<&str> {
    value
        .get("cid")
        .and_then(Value::as_str)
        .filter(|fingerprint| !fingerprint.is_empty())
        .or_else(|| {
            value
                .get("did")
                .and_then(Value::as_str)
                .filter(|fingerprint| !fingerprint.is_empty())
        })
}

fn segment_cid(path: &Path) -> String {
    let Ok(bytes) = fs::read(path.join("device.json")) else {
        return "unknown".to_owned();
    };
    let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
        return "unknown".to_owned();
    };
    device_json_fingerprint(&value)
        .map(str::to_owned)
        .unwrap_or_else(|| "unknown".to_owned())
}

pub(crate) fn select_location_targets(journal: &Path) -> LocationScan {
    let mut selected = Vec::new();
    let days = match list_days(journal) {
        Ok(days) => days,
        Err(_) => {
            return LocationScan {
                targets: selected,
                complete: false,
            };
        }
    };
    let mut complete = true;
    for (day, _) in days {
        let segments = match list_segments(journal, &day) {
            Ok(segments) => segments,
            Err(_) => {
                complete = false;
                continue;
            }
        };
        for segment in segments {
            let Ok(identity) = segment.record_identity() else {
                complete = false;
                continue;
            };
            let dir = identity.name.to_owned();
            if dir.starts_with('.') {
                continue;
            }
            let Ok(entries) = list_dir_entries(segment.path()) else {
                continue;
            };
            if holds_tombstone(&entries) || !holds_location_file(&entries) {
                continue;
            }
            selected.push(Selected {
                target: Target {
                    day: day.clone(),
                    stream: identity.stream.to_owned(),
                    dir,
                },
                mixed: segment_is_mixed(&entries),
                cid: segment_cid(segment.path()),
            });
        }
    }
    LocationScan {
        targets: selected,
        complete,
    }
}

pub(crate) fn days_holding_tombstones(journal: &Path) -> Vec<String> {
    let mut days = Vec::new();
    let Ok(listed) = list_days(journal) else {
        return days;
    };
    for (day, _) in listed {
        let Ok(segments) = list_segments(journal, &day) else {
            continue;
        };
        let holds = segments.iter().any(|segment| {
            list_dir_entries(segment.path())
                .ok()
                .is_some_and(|entries| holds_tombstone(&entries))
        });
        if holds {
            days.push(day);
        }
    }
    days
}
