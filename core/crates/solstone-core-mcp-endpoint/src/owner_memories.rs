// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The owner's read of what each connected agent saved, one agent at a time.
//!
//! This reads the journal's files and not the search index, so a memory that
//! failed to index can still be seen and deleted. It walks days and never the
//! stream's chain: deleting a memory removes the marker its neighbour's chain
//! runs through.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Json;
use axum::extract::{Extension, Path as RoutePath, Query};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Value, json};
use solstone_core_format::agent_memory::{Coordinate, SourceKey};
use solstone_core_memory_original::{OriginalRead, read_original};
use solstone_core_segment::{OwnerDeletionState, owner_deletion_state};

use crate::owner_web::refusal;

const STREAM_PREFIX: &str = "agent-memory-";
const PAGE_SIZE: usize = 30;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct Place {
    day: String,
    segment: String,
}

/// Every memory on disk, newest first per source, and whether the walk saw it all.
#[derive(Debug, Default)]
struct Found {
    by_source: BTreeMap<String, Vec<Place>>,
    complete: bool,
}

fn is_day(name: &str) -> bool {
    name.len() == 8 && name.bytes().all(|byte| byte.is_ascii_digit())
}

fn is_segment(name: &str) -> bool {
    name.split_once('_').is_some_and(|(time, length)| {
        time.len() == 6
            && time.bytes().all(|byte| byte.is_ascii_digit())
            && !length.is_empty()
            && length.bytes().all(|byte| byte.is_ascii_digit())
    })
}

fn parse_source(component: &str) -> Option<SourceKey> {
    SourceKey::parse(format!("sha256:{component}")).ok()
}

/// The directories under `dir` with their names, without following a link.
fn child_dirs(dir: &Path) -> Result<Vec<(String, PathBuf)>, std::io::Error> {
    let mut children = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        if let Some(name) = entry.file_name().to_str() {
            children.push((name.to_owned(), entry.path()));
        }
    }
    Ok(children)
}

fn find_memories(journal: &Path, only: Option<&str>) -> Found {
    let mut found = Found {
        complete: true,
        ..Found::default()
    };
    let days = match child_dirs(&journal.join("chronicle")) {
        Ok(days) => days,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return found,
        Err(_) => {
            found.complete = false;
            return found;
        }
    };
    for (day, day_path) in days {
        if !is_day(&day) {
            continue;
        }
        let Ok(streams) = child_dirs(&day_path) else {
            found.complete = false;
            continue;
        };
        for (stream, stream_path) in streams {
            let Some(source) = stream.strip_prefix(STREAM_PREFIX) else {
                continue;
            };
            if parse_source(source).is_none() || only.is_some_and(|wanted| wanted != source) {
                continue;
            }
            let Ok(segments) = child_dirs(&stream_path) else {
                found.complete = false;
                continue;
            };
            for (segment, segment_path) in segments {
                if !is_segment(&segment) {
                    continue;
                }
                // A marker this can't read still lists the memory: its row says
                // it can't be read, and the owner can delete it.
                if matches!(
                    owner_deletion_state(&segment_path),
                    Ok(OwnerDeletionState::Deleted | OwnerDeletionState::Staged)
                ) {
                    continue;
                }
                found
                    .by_source
                    .entry(source.to_owned())
                    .or_default()
                    .push(Place {
                        day: day.clone(),
                        segment,
                    });
            }
        }
    }
    for places in found.by_source.values_mut() {
        places.sort_unstable_by(|a, b| b.cmp(a));
    }
    found
}

fn read_place(journal: &Path, source: &SourceKey, place: &Place) -> OriginalRead {
    let coordinate = Coordinate {
        day: place.day.clone(),
        stream: format!("{STREAM_PREFIX}{}", source.component()),
        segment: place.segment.clone(),
    };
    read_original(journal, source, &coordinate)
}

fn summary_value(journal: &Path) -> Value {
    let found = find_memories(journal, None);
    let agents = found
        .by_source
        .iter()
        .filter_map(|(component, places)| {
            let source = parse_source(component)?;
            let newest =
                places
                    .iter()
                    .find_map(|place| match read_place(journal, &source, place) {
                        OriginalRead::Ready { origin, .. } => Some(origin),
                        _ => None,
                    });
            Some(json!({
                "source": component,
                "count": places.len(),
                "latest_at": newest.as_ref().map(|origin| origin.created_at),
                "label": newest.as_ref().map(|origin| origin.creation_label.as_str()),
            }))
        })
        .collect::<Vec<_>>();
    json!({"agents": agents, "complete": found.complete})
}

