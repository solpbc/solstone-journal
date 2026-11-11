// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! What a hide keeps out of search, decided as each file is indexed.
//!
//! A hide moves no file: what is hidden is written in a journal file this
//! module reads. A hidden activity's entry, a hidden person's notes, a name
//! hidden in a facet's detections, an output of a hidden activity, and a week
//! page with anything left out are kept out of the index; every other entry
//! of the same file keeps its position. When a hide can't be decided because
//! the file that says it can't be read, the file is indexed with nothing and
//! tried again on the next scan.
//!
//! One [`HideCache`] serves one scan pass, one scan turn, one drain batch or
//! one rescan, so each facet's hides and each day's hidden activities are read
//! once for all the files that need them.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::rc::Rc;

use serde_json::Value;
use solstone_core_entity::hidden_names::{HiddenKeys, facet_hidden_keys, link_folder_hidden};
use solstone_core_format::content::week_left_out::{
    LEFT_OUT_RELATIVE_PATH, LeftOut, WeekPageKeys, read_left_out, read_week_page_keys,
};
use solstone_core_format::content::{Family, IndexChunk, indexed, is_hidden_activity};
use solstone_core_format::paths::resolve_journal_path;

/// What of one file goes into the index.
#[derive(Debug, Clone)]
pub(crate) enum FileHide {
    /// Every chunk [`indexed`] keeps.
    Index,
    /// The detected rows whose names the facet doesn't hide.
    DetectedNames(Rc<HiddenKeys>),
    /// Nothing: the whole file is hidden.
    Skip,
    /// Nothing for now: what is hidden can't be read. Says which store.
    Undecidable(String),
}

impl FileHide {
    /// Whether `chunk` of a `family` file goes into the index.
    pub(crate) fn keeps(&self, family: Family, chunk: &IndexChunk) -> bool {
        if !indexed(family, chunk) {
            return false;
        }
        match self {
            Self::Index => true,
            Self::DetectedNames(keys) => !chunk
                .source
                .as_ref()
                .and_then(|source| source.get("name"))
                .and_then(Value::as_str)
                .is_some_and(|name| keys.is_hidden(name)),
            Self::Skip | Self::Undecidable(_) => false,
        }
    }
}

/// The rows of one activity day file, as far as hides go.
#[derive(Debug, Default)]
struct DayRows {
    hidden: HashSet<String>,
    visible: HashSet<String>,
    /// A line that isn't a record: it could be any id's row.
    damaged: bool,
    /// The file couldn't be read at all.
    unreadable: Option<String>,
}

/// The hides read so far in one pass.
#[derive(Default)]
pub(crate) struct HideCache {
    facets: HashMap<String, Result<Rc<HiddenKeys>, String>>,
    folders: HashMap<(String, String), Result<bool, String>>,
    days: HashMap<(String, String), Rc<DayRows>>,
    left_out: Option<LeftOut>,
}

impl HideCache {
    /// What of the file at `rel` goes into the index.
    pub(crate) fn decide(&mut self, journal: &Path, rel: &str) -> FileHide {
        let parts: Vec<&str> = rel.split('/').collect();
        match parts.as_slice() {
            ["facets", facet, "entities", file] if is_detected_day(file) => {
                match self.facet_keys(journal, facet) {
                    Ok(keys) => FileHide::DetectedNames(keys),
                    Err(error) => FileHide::Undecidable(error),
                }
            }
            ["facets", facet, "entities", folder, "observations.jsonl"] => {
                match self.folder_hidden(journal, facet, folder) {
                    Ok(true) => FileHide::Skip,
                    Ok(false) => FileHide::Index,
                    Err(error) => FileHide::Undecidable(error),
                }
            }
            ["facets", facet, "activities", day, id, file] if file.ends_with(".md") => {
                let rows = self.day_rows(journal, facet, day);
                if rows.hidden.contains(*id) {
                    FileHide::Skip
                } else if rows.visible.contains(*id) {
                    FileHide::Index
                } else if let Some(error) = &rows.unreadable {
                    FileHide::Undecidable(error.clone())
                } else if rows.damaged {
                    FileHide::Undecidable(format!(
                        "can't tell whether this activity is hidden: facets/{facet}/activities/{day}.jsonl has a line that isn't a record"
                    ))
                } else {
                    FileHide::Index
                }
            }
            ["reflections", "weekly", file] => match file.strip_suffix(".md") {
                Some(start) => self.week_page(journal, start),
                None => FileHide::Index,
            },
            _ => FileHide::Index,
        }
    }

