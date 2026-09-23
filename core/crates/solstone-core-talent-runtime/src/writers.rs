// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

mod daily;
pub use daily::{
    PreparedDailyAction, PreparedDailyPublication, prepare_daily_output, prepare_daily_publication,
    publish_daily_publication, required_artifact_receipts,
};
pub(crate) use daily::{bind_output_action, prepare_frozen_output_action};

use std::fs;
use std::path::PathBuf;

use serde_json::{Map, Value};
use solstone_core_indexer_store::scan::{RescanFileStatus, rescan_file};
use solstone_core_journal_io::{AtomicWriteError, AtomicWriteOptions, atomic_replace, write_jsonl};

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
        destination_id: String,
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
        destination_id: String,
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
    EntitySuggest {
        output: String,
        facet: String,
        day: String,
    },
    EntityObserver {
        output: String,
        facet: String,
        day: String,
        served_ids: std::collections::BTreeSet<String>,
        shown_observation_ids: std::collections::BTreeMap<String, std::collections::BTreeSet<u64>>,
    },
    SpeakerAttribution {
        output: String,
        day: String,
        stream_layout: solstone_core_journal_io::SegmentLayout,
        segment: String,
        stream: String,
        state: crate::speaker_attribution::SpeakerAttributionState,
    },
}

