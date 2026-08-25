// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Fail-closed decoding and selection for whole-journal restore snapshots.

use chrono::{DateTime, FixedOffset};
use serde_json::{Map, Value};

use crate::ARCHIVE_TAG;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct JournalSnapshot {
    pub id: String,
    pub time: DateTime<FixedOffset>,
    pub path: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CatalogError {
    Invalid,
    NotFound,
    Ambiguous,
}

pub(crate) fn select_journal_snapshot(
    catalog: Option<&Value>,
) -> Result<JournalSnapshot, CatalogError> {
    let entries = catalog
        .and_then(Value::as_array)
        .ok_or(CatalogError::Invalid)?;
    let mut candidates = Vec::new();
    for entry in entries {
        let object = entry.as_object().ok_or(CatalogError::Invalid)?;
        if is_archive(object)? {
            continue;
        }
        candidates.push(journal_snapshot(object)?);
    }
    let latest = candidates
        .iter()
        .map(|candidate| candidate.time)
        .max()
        .ok_or(CatalogError::NotFound)?;
    let mut latest_candidates = candidates
        .into_iter()
        .filter(|candidate| candidate.time == latest);
    let selected = latest_candidates
        .next()
        .expect("a maximum instant has at least one candidate");
    if latest_candidates.next().is_some() {
        Err(CatalogError::Ambiguous)
    } else {
        Ok(selected)
    }
}

fn is_archive(entry: &Map<String, Value>) -> Result<bool, CatalogError> {
    match entry.get("tags") {
        None => Ok(false),
        Some(Value::Array(tags)) if tags.len() == 1 && tags[0].as_str() == Some(ARCHIVE_TAG) => {
            Ok(true)
        }
        Some(_) => Err(CatalogError::Invalid),
    }
}

fn journal_snapshot(entry: &Map<String, Value>) -> Result<JournalSnapshot, CatalogError> {
    let id = entry
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| is_full_lower_hex_id(id))
        .ok_or(CatalogError::Invalid)?
        .to_owned();
    let time = entry
        .get("time")
        .and_then(Value::as_str)
        .and_then(|time| DateTime::parse_from_rfc3339(time).ok())
        .ok_or(CatalogError::Invalid)?;
    let paths = entry
        .get("paths")
        .and_then(Value::as_array)
        .filter(|paths| paths.len() == 1)
        .ok_or(CatalogError::Invalid)?;
    let path = paths[0]
        .as_str()
        .filter(|path| !path.is_empty())
        .ok_or(CatalogError::Invalid)?
        .to_owned();
    Ok(JournalSnapshot { id, time, path })
}

fn is_full_lower_hex_id(id: &str) -> bool {
    id.len() == 64
        && id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const FIRST_ID: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const SECOND_ID: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const THIRD_ID: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

    fn snapshot(id: &str, time: &str) -> Value {
        json!({"id": id, "time": time, "paths": ["/journal"]})
    }

    #[test]
    fn catalog_accepts_untagged_and_skips_archive() {
        let selected = select_journal_snapshot(Some(&json!([
            {"tags":[ARCHIVE_TAG]},
            snapshot(FIRST_ID, "2026-01-01T00:00:00.000000001+00:00"),
        ])))
        .unwrap();
        assert_eq!(selected.id, FIRST_ID);
        assert_eq!(selected.path, "/journal");
    }

    #[test]
    fn catalog_rejects_all_other_tag_shapes() {
        for tags in [
            Value::Null,
            json!([]),
            json!([ARCHIVE_TAG, ARCHIVE_TAG]),
            json!(["different"]),
            json!([ARCHIVE_TAG, "different"]),
            json!(ARCHIVE_TAG),
        ] {
            let mut entry = snapshot(FIRST_ID, "2026-01-01T00:00:00Z");
            entry["tags"] = tags;
            assert_eq!(
                select_journal_snapshot(Some(&json!([entry]))),
                Err(CatalogError::Invalid)
            );
        }
    }

    #[test]
    fn catalog_rejects_invalid_candidates() {
        for entry in [
            json!({"id":"short", "time":"2026-01-01T00:00:00Z", "paths":["/journal"]}),
            json!({"id":FIRST_ID.to_uppercase(), "time":"2026-01-01T00:00:00Z", "paths":["/journal"]}),
            json!({"id":FIRST_ID, "time":"not-a-time", "paths":["/journal"]}),
            json!({"id":FIRST_ID, "time":"2026-01-01T00:00:00Z", "paths":[]}),
            json!({"id":FIRST_ID, "time":"2026-01-01T00:00:00Z", "paths":["/a", "/b"]}),
            json!({"id":FIRST_ID, "time":"2026-01-01T00:00:00Z", "paths":[""]}),
        ] {
            assert_eq!(
                select_journal_snapshot(Some(&json!([entry]))),
                Err(CatalogError::Invalid)
            );
        }
        assert_eq!(
            select_journal_snapshot(Some(&json!({}))),
            Err(CatalogError::Invalid)
        );
    }

    #[test]
    fn catalog_ignores_nonmaximum_ties_regardless_of_input_order() {
        for entries in [
            vec![
                snapshot(FIRST_ID, "2026-01-01T00:00:00Z"),
                snapshot(SECOND_ID, "2026-01-01T00:00:00Z"),
                snapshot(THIRD_ID, "2026-01-01T00:30:00Z"),
            ],
            vec![
                snapshot(THIRD_ID, "2026-01-01T00:30:00Z"),
                snapshot(FIRST_ID, "2026-01-01T00:00:00Z"),
                snapshot(SECOND_ID, "2026-01-01T00:00:00Z"),
            ],
        ] {
            let selected = select_journal_snapshot(Some(&Value::Array(entries))).unwrap();
            assert_eq!(selected.id, THIRD_ID);
        }
    }

    #[test]
    fn catalog_selects_unique_latest_in_the_middle() {
        let selected = select_journal_snapshot(Some(&json!([
            snapshot(FIRST_ID, "2026-01-01T01:00:00+01:00"),
            snapshot(SECOND_ID, "2026-01-01T00:30:00+00:00"),
            snapshot(THIRD_ID, "2026-01-01T00:00:00+00:00"),
        ])))
        .unwrap();
        assert_eq!(selected.id, SECOND_ID);
    }

    #[test]
    fn catalog_rejects_equal_latest_instants_and_empty_candidate_sets() {
        assert_eq!(
            select_journal_snapshot(Some(&json!([
                snapshot(FIRST_ID, "2026-01-01T12:00:00+00:00"),
                snapshot(SECOND_ID, "2026-01-01T13:00:00+01:00"),
                snapshot(THIRD_ID, "2026-01-01T11:00:00+00:00"),
            ]))),
            Err(CatalogError::Ambiguous)
        );
        assert_eq!(
            select_journal_snapshot(Some(&json!([{"tags":[ARCHIVE_TAG]}]))),
            Err(CatalogError::NotFound)
        );
    }
}