    fn facet_keys(&mut self, journal: &Path, facet: &str) -> Result<Rc<HiddenKeys>, String> {
        self.facets
            .entry(facet.to_owned())
            .or_insert_with(|| {
                facet_hidden_keys(journal, facet)
                    .map(Rc::new)
                    .map_err(|error| error.to_string())
            })
            .clone()
    }

    /// Whether a link folder's notes are hidden. The facet's own answer
    /// stands when it has one; when something elsewhere in the facet can't be
    /// read, the folder is decided alone.
    fn folder_hidden(&mut self, journal: &Path, facet: &str, folder: &str) -> Result<bool, String> {
        if let Ok(keys) = self.facet_keys(journal, facet) {
            return Ok(keys.hides_folder(folder));
        }
        self.folders
            .entry((facet.to_owned(), folder.to_owned()))
            .or_insert_with(|| {
                link_folder_hidden(journal, facet, folder).map_err(|error| error.to_string())
            })
            .clone()
    }

    fn day_rows(&mut self, journal: &Path, facet: &str, day: &str) -> Rc<DayRows> {
        Rc::clone(
            self.days
                .entry((facet.to_owned(), day.to_owned()))
                .or_insert_with(|| Rc::new(read_day_rows(journal, facet, day))),
        )
    }

    fn week_page(&mut self, journal: &Path, start: &str) -> FileHide {
        let left_out = self.left_out.get_or_insert_with(|| read_left_out(journal));
        let keys = match left_out {
            LeftOut::None => return FileHide::Index,
            LeftOut::Keys(keys) if keys.is_empty() => return FileHide::Index,
            LeftOut::Keys(keys) => keys,
            LeftOut::Unreadable => {
                return FileHide::Undecidable(format!(
                    "can't tell what is left out of week pages: {LEFT_OUT_RELATIVE_PATH} can't be read"
                ));
            }
        };
        match read_week_page_keys(journal, start) {
            WeekPageKeys::Keys(page) if page.is_disjoint(keys) => FileHide::Index,
            WeekPageKeys::Keys(_) => FileHide::Skip,
            WeekPageKeys::Absent | WeekPageKeys::Unreadable => FileHide::Undecidable(format!(
                "can't tell what is left out of this week page: reflections/weekly/{start}.json can't be read"
            )),
        }
    }
}

/// A facet's detected names for a day, named as the index names them: any
/// all-digit file name, not only a valid `YYYYMMDD`.
fn is_detected_day(file: &str) -> bool {
    file.strip_suffix(".jsonl")
        .is_some_and(|stem| !stem.is_empty() && stem.bytes().all(|byte| byte.is_ascii_digit()))
}

fn read_day_rows(journal: &Path, facet: &str, day: &str) -> DayRows {
    let rel = format!("facets/{facet}/activities/{day}.jsonl");
    let text = match resolve_journal_path(journal, &rel).map(fs::read_to_string) {
        Ok(Ok(text)) => text,
        Ok(Err(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            return DayRows::default();
        }
        Ok(Err(error)) => {
            return DayRows {
                unreadable: Some(format!(
                    "can't tell whether this activity is hidden: {rel} can't be read: {error}"
                )),
                ..DayRows::default()
            };
        }
        Err(error) => {
            return DayRows {
                unreadable: Some(format!(
                    "can't tell whether this activity is hidden: {rel} can't be read: {error}"
                )),
                ..DayRows::default()
            };
        }
    };
    let mut rows = DayRows::default();
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let Ok(Value::Object(record)) = serde_json::from_str::<Value>(line) else {
            rows.damaged = true;
            continue;
        };
        let Some(id) = record.get("id").and_then(Value::as_str) else {
            continue;
        };
        if is_hidden_activity(&record) {
            rows.hidden.insert(id.to_owned());
        } else {
            rows.visible.insert(id.to_owned());
        }
    }
    rows
}
