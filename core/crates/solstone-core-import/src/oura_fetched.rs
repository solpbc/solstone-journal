// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! What Oura runs have brought into the journal: the sync's own record.
//!
//! An ordinary Oura sync carries on from what it already fetched and doesn't
//! bring back what the owner deleted. It knows what it fetched from this
//! record: one line per day in `imports/.oura-fetched.jsonl`, each a short
//! hash of a row id an Oura run read and the journal then held, or that an
//! Oura import's delete took. It keeps no value or name; a row id is instants
//! and a source, so the record can confirm a guessed time was fetched. Every Oura run adds
//! to it; an ordinary sync leaves out a row it finds here that the journal no
//! longer holds. A range the owner asks for reads it, and ignores it.
//!
//! It is the sync's own business: deleting or erasing an import leaves it, so
//! an erased day stays out of later syncs. A journal that has none yet starts
//! it from the Oura rows it holds.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::Path;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use solstone_core_format::content::body_records::{BODY_RECORDS_FILE, RowKind, decode_rows};

/// The record, beside the import folders.
pub const FETCHED_REL: &str = "imports/.oura-fetched.jsonl";
const OURA_STREAM: &str = "import.oura";

/// A row id's mark in the record: the first 16 hex digits of its SHA-256.
fn mark(id: &str) -> String {
    let digest = Sha256::digest(id.as_bytes());
    digest
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// What the record holds, read once per run.
#[derive(Debug, Default)]
pub struct Fetched {
    /// Each mark, with the days it was recorded on.
    marks: HashMap<String, BTreeSet<String>>,
}

impl Fetched {
    /// Read the record, starting it from the journal's Oura rows when there is
    /// none. One that can't be read is an error: forgetting it would bring back
    /// everything the owner deleted.
    pub fn load(journal: &Path) -> Result<Self, String> {
        let path = journal.join(FETCHED_REL);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Self::start(journal);
            }
            Err(error) => return Err(format!("{}: {error}", path.display())),
        };
        let mut marks: HashMap<String, BTreeSet<String>> = HashMap::new();
        let lines: Vec<&str> = text
            .lines()
            .filter(|line| !line.trim().is_empty())
            .collect();
        for (index, line) in lines.iter().enumerate() {
            let Ok(entry) = serde_json::from_str::<Value>(line) else {
                // A line a stop cut short is the last one, and the rest still
                // count. Any other that can't be read would forget what it held.
                if index + 1 == lines.len() {
                    continue;
                }
                return Err(format!("{}: a line can't be read", path.display()));
            };
            let day = entry.get("day").and_then(Value::as_str);
            for value in entry
                .get("ids")
                .and_then(Value::as_array)
                .map_or(&[][..], Vec::as_slice)
            {
                if let Some(mark) = value.as_str() {
                    let days = marks.entry(mark.to_owned()).or_default();
                    days.extend(day.map(str::to_owned));
                }
            }
        }
        Ok(Self { marks })
    }

    /// The first record: every Oura row the journal holds now, by day.
    fn start(journal: &Path) -> Result<Self, String> {
        let mut by_day: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let chronicle = journal.join("chronicle");
        for day in fs::read_dir(&chronicle).into_iter().flatten().flatten() {
            let Some(day_name) = day.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            for window in fs::read_dir(day.path().join(OURA_STREAM))
                .into_iter()
                .flatten()
                .flatten()
            {
                if window
                    .file_name()
                    .to_str()
                    .is_none_or(|name| name.starts_with('.'))
                    || solstone_core_segment::owner_deleted(&window.path()).unwrap_or(true)
                {
                    continue;
                }
                let Ok(text) = fs::read_to_string(window.path().join(BODY_RECORDS_FILE)) else {
                    continue;
                };
                let Ok(rows) = decode_rows(&text) else {
                    continue;
                };
                by_day.entry(day_name.clone()).or_default().extend(
                    rows.iter()
                        .filter(|row| row.kind != RowKind::Moved)
                        .map(|row| mark(&row.id)),
                );
            }
        }
        let mut fetched = Self::default();
        let mut text = String::new();
        for (day, marks) in by_day {
            text.push_str(&json!({"day": day, "ids": marks}).to_string());
            text.push('\n');
            for mark in marks {
                fetched.marks.entry(mark).or_default().insert(day.clone());
            }
        }
        solstone_core_journal_io::atomic_replace(
            journal.join(FETCHED_REL),
            text.as_bytes(),
            solstone_core_journal_io::AtomicWriteOptions { mode: Some(0o600) },
        )
        .map_err(|error| format!("{FETCHED_REL}: {error}"))?;
        Ok(fetched)
    }

    /// Whether an Oura run brought a row with this id in before.
    #[must_use]
    pub fn contains(&self, id: &str) -> bool {
        self.marks.contains_key(&mark(id))
    }

    /// The days an Oura run recorded a row with this id on: where the journal
    /// first held it.
    #[must_use]
    pub fn days_of(&self, id: &str) -> Vec<String> {
        self.marks
            .get(&mark(id))
            .map(|days| days.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Add the rows a day now holds that the record doesn't have yet. Call it
    /// holding Oura's lock, once the day's rows are in the journal.
    pub fn record<'i>(
        &mut self,
        journal: &Path,
        day: &str,
        ids: impl IntoIterator<Item = &'i str>,
    ) -> Result<(), String> {
        let new: BTreeSet<String> = ids
            .into_iter()
            .map(mark)
            .filter(|mark| !self.marks.contains_key(mark))
            .collect();
        if new.is_empty() {
            return Ok(());
        }
        let path = journal.join(FETCHED_REL);
        let mut options = OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut file = options
            .open(&path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        let line = format!("{}\n", json!({"day": day, "ids": new}));
        file.write_all(line.as_bytes())
            .and_then(|()| file.sync_data())
            .map_err(|error| format!("{}: {error}", path.display()))?;
        for mark in new {
            self.marks.entry(mark).or_default().insert(day.to_owned());
        }
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "test code")]
mod tests {
    use super::*;

    #[test]
    fn it_remembers_what_it_recorded_and_a_cut_last_line_only() {
        let journal = tempfile::tempdir().unwrap();
        let root = journal.path();
        fs::create_dir_all(root.join("imports")).unwrap();
        let mut fetched = Fetched::load(root).unwrap();
        assert!(!fetched.contains("a"));
        fetched.record(root, "20261001", ["a", "b"]).unwrap();
        fetched.record(root, "20261002", ["b", "c"]).unwrap();
        let again = Fetched::load(root).unwrap();
        for id in ["a", "b", "c"] {
            assert!(again.contains(id), "{id}");
        }
        // Nothing but marks: no id is written as itself.
        let text = fs::read_to_string(root.join(FETCHED_REL)).unwrap();
        assert!(!text.contains("\"a\""));
        // A stop that cut the last line short loses that line only.
        let mut cut = text.clone();
        cut.push_str("{\"day\":\"2026");
        fs::write(root.join(FETCHED_REL), &cut).unwrap();
        assert!(Fetched::load(root).unwrap().contains("c"));
        // A line in the middle that can't be read is an error: forgetting it would
        // bring back what the owner deleted.
        let broken = text.replacen('{', "{x", 1);
        fs::write(root.join(FETCHED_REL), broken).unwrap();
        assert!(Fetched::load(root).is_err());
    }

    #[test]
    fn a_journal_with_no_record_starts_it_from_the_oura_rows_it_holds() {
        let journal = tempfile::tempdir().unwrap();
        let root = journal.path();
        let window = root.join("chronicle/20261001/import.oura/000000_86400");
        fs::create_dir_all(&window).unwrap();
        fs::write(
            window.join(BODY_RECORDS_FILE),
            r#"{"id":"r1","kind":"dated","date":"2026-10-01","by":"Oura","type":"daily_readiness","label":"readiness","v":1,"source":"oura"}"#,
        )
        .unwrap();
        fs::create_dir_all(root.join("imports")).unwrap();
        let fetched = Fetched::load(root).unwrap();
        assert!(fetched.contains("r1"));
        assert!(root.join(FETCHED_REL).is_file());
    }
}
