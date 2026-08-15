// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::PathBuf;

use serde_json::{Map, Value};
use solstone_core_indexer_store::scan::{RescanFileStatus, rescan_file};
use solstone_core_journal_io::{AtomicWriteOptions, MalformedPolicy, read_jsonl, write_jsonl};

use crate::contract::{CommitDisposition, CommitPlan};
use crate::{ExecutionContext, PreparedTalent, StageError};

#[cfg(test)]
thread_local! {
    static TEST_INDEX_WARNINGS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[derive(Clone, Debug, PartialEq)]
pub enum WriteIntent {
    DayAccumulator {
        day: String,
        agent: String,
        record: Map<String, Value>,
    },
    Story {
        talent: String,
        facet: String,
        day: String,
        record_id: String,
        value: Value,
    },
}

pub fn write_output_if_configured(prepared: &PreparedTalent, output: &str) -> Result<bool, String> {
    let Some(path) = prepared.config.get("output_path").and_then(Value::as_str) else {
        return Ok(false);
    };
    write_output(PathBuf::from(path), output).map_err(|error| error.to_string())
}

pub fn write_output(path: PathBuf, output: &str) -> Result<bool, std::io::Error> {
    let bytes = output.as_bytes();
    if path.exists() && fs::read(&path)? == bytes {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    // Python uses a plain unlocked write here; do not add atomicity or a lock.
    fs::write(path, bytes)?;
    Ok(true)
}

pub fn apply(
    plan: CommitPlan,
    context: &ExecutionContext,
) -> Result<CommitDisposition, StageError> {
    match plan {
        CommitPlan::NoOutput => Ok(CommitDisposition::CommittedNoOutput),
        CommitPlan::Write(WriteIntent::DayAccumulator {
            day,
            agent,
            mut record,
        }) => {
            append_day_record(&context.journal, &day, &agent, &mut record)?;
            Ok(CommitDisposition::Written)
        }
        CommitPlan::Write(WriteIntent::Story {
            talent,
            facet,
            day,
            record_id,
            value,
        }) => {
            crate::story::apply_story(&context.journal, &talent, &facet, &day, &record_id, &value)
                .map_err(|detail| StageError {
                    phase: "commit",
                    stage: "story",
                    talent: "story".to_owned(),
                    detail,
                })?;
            Ok(CommitDisposition::CommittedNoOutput)
        }
    }
}

pub fn append_day_record(
    journal: &std::path::Path,
    day: &str,
    agent: &str,
    record: &mut Map<String, Value>,
) -> Result<(), StageError> {
    if !record.contains_key("ts") {
        record.insert(
            "ts".to_owned(),
            Value::from(chrono::Utc::now().timestamp_millis()),
        );
    }
    let path = journal
        .join("chronicle")
        .join(day)
        .join("talents")
        .join(format!("{agent}.jsonl"));
    let mut records: Vec<Value> = read_jsonl(&path, Vec::new(), MalformedPolicy::Skip)
        .map_err(|error| stage_error(agent, error.to_string()))?;
    records.push(Value::Object(record.clone()));
    write_jsonl(&path, records, AtomicWriteOptions { mode: Some(0o600) })
        .map_err(|error| stage_error(agent, error.to_string()))?;
    match rescan_file(journal, &path) {
        Ok(RescanFileStatus::Indexed { warnings }) => {
            for warning in warnings {
                index_warning(&format!("talent accumulator index warning: {warning}"));
            }
        }
        Ok(RescanFileStatus::Declined) => {}
        Err(error) => index_warning(&format!("talent accumulator index failed: {error}")),
    }
    Ok(())
}

fn index_warning(message: &str) {
    log::warn!("{message}");
    #[cfg(test)]
    TEST_INDEX_WARNINGS.with(|warnings| warnings.set(warnings.get() + 1));
}

fn stage_error(stage: &str, detail: String) -> StageError {
    StageError {
        phase: "write-intent",
        stage: "day-accumulator",
        talent: stage.to_owned(),
        detail,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::MetadataExt;

    use nix::fcntl::{Flock, FlockArg};

    use serde_json::json;

    use super::*;

    fn reset_index_warnings() {
        TEST_INDEX_WARNINGS.with(|warnings| warnings.set(0));
    }

    fn index_warnings() -> usize {
        TEST_INDEX_WARNINGS.with(|warnings| warnings.get())
    }

    #[test]
    fn criterion_6_output_guard_is_plain_and_bidirectional() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("output.md");
        assert!(write_output(path.clone(), "one").unwrap());
        assert!(!write_output(path.clone(), "one").unwrap());
        assert_eq!(fs::read(path).unwrap(), b"one");
    }

    #[test]
    fn criterion_19_accumulator_stamps_preserves_and_drops_malformed() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("chronicle/20260101/talents/steward.jsonl");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "bad\n{\"kept\":true}\n").unwrap();
        let mut record =
            Map::from_iter([("ts".to_owned(), json!(7)), ("new".to_owned(), json!(true))]);
        let result = append_day_record(root.path(), "20260101", "steward", &mut record);
        // The index database is intentionally absent in this fixture; index failure is a warning.
        assert!(result.is_ok());
        let rows: Vec<Value> = read_jsonl(path, Vec::new(), MalformedPolicy::Skip).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1]["ts"], 7);
    }

    #[test]
    fn criterion_19_accumulator_is_atomic_unlocked_and_declines_without_warning() {
        let root = tempfile::tempdir().unwrap();
        let path = root
            .path()
            .join("chronicle/20260101/talents/unrecognized.jsonl");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "{\"kept\":true}\n").unwrap();
        let before_inode = fs::metadata(&path).unwrap().ino();
        let locked = std::fs::OpenOptions::new().read(true).open(&path).unwrap();
        let _lock = Flock::lock(locked, FlockArg::LockExclusiveNonblock).unwrap();
        reset_index_warnings();
        let mut record = Map::from_iter([("new".to_owned(), json!(true))]);
        append_day_record(root.path(), "20260101", "unrecognized", &mut record).unwrap();
        assert_ne!(fs::metadata(&path).unwrap().ino(), before_inode);
        let declined = root.path().join("health/not-indexed.jsonl");
        fs::create_dir_all(declined.parent().unwrap()).unwrap();
        fs::write(&declined, "{}\n").unwrap();
        assert!(matches!(
            rescan_file(root.path(), &declined).unwrap(),
            RescanFileStatus::Declined
        ));
        assert_eq!(index_warnings(), 0);
    }

    #[test]
    fn criterion_19_index_failure_warns_once_without_failing_the_write() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("indexer"), b"not a directory").unwrap();
        reset_index_warnings();
        let mut record = Map::from_iter([("event".to_owned(), json!("summary"))]);
        assert!(append_day_record(root.path(), "20260101", "steward", &mut record).is_ok());
        assert_eq!(index_warnings(), 1);
    }
}
