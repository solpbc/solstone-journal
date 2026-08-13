// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use serde_json::Value;
use solstone_core_journal_io::{DirEntryKind, list_dir_entries};

use super::paths::history_dir;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryStop {
    Malformed { line: usize },
    Io,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HistoryRead {
    pub records: Vec<Value>,
    pub stopped: Option<HistoryStop>,
}

/// Python-compatible JSONL reader: retain preceding rows then stop at the
/// first malformed line or I/O failure.
pub fn load_history(path: &Path) -> HistoryRead {
    let Ok(file) = File::open(path) else {
        return HistoryRead {
            records: Vec::new(),
            stopped: None,
        };
    };
    let mut records = Vec::new();
    for (index, line) in BufReader::new(file).lines().enumerate() {
        let line = match line {
            Ok(line) => line,
            Err(_) => {
                return HistoryRead {
                    records,
                    stopped: Some(HistoryStop::Io),
                };
            }
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str(line) {
            Ok(value) => records.push(value),
            Err(_) => {
                return HistoryRead {
                    records,
                    stopped: Some(HistoryStop::Malformed { line: index + 1 }),
                };
            }
        }
    }
    HistoryRead {
        records,
        stopped: None,
    }
}

/// List history day stems without creating the history directory. Missing
/// history is a legitimate empty set; other directory failures are surfaced.
pub fn history_days(journal_root: &Path, prefix: &str) -> Result<Vec<String>, String> {
    let directory = history_dir(journal_root, prefix);
    let entries = list_dir_entries(&directory).map_err(|error| error.to_string())?;
    let mut days = Vec::new();
    for entry in entries {
        if entry.kind != DirEntryKind::File {
            continue;
        }
        let Some(stem) = entry
            .name
            .to_str()
            .and_then(|name| name.strip_suffix(".jsonl"))
        else {
            continue;
        };
        days.push(stem.to_owned());
    }
    days.sort();
    Ok(days)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn stops_at_first_malformed_line_without_mutating_file() {
        let path = std::env::temp_dir().join(format!(
            "observer-history-{}.jsonl",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        let contents = "{\"segment\":\"one\"}\n{broken}\n{\"segment\":\"three\"}\n";
        fs::write(&path, contents).expect("write");
        let read = load_history(&path);
        assert_eq!(read.records.len(), 1);
        assert_eq!(read.stopped, Some(HistoryStop::Malformed { line: 2 }));
        assert_eq!(fs::read_to_string(&path).expect("read"), contents);
        fs::remove_file(path).expect("cleanup");
    }
}