pub fn write_output_if_configured(
    prepared: &PreparedTalent,
    context: &ExecutionContext,
    output: &str,
) -> Result<bool, String> {
    let Some(path_str) = prepared.config.get("output_path").and_then(Value::as_str) else {
        return Ok(false);
    };
    let path = PathBuf::from(path_str);
    let activity_owned = prepared.config.get("activity").is_some()
        || prepared.config.get("schedule").and_then(Value::as_str) == Some("activity")
        || prepared.config.get("destination_id").is_some();
    if activity_owned {
        let facet = prepared
            .config
            .get("facet")
            .and_then(Value::as_str)
            .ok_or("activity output is missing its destination facet")?;
        let expected = prepared
            .config
            .get("destination_id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or("activity output is missing its destination identity")?;
        let _guard =
            solstone_core_facets::hold_activity_enrichment(&context.journal, facet, expected)
                .map_err(|error| error.to_string())?;
        let directory = solstone_core_journal_io::contained_path(
            &context.journal,
            &format!("facets/{facet}/activities"),
        )
        .map_err(|error| error.to_string())?;
        let contained = solstone_core_journal_io::realpath_non_strict(&path)
            .map_err(|error| error.to_string())?;
        if !contained.starts_with(&directory) {
            return Err("activity output is outside its destination facet".to_owned());
        }
        return write_output(contained, output).map_err(|error| error.to_string());
    }
    write_output(path, output).map_err(|error| error.to_string())
}

pub fn write_output(path: PathBuf, output: &str) -> Result<bool, std::io::Error> {
    let bytes = output.as_bytes();
    if path.exists() && fs::read(&path)? == bytes {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    match atomic_replace(&path, bytes, AtomicWriteOptions::default()) {
        Ok(()) => Ok(true),
        #[cfg(windows)]
        Err(error @ AtomicWriteError::PublicationUncertain { .. }) => {
            Err(std::io::Error::other(error))
        }
        Err(AtomicWriteError::Io { source, .. }) => Err(source),
    }
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
            destination_id,
            talent,
            facet,
            day,
            record_id,
            value,
        }) => {
            crate::story::apply_story(
                &context.journal,
                &talent,
                &facet,
                &destination_id,
                &day,
                &record_id,
                &value,
            )
            .map_err(|detail| StageError::new("commit", "story", talent, detail))?;
            Ok(CommitDisposition::CommittedNoOutput)
        }
        CommitPlan::Write(WriteIntent::DailySchedule {
            output,
            output_path,
        }) => {
            crate::daily_schedule::apply_result(&context.journal, &output).map_err(|detail| {
                StageError::new("write-intent", "daily_schedule", "daily_schedule", detail)
            })?;
            let Some(output_path) = output_path else {
                return Ok(CommitDisposition::CommittedNoOutput);
            };
            write_output(PathBuf::from(output_path), &output).map_err(|error| {
                StageError::new(
                    "write-intent",
                    "daily_schedule",
                    "daily_schedule",
                    error.to_string(),
                )
            })?;
            Ok(CommitDisposition::Written)
        }
        CommitPlan::Write(WriteIntent::Participation {
            destination_id,
            output,
            facet,
            day,
            activity,
        }) => {
            crate::participation::apply_result(
                &context.journal,
                &output,
                &facet,
                &destination_id,
                &day,
                &activity,
            )
            .map_err(|detail| {
                StageError::new("write-intent", "participation", "participation", detail)
            })?;
            Ok(CommitDisposition::CommittedNoOutput)
        }
        CommitPlan::Write(WriteIntent::Schedule { output, day }) => {
            crate::schedule::apply_result(&context.journal, &output, &day).map_err(|detail| {
                StageError::new("write-intent", "schedule", "schedule", detail)
            })?;
            Ok(CommitDisposition::CommittedNoOutput)
        }
        CommitPlan::Write(WriteIntent::FacetNewsletter { output, facet, day }) => {
            crate::facet_newsletter::apply_result(&context.journal, &output, &facet, &day)
                .map_err(|detail| {
                    StageError::new(
                        "write-intent",
                        "facet_newsletter",
                        "facet_newsletter",
                        detail,
                    )
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
            .map_err(|detail| {
                StageError::new(
                    "write-intent",
                    "entities:detection",
                    "entities:detection",
                    detail,
                )
            })?;
            Ok(CommitDisposition::CommittedNoOutput)
        }
        CommitPlan::Write(WriteIntent::EntitiesReview { output, facet, day }) => {
            crate::entities::review::apply_result(&context.journal, &output, &facet, &day)
                .map_err(|detail| {
                    StageError::new(
                        "write-intent",
                        "entities:entities_review",
                        "entities:entities_review",
                        detail,
                    )
                })?;
            Ok(CommitDisposition::CommittedNoOutput)
        }
        CommitPlan::Write(WriteIntent::EntitySuggest { output, facet, day }) => {
            let path = context
                .journal
                .join("facets")
                .join(&facet)
                .join("entities")
                .join(format!("{day}_observer_suggestions.json"));
            write_output(path, &format!("{output}\n")).map_err(|e| {
                StageError::new(
                    "write-intent",
                    "entities:entity_suggest",
                    "entities:entity_suggest",
                    e.to_string(),
                )
            })?;
            Ok(CommitDisposition::CommittedNoOutput)
        }
        CommitPlan::Write(WriteIntent::EntityObserver {
            output,
            facet,
            day,
            served_ids,
            shown_observation_ids,
        }) => {
            crate::entities::observer::apply_result(
                &context.journal,
                &output,
                &facet,
                &day,
                &served_ids,
                &shown_observation_ids,
            )
            .map_err(|detail| {
                StageError::new(
                    "write-intent",
                    "entities:entity_observer",
                    "entities:entity_observer",
                    detail,
                )
            })?;
            Ok(CommitDisposition::CommittedNoOutput)
        }
        CommitPlan::Write(WriteIntent::SpeakerAttribution {
            output,
            day,
            stream_layout,
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
                stream_layout,
                &state,
            )
            .map_err(|detail| {
                StageError::new(
                    "write-intent",
                    "speaker_attribution",
                    "speaker_attribution",
                    detail,
                )
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
    let mut records: Vec<Value> =
        solstone_core_journal_io::durability::read_jsonl_durable::<Value>(
            solstone_core_journal_io::durability::ArtifactId::TalentDayAccumulator,
            &path,
        )
        .map_err(|error| stage_error(agent, error.to_string()))?
        .records;
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
    StageError::new("write-intent", "day-accumulator", stage, detail)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Cursor;
    use std::os::unix::fs::MetadataExt;

    use nix::fcntl::{Flock, FlockArg};
    use serde_json::json;
    use solstone_core_journal_io::{MalformedPolicy, read_jsonl};

    use super::*;

    #[test]
    fn activity_commit_intents_fence_replacement_and_deletion_before_side_effects() {
        use crate::contract::{ParsedOutput, PrePostState};
        for hook in ["story", "participation"] {
            for transition in ["unchanged", "replaced", "deleted", "muted", "missing_row"] {
                let root = tempfile::tempdir().unwrap();
                let context = ExecutionContext {
                    journal: root.path().to_path_buf(),
                };
                solstone_core_facets::create_facet(root.path(), "work", "Work", "", "", "", None)
                    .unwrap();
                let id = solstone_core_facets::observe_facet_write_identity(root.path(), "work")
                    .unwrap();
                let row = json!({"id":"activity-1","activity":"work","segments":[]});
                let row_path = root.path().join("facets/work/activities/20260101.jsonl");
                fs::create_dir_all(row_path.parent().unwrap()).unwrap();
                // Historical rows without destination_id must still be fenced by preparation.
                fs::write(&row_path, format!("{row}\n")).unwrap();
                let prepared = PreparedTalent {
                    name: hook.to_owned(),
                    config: json!({
                        "facet":"work", "day":"20260101", "activity":row, "destination_id":id
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                };
                let state = PrePostState::None;
                let plan = if hook == "story" {
                    crate::story::commit(
                        ParsedOutput::Json(json!({"body":"updated", "topics":[], "confidence":1,
                        "commitments":[], "closures":[], "decisions":[], "relations":[]})),
                        &prepared,
                        &state,
                    )
                    .unwrap()
                } else {
                    crate::participation::commit(
                        ParsedOutput::Text(
                            json!({"participation":[{"name":"New Name","role":"mentioned"}]})
                                .to_string(),
                        ),
                        &prepared,
                        &state,
                    )
                    .unwrap()
                };
                match transition {
                    "replaced" => {
                        // A journal keeps one enabled facet; the sibling lets "work" go.
                        let _ = solstone_core_facets::create_facet(
                            root.path(),
                            "personal",
                            "Personal",
                            "",
                            "",
                            "",
                            None,
                        );
                        solstone_core_facets::delete_facet(root.path(), "work").unwrap();
                        solstone_core_facets::create_facet(
                            root.path(),
                            "work",
                            "Replacement",
                            "",
                            "",
                            "",
                            None,
                        )
                        .unwrap();
                        fs::create_dir_all(row_path.parent().unwrap()).unwrap();
                        fs::write(&row_path, format!("{row}\n")).unwrap();
                    }
                    "deleted" => {
                        // A journal keeps one enabled facet; the sibling lets "work" go.
                        let _ = solstone_core_facets::create_facet(
                            root.path(),
                            "personal",
                            "Personal",
                            "",
                            "",
                            "",
                            None,
                        );
                        solstone_core_facets::delete_facet(root.path(), "work").unwrap();
                    }
                    "muted" => {
                        // A journal keeps one enabled facet; the sibling lets "work" go.
                        let _ = solstone_core_facets::create_facet(
                            root.path(),
                            "personal",
                            "Personal",
                            "",
                            "",
                            "",
                            None,
                        );
                        solstone_core_facets::set_facet_muted(root.path(), "work", true).unwrap();
                    }
                    "missing_row" => {
                        fs::remove_file(&row_path).unwrap();
                    }
                    _ => {}
                }
                let before = fs::read(&row_path).ok();
                let result = apply(plan, &context);
                if transition == "unchanged" {
                    assert!(result.is_ok(), "{hook}: {result:?}");
                    assert_ne!(fs::read(&row_path).ok(), before);
                } else {
                    assert!(result.is_err(), "{hook} {transition}");
                    assert_eq!(fs::read(&row_path).ok(), before);
                    assert!(!root.path().join("entities").exists());
                    if transition == "deleted" {
                        assert!(!root.path().join("facets/work").exists());
                    }
                }
            }
        }
    }

    #[test]
    fn activity_raw_output_uses_context_root_and_holds_destination_lock() {
        let temp = tempfile::tempdir().unwrap();
        let journal = temp.path().join("facets/journal");
        solstone_core_facets::create_facet(&journal, "work", "Work", "", "", "", None).unwrap();
        let id = solstone_core_facets::observe_facet_write_identity(&journal, "work").unwrap();
        let path = journal.join("facets/work/activities/out.json");
        let mut prepared = PreparedTalent {
            name: "test".to_owned(),
            config: json!({
                "facet":"work", "activity":{"id":"a"}, "destination_id":id, "output_path":path
            })
            .as_object()
            .unwrap()
            .clone(),
        };
        let context = ExecutionContext {
            journal: journal.clone(),
        };
        assert!(write_output_if_configured(&prepared, &context, "first").unwrap());
        prepared.config.remove("destination_id");
        assert!(write_output_if_configured(&prepared, &context, "missing binding").is_err());
        prepared
            .config
            .insert("destination_id".to_owned(), json!(id));
        prepared.config.insert(
            "output_path".to_owned(),
            json!(journal.join("outside.json")),
        );
        assert!(write_output_if_configured(&prepared, &context, "wrong path").is_err());
        prepared
            .config
            .insert("output_path".to_owned(), json!(path));
        let link = temp.path().join("journal-link");
        std::os::unix::fs::symlink(&journal, &link).unwrap();
        prepared.config.insert(
            "output_path".to_owned(),
            json!(link.join("facets/work/activities/20260101/a/out.json")),
        );
        assert!(
            write_output_if_configured(
                &prepared,
                &ExecutionContext { journal: link },
                "through link"
            )
            .unwrap()
        );
        assert!(
            journal
                .join("facets/work/activities/20260101/a/out.json")
                .exists()
        );
        prepared
            .config
            .insert("output_path".to_owned(), json!(path));
        let guard = solstone_core_facets::hold_activity_enrichment(&journal, "work", &id).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            tx.send(()).unwrap();
            write_output_if_configured(&prepared, &context, "stale")
        });
        rx.recv().unwrap();
        // A journal keeps one enabled facet; the sibling lets "work" go.
        let _ =
            solstone_core_facets::create_facet(&journal, "personal", "Personal", "", "", "", None);
        solstone_core_facets::delete_facet(&journal, "work").unwrap();
        drop(guard);
        assert!(worker.join().unwrap().is_err());
        assert!(!journal.join("facets/work").exists());
    }

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
    fn criterion_6_output_guard_is_atomic_and_bidirectional() {
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
                destination_id: "00000000-0000-4000-8000-000000000001".to_owned(),
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