fn row(journal: &Path, source: &SourceKey, place: &Place) -> Option<Value> {
    let stream = format!("{STREAM_PREFIX}{}", source.component());
    match read_place(journal, source, place) {
        OriginalRead::Ready { bytes, origin, .. } => Some(json!({
            "day": place.day,
            "stream": stream,
            "segment": place.segment,
            "created_at": origin.created_at,
            "content": String::from_utf8_lossy(&bytes),
        })),
        OriginalRead::Absent | OriginalRead::Deleted | OriginalRead::Staged => None,
        OriginalRead::Unready | OriginalRead::Corrupt | OriginalRead::Unavailable { .. } => {
            Some(json!({
                "day": place.day,
                "stream": stream,
                "segment": place.segment,
                "unreadable": true,
            }))
        }
    }
}

fn page_value(journal: &Path, source: &SourceKey, before: Option<&Place>) -> Value {
    sized_page_value(journal, source, before, PAGE_SIZE)
}

fn sized_page_value(
    journal: &Path,
    source: &SourceKey,
    before: Option<&Place>,
    size: usize,
) -> Value {
    let component = source.component();
    let mut found = find_memories(journal, Some(component));
    let places = found.by_source.remove(component).unwrap_or_default();
    let mut remaining = places
        .iter()
        .filter(|place| before.is_none_or(|before| *place < before));
    let mut memories = Vec::new();
    let mut last = None;
    for place in remaining.by_ref() {
        last = Some(place);
        if let Some(memory) = row(journal, source, place) {
            memories.push(memory);
            if memories.len() == size {
                break;
            }
        }
    }
    let next = remaining
        .next()
        .and(last)
        .map(|place| json!({"day": place.day, "segment": place.segment}));
    json!({
        "source": component,
        "memories": memories,
        "next": next,
        "complete": found.complete,
    })
}

fn unavailable() -> Response {
    refusal(
        "agent_memories_unavailable",
        "your journal couldn't read its agents' memories",
        StatusCode::INTERNAL_SERVER_ERROR,
    )
}

