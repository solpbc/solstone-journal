// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The Strava records that became code records.
//!
//! Each line names one record an earlier reading filed, by day, facet and id,
//! and holds nothing else. Thinking reads it: a record that's gone and is named
//! here is settled body data, so its receipt stands and no work waits on it.

use std::collections::BTreeSet;
use std::io;
use std::path::Path;

use serde_json::{Value, json};
use solstone_core_journal_io::{AppendError, append_text};

const CONVERTED_LIST: &str = "imports/.strava-converted.jsonl";

/// A record on the list.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct StravaConvertedLine {
    pub day: String,
    pub facet: String,
    pub id: String,
}

/// Every record on the list. A bad line costs only itself.
///
/// # Errors
///
/// The list exists and can't be read.
pub fn read_strava_converted(root: &Path) -> io::Result<BTreeSet<StravaConvertedLine>> {
    let bytes = match std::fs::read(root.join(CONVERTED_LIST)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(BTreeSet::new()),
        Err(error) => return Err(error),
    };
    Ok(bytes
        .split(|&byte| byte == b'\n')
        .filter_map(|line| serde_json::from_slice::<Value>(line).ok())
        .filter_map(|value| {
            Some(StravaConvertedLine {
                day: value.get("day")?.as_str()?.to_owned(),
                facet: value.get("facet")?.as_str()?.to_owned(),
                id: value.get("id")?.as_str()?.to_owned(),
            })
        })
        .collect())
}

/// Add records to the list, durably, leaving out any already on it.
///
/// # Errors
///
/// The list can't be read or written.
pub fn append_strava_converted(
    root: &Path,
    lines: &[StravaConvertedLine],
) -> Result<(), AppendError> {
    let known = read_strava_converted(root).unwrap_or_default();
    let path = root.join(CONVERTED_LIST);
    let torn = std::fs::read(&path).is_ok_and(|bytes| bytes.last().is_some_and(|&b| b != b'\n'));
    let mut text = String::new();
    for line in lines.iter().filter(|line| !known.contains(line)) {
        if !text.is_empty() || torn {
            text.push('\n');
        }
        text.push_str(&json!({"day": line.day, "facet": line.facet, "id": line.id}).to_string());
    }
    if text.is_empty() {
        return Ok(());
    }
    append_text(&path, &text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(id: &str) -> StravaConvertedLine {
        StravaConvertedLine {
            day: "20260101".to_owned(),
            facet: "a".to_owned(),
            id: id.to_owned(),
        }
    }

    #[test]
    fn a_missing_list_is_empty_and_lines_are_added_once() {
        let root = tempfile::tempdir().unwrap();
        assert!(read_strava_converted(root.path()).unwrap().is_empty());
        std::fs::create_dir_all(root.path().join("imports")).unwrap();
        append_strava_converted(root.path(), &[line("x"), line("y")]).unwrap();
        append_strava_converted(root.path(), &[line("y"), line("z")]).unwrap();
        let read = read_strava_converted(root.path()).unwrap();
        assert_eq!(read.len(), 3);
        let text = std::fs::read_to_string(root.path().join(CONVERTED_LIST)).unwrap();
        assert_eq!(text.lines().count(), 3);
    }

    #[test]
    fn a_torn_line_costs_only_itself_and_the_next_append_survives_it() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("imports")).unwrap();
        std::fs::write(root.path().join(CONVERTED_LIST), b"{\"day\":\"2026").unwrap();
        append_strava_converted(root.path(), &[line("x")]).unwrap();
        assert_eq!(
            read_strava_converted(root.path()).unwrap(),
            BTreeSet::from([line("x")])
        );
    }
}
