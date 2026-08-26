// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::Path;

use serde_json::Value;
use solstone_core_journal_io::{DirEntry, DirEntryKind, list_dir_entries};
use solstone_core_retention::receipt::Target;
use solstone_core_segment::{is_reserved_name, list_days, list_segments};

use crate::receipt::Issue;

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
    pub incomplete: Vec<Issue>,
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

fn segment_entries(path: &Path) -> Option<Vec<DirEntry>> {
    #[cfg(test)]
    if forced_unreadable_segment(path) {
        return None;
    }
    list_dir_entries(path).ok()
}

pub(crate) fn select_location_targets(journal: &Path) -> LocationScan {
    let mut selected = Vec::new();
    let days = match list_days(journal) {
        Ok(days) => days,
        Err(_) => {
            return LocationScan {
                targets: selected,
                complete: false,
                incomplete: Vec::new(),
            };
        }
    };
    let mut complete = true;
    let mut incomplete = Vec::new();
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
            let Some(entries) = segment_entries(segment.path()) else {
                complete = false;
                incomplete.push(Issue {
                    what: format!("chronicle/{day}/{}/{dir}", identity.stream),
                    plain_reason: "this segment directory could not be listed".to_owned(),
                });
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
        incomplete,
    }
}

#[cfg(test)]
thread_local! {
    static FORCED_UNREADABLE_SEGMENT: std::cell::RefCell<Option<std::path::PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
fn forced_unreadable_segment(path: &Path) -> bool {
    FORCED_UNREADABLE_SEGMENT.with(|forced| {
        forced
            .borrow()
            .as_ref()
            .is_some_and(|forced_path| forced_path == path)
    })
}

#[cfg(test)]
pub(crate) fn force_unreadable_segment(path: &Path) {
    FORCED_UNREADABLE_SEGMENT.with(|forced| *forced.borrow_mut() = Some(path.to_path_buf()));
}

#[cfg(test)]
pub(crate) fn clear_forced_unreadable_segment() {
    FORCED_UNREADABLE_SEGMENT.with(|forced| *forced.borrow_mut() = None);
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::{
        clear_forced_unreadable_segment, force_unreadable_segment, select_location_targets,
    };

    #[test]
    fn unreadable_segment_listing_is_incomplete_and_names_the_residue() {
        let temporary = TempDir::new_in("/var/tmp").unwrap();
        let path = temporary
            .path()
            .join("chronicle/20260805/location/070000_17");
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("location.jsonl"), b"{}\n").unwrap();
        fs::write(path.join("stream.json"), b"{}").unwrap();

        force_unreadable_segment(&path);
        let scan = select_location_targets(temporary.path());
        clear_forced_unreadable_segment();

        assert!(!scan.complete);
        assert!(scan.targets.is_empty());
        assert_eq!(scan.incomplete.len(), 1);
        assert_eq!(
            scan.incomplete[0].what,
            "chronicle/20260805/location/070000_17"
        );
        assert!(
            scan.incomplete[0]
                .plain_reason
                .contains("could not be listed")
        );
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