pub(crate) async fn summary(Extension(journal): Extension<Arc<PathBuf>>) -> Response {
    match tokio::task::spawn_blocking(move || summary_value(&journal)).await {
        Ok(value) => Json(value).into_response(),
        Err(_) => unavailable(),
    }
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct PageParams {
    before_day: Option<String>,
    before_segment: Option<String>,
}

pub(crate) async fn page(
    Extension(journal): Extension<Arc<PathBuf>>,
    RoutePath(component): RoutePath<String>,
    Query(params): Query<PageParams>,
) -> Response {
    let Some(source) = parse_source(&component) else {
        return refusal(
            "agent_memory_source_invalid",
            "that isn't an agent's key",
            StatusCode::BAD_REQUEST,
        );
    };
    let before = match (params.before_day, params.before_segment) {
        (None, None) => None,
        (Some(day), Some(segment)) if is_day(&day) && is_segment(&segment) => {
            Some(Place { day, segment })
        }
        _ => {
            return refusal(
                "agent_memory_cursor_invalid",
                "that isn't a place in an agent's memories",
                StatusCode::BAD_REQUEST,
            );
        }
    };
    match tokio::task::spawn_blocking(move || page_value(&journal, &source, before.as_ref())).await
    {
        Ok(value) => Json(value).into_response(),
        Err(_) => unavailable(),
    }
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Duration, TimeZone, Utc};
    use tempfile::TempDir;

    use super::*;
    use crate::memory::{AppendResult, AuthenticatedMemorySource, append_connection_memory};

    const ONE: &str = "bearer:one";
    const TWO: &str = "oauth:two";

    fn journal() -> TempDir {
        let journal = TempDir::new().expect("journal");
        fs::create_dir_all(journal.path().join("chronicle")).expect("chronicle");
        journal
    }

    fn at(minutes: i64) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 3, 12, 0, 0).unwrap() + Duration::minutes(minutes)
    }

    fn save(journal: &TempDir, id: &str, label: &str, text: &str, minutes: i64) -> Coordinate {
        let source = AuthenticatedMemorySource {
            verified_id: id,
            creation_label: label,
        };
        let operation = format!("op-{minutes}");
        match append_connection_memory(journal.path(), source, text, &operation, at(minutes)) {
            Ok(AppendResult::Stored(receipt)) => receipt.coordinate,
            _ => panic!("the memory should save"),
        }
    }

    fn contents(value: &Value) -> Vec<String> {
        value["memories"]
            .as_array()
            .unwrap()
            .iter()
            .map(|memory| memory["content"].as_str().unwrap_or("").to_owned())
            .collect()
    }

    #[test]
    fn a_page_holds_one_agents_memories_newest_first() {
        let journal = journal();
        save(&journal, ONE, "Claude", "first", 0);
        save(&journal, TWO, "Claude", "other agent", 1);
        save(&journal, ONE, "Claude", "second", 2);
        let source = SourceKey::from_verified_id(ONE);
        let page = page_value(journal.path(), &source, None);
        assert_eq!(contents(&page), ["second", "first"]);
        assert_eq!(page["complete"], true);
        assert!(page["next"].is_null());
    }

    #[test]
    fn the_summary_names_each_agent_with_its_count_and_latest_save() {
        let journal = journal();
        save(&journal, ONE, "Claude", "first", 0);
        save(&journal, ONE, "Claude", "second", 2);
        save(&journal, TWO, "Codex", "other", 1);
        let summary = summary_value(journal.path());
        let agents = summary["agents"].as_array().unwrap();
        assert_eq!(agents.len(), 2);
        let one = SourceKey::from_verified_id(ONE);
        let found = agents
            .iter()
            .find(|agent| agent["source"] == one.component())
            .unwrap();
        assert_eq!(found["count"], 2);
        assert_eq!(found["label"], "Claude");
        assert_eq!(found["latest_at"], json!(at(2)));
    }

    #[test]
    fn pages_walk_every_memory_once() {
        let journal = journal();
        for index in 0..3 {
            save(&journal, ONE, "Claude", &format!("note {index}"), index);
        }
        let source = SourceKey::from_verified_id(ONE);
        let first = sized_page_value(journal.path(), &source, None, 2);
        assert_eq!(contents(&first), ["note 2", "note 1"]);
        let before = Place {
            day: first["next"]["day"].as_str().unwrap().to_owned(),
            segment: first["next"]["segment"].as_str().unwrap().to_owned(),
        };
        let second = sized_page_value(journal.path(), &source, Some(&before), 2);
        assert_eq!(contents(&second), ["note 0"]);
        assert!(second["next"].is_null());
    }

    #[test]
    fn a_deleted_memory_leaves_the_page_and_the_count() {
        let journal = journal();
        let kept = save(&journal, ONE, "Claude", "kept", 0);
        let gone = save(&journal, ONE, "Claude", "gone", 1);
        let target = solstone_core_retention::receipt::Target {
            day: gone.day.clone(),
            stream: gone.stream.clone(),
            dir: gone.segment.clone(),
        };
        let removed = solstone_core_retention::door::remove_segments(
            journal.path(),
            std::slice::from_ref(&target),
            &at(2).to_rfc3339(),
            solstone_core_retention::tombstone::RemovalReason::OwnerSegmentDelete,
            "journal-owner",
        );
        assert!(removed.removed_paths().next().is_some());
        let source = SourceKey::from_verified_id(ONE);
        let page = page_value(journal.path(), &source, None);
        assert_eq!(contents(&page), ["kept"]);
        assert_eq!(page["memories"][0]["segment"], kept.segment.as_str());
        assert_eq!(summary_value(journal.path())["agents"][0]["count"], 1);
    }

    #[test]
    fn a_memory_that_cannot_be_read_says_so_and_can_still_be_found() {
        let journal = journal();
        let bad = save(&journal, ONE, "Claude", "will break", 0);
        save(&journal, ONE, "Claude", "fine", 1);
        let note = journal
            .path()
            .join("chronicle")
            .join(&bad.day)
            .join(&bad.stream)
            .join(&bad.segment)
            .join("note.txt");
        fs::write(note, "altered").unwrap();
        let source = SourceKey::from_verified_id(ONE);
        let page = page_value(journal.path(), &source, None);
        let memories = page["memories"].as_array().unwrap();
        assert_eq!(memories.len(), 2);
        assert_eq!(memories[0]["content"], "fine");
        assert_eq!(memories[1]["unreadable"], true);
        assert_eq!(memories[1]["segment"], bad.segment.as_str());
        assert!(memories[1].get("content").is_none());
    }

    #[test]
    fn a_journal_with_no_memories_is_complete_and_empty() {
        let journal = TempDir::new().expect("journal");
        let summary = summary_value(journal.path());
        assert_eq!(summary["agents"], json!([]));
        assert_eq!(summary["complete"], true);
    }
}
