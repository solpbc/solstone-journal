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
    DailySchedule {
        output: String,
        output_path: Option<String>,
    },
    Participation {
        output: String,
        facet: String,
        day: String,
        activity: Map<String, Value>,
    },
    Schedule {
        output: String,
        day: String,
    },
    FacetNewsletter {
        output: String,
        facet: String,
        day: String,
    },
    EntityDetection {
        output: String,
        day: String,
        segment: String,
        stream: Option<String>,
    },
    EntitiesReview {
        output: String,
        facet: String,
        day: String,
    },
    EntityObserver {
        output: String,
        facet: String,
        day: String,
    },
    TimelineSegmentSummary {
        result: Value,
        binding: solstone_core_timeline::SegmentBindingV1,
        source: Box<solstone_core_timeline::SegmentSourceV1>,
        input_digest: String,
        provenance: Box<solstone_core_timeline::GenerationProvenanceV1>,
    },
    SpeakerAttribution {
        output: String,
        day: String,
        segment: String,
        stream: String,
        state: crate::speaker_attribution::SpeakerAttributionState,
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
                    talent,
                    detail,
                })?;
            Ok(CommitDisposition::CommittedNoOutput)
        }
        CommitPlan::Write(WriteIntent::DailySchedule {
            output,
            output_path,
        }) => {
            crate::daily_schedule::apply_result(&context.journal, &output).map_err(|detail| {
                StageError {
                    phase: "write-intent",
                    stage: "daily_schedule",
                    talent: "daily_schedule".to_owned(),
                    detail,
                }
            })?;
            let Some(output_path) = output_path else {
                return Ok(CommitDisposition::CommittedNoOutput);
            };
            write_output(PathBuf::from(output_path), &output).map_err(|error| StageError {
                phase: "write-intent",
                stage: "daily_schedule",
                talent: "daily_schedule".to_owned(),
                detail: error.to_string(),
            })?;
            Ok(CommitDisposition::Written)
        }
        CommitPlan::Write(WriteIntent::Participation {
            output,
            facet,
            day,
            activity,
        }) => {
            crate::participation::apply_result(&context.journal, &output, &facet, &day, &activity)
                .map_err(|detail| StageError {
                    phase: "write-intent",
                    stage: "participation",
                    talent: "participation".to_owned(),
                    detail,
                })?;
            Ok(CommitDisposition::CommittedNoOutput)
        }
        CommitPlan::Write(WriteIntent::Schedule { output, day }) => {
            crate::schedule::apply_result(&context.journal, &output, &day).map_err(|detail| {
                StageError {
                    phase: "write-intent",
                    stage: "schedule",
                    talent: "schedule".to_owned(),
                    detail,
                }
            })?;
            Ok(CommitDisposition::CommittedNoOutput)
        }
        CommitPlan::Write(WriteIntent::FacetNewsletter { output, facet, day }) => {
            crate::facet_newsletter::apply_result(&context.journal, &output, &facet, &day)
                .map_err(|detail| StageError {
                    phase: "write-intent",
                    stage: "facet_newsletter",
                    talent: "facet_newsletter".to_owned(),
                    detail,
                })?;
            Ok(CommitDisposition::CommittedNoOutput)
        }
        CommitPlan::Write(WriteIntent::EntityDetection {
            output,
            day,
            segment,
            stream,
        }) => {
            crate::entities::detection::apply_result(
                &context.journal,
                &output,
                &day,
                &segment,
                stream.as_deref(),
            )
            .map_err(|detail| StageError {
                phase: "write-intent",
                stage: "entities:detection",
                talent: "entities:detection".to_owned(),
                detail,
            })?;
            Ok(CommitDisposition::CommittedNoOutput)
        }
        CommitPlan::Write(WriteIntent::EntitiesReview { output, facet, day }) => {
            crate::entities::review::apply_result(&context.journal, &output, &facet, &day)
                .map_err(|detail| StageError {
                    phase: "write-intent",
                    stage: "entities:entities_review",
                    talent: "entities:entities_review".to_owned(),
                    detail,
                })?;
            Ok(CommitDisposition::CommittedNoOutput)
        }
        CommitPlan::Write(WriteIntent::EntityObserver { output, facet, day }) => {
            crate::entities::observer::apply_result(&context.journal, &output, &facet, &day)
                .map_err(|detail| StageError {
                    phase: "write-intent",
                    stage: "entities:entity_observer",
                    talent: "entities:entity_observer".to_owned(),
                    detail,
                })?;
            Ok(CommitDisposition::CommittedNoOutput)
        }
        CommitPlan::Write(WriteIntent::TimelineSegmentSummary {
            result,
            binding,
            source,
            input_digest,
            provenance,
        }) => {
            crate::timeline::apply_result(
                &context.journal,
                &result,
                binding,
                *source,
                input_digest,
                *provenance,
            )
            .map_err(|detail| StageError {
                phase: "write-intent",
                stage: "timeline:segment_summary",
                talent: "timeline:segment_summary".to_owned(),
                detail,
            })?;
            Ok(CommitDisposition::CommittedNoOutput)
        }
        CommitPlan::Write(WriteIntent::SpeakerAttribution {
            output,
            day,
            segment,
            stream,
            state,
        }) => {
            crate::speaker_attribution::apply_result(
                &context.journal,
                &output,
                &day,
                &segment,
                &stream,
                &state,
            )
            .map_err(|detail| StageError {
                phase: "write-intent",
                stage: "speaker_attribution",
                talent: "speaker_attribution".to_owned(),
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
    use std::io::Cursor;
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

    fn worker_events(bytes: &[u8]) -> Vec<Value> {
        std::str::from_utf8(bytes)
            .unwrap()
            .lines()
            .map(serde_json::from_str)
            .collect::<Result<_, _>>()
            .unwrap()
    }

    fn steward_worker_fixture(
        root: &tempfile::TempDir,
    ) -> (crate::prepare::RuntimePaths, ExecutionContext) {
        let talent_root = root.path().join("talent");
        let apps_root = root.path().join("apps");
        let templates_dir = root.path().join("templates");
        let journal = root.path().join("journal");
        fs::create_dir_all(&talent_root).unwrap();
        fs::create_dir_all(&apps_root).unwrap();
        fs::create_dir_all(&templates_dir).unwrap();
        fs::create_dir_all(journal.join("config")).unwrap();
        fs::write(
            talent_root.join("steward.md"),
            "{\n\"type\":\"generate\", \"hook\":{\"pre\":\"steward\",\"post\":\"steward\"}, \"load\":{\"transcripts\":false}\n}\nfixture",
        )
        .unwrap();
        fs::write(
            journal.join("config/journal.json"),
            r#"{"providers":{"active":{"provider":"test","model":"test-model"}}}"#,
        )
        .unwrap();
        (
            crate::prepare::RuntimePaths {
                talent_root,
                apps_root,
                templates_dir,
            },
            ExecutionContext { journal },
        )
    }

    #[test]
    fn criterion_6_output_guard_is_plain_and_bidirectional() {
        let root = tempfile::Builder::new()
            .prefix("solstone-talent-output-guard-")
            .tempdir_in("/var/tmp")
            .unwrap();
        let path = root.path().join("output.md");
        assert!(write_output(path.clone(), "one").unwrap());
        assert!(!write_output(path.clone(), "one").unwrap());
        assert_eq!(fs::read(path).unwrap(), b"one");
    }

    #[test]
    fn daily_schedule_writes_day_output_and_only_primary_metadata() {
        let root = tempfile::Builder::new()
            .prefix("solstone-daily-schedule-output-")
            .tempdir_in("/var/tmp")
            .unwrap();
        let output_path = root
            .path()
            .join("journal/chronicle/20990101/talents/daily_schedule.json");
        let output = r#"{"primary":"03:00","fallback":"04:00"}"#;
        let disposition = apply(
            CommitPlan::Write(WriteIntent::DailySchedule {
                output: output.to_owned(),
                output_path: Some(output_path.to_string_lossy().into_owned()),
            }),
            &ExecutionContext {
                journal: root.path().join("journal"),
            },
        )
        .unwrap();

        assert_eq!(disposition, CommitDisposition::Written);
        assert_eq!(fs::read_to_string(output_path).unwrap(), output);
        let schedules: Value = serde_json::from_slice(
            &fs::read(root.path().join("journal/config/schedules.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(schedules["daily_time"], "03:00");
        assert!(schedules.get("fallback").is_none());
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
            .join("chronicle/20260101/talents/nested/unrecognized.jsonl");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "{\"kept\":true}\n").unwrap();
        let before_inode = fs::metadata(&path).unwrap().ino();
        let locked = std::fs::OpenOptions::new().read(true).open(&path).unwrap();
        let _lock = Flock::lock(locked, FlockArg::LockExclusiveNonblock).unwrap();
        // A classified path would fail to open this as the index directory.
        // Reaching no warning therefore proves append_day_record saw Declined.
        fs::write(root.path().join("indexer"), b"not a directory").unwrap();
        reset_index_warnings();
        let mut record = Map::from_iter([("new".to_owned(), json!(true))]);
        append_day_record(root.path(), "20260101", "nested/unrecognized", &mut record).unwrap();
        assert_ne!(fs::metadata(&path).unwrap().ino(), before_inode);
        assert_eq!(index_warnings(), 0);
    }

    #[test]
    fn criterion_19_index_failure_warns_once_and_worker_finishes() {
        let root = tempfile::tempdir().unwrap();
        let (paths, context) = steward_worker_fixture(&root);
        fs::write(context.journal.join("indexer"), b"not a directory").unwrap();
        let client =
            solstone_core_generate::OneShotClient::at_path(crate::test_support::one_shot_stub(
                root.path(),
                r#"{"headline":"All clear","summary_sentence":"fine","suggested_action":"none"}"#,
            ));
        reset_index_warnings();
        let mut output = Vec::new();
        crate::run_lines(
            Cursor::new("{\"name\":\"steward\",\"day\":\"20260101\",\"prompt\":\"hello\"}\n"),
            &mut output,
            &paths,
            &context,
            Ok(&client),
            Ok(
                &solstone_core_cogitate_wire::CogitateOneShotClient::at_path(
                    root.path().join("unused-cogitate"),
                ),
            ),
        );
        let events = worker_events(&output);
        assert!(events.iter().any(|event| event["event"] == "finish"));
        assert!(!events.iter().any(|event| event["event"] == "error"));
        assert_eq!(index_warnings(), 1);
    }

    #[test]
    fn story_commit_error_names_the_talent() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("facets"), b"not a directory").unwrap();
        let error = apply(
            CommitPlan::Write(WriteIntent::Story {
                talent: "conversation".to_owned(),
                facet: "work".to_owned(),
                day: "20260101".to_owned(),
                record_id: "activity-1".to_owned(),
                value: json!({
                    "body":"body", "topics":[], "confidence":1,
                    "commitments":[], "closures":[], "decisions":[], "relations":[]
                }),
            }),
            &ExecutionContext {
                journal: root.path().to_owned(),
            },
        )
        .unwrap_err();
        assert_eq!(error.phase, "commit");
        assert_eq!(error.stage, "story");
        assert_eq!(error.talent, "conversation");
    }
}
