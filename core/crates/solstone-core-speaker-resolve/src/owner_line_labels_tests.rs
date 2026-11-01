// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use solstone_core_entity::{EncoderIdentity, EntityMergeOptions, VoiceprintItem};
use solstone_core_speaker_id::owner_line_labels::{
    OwnerLineLabels, owner_line_labels_path, read_owner_line_labels,
};

use super::*;

const DAY: &str = "20260808";
const STREAM: &str = "mic";
const HEX: &str = "0123456789abcdef";

fn journal() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    entity(root.path(), "owner", json!({"is_principal": true}));
    entity(root.path(), "ada", json!({}));
    entity(root.path(), "bo", json!({}));
    // Any journal that has used its people has the journal-wide trust lock.
    drop(hold_entity_trust_lock(root.path()).unwrap());
    root
}

fn entity(root: &Path, id: &str, extra: Value) {
    let mut value = json!({"id": id, "name": id, "type": "Person"});
    for (key, field) in extra.as_object().unwrap() {
        value[key] = field.clone();
    }
    let directory = root.join("entities").join(id);
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join("entity.json"), value.to_string()).unwrap();
}

fn segment_dir(root: &Path, key: &str) -> PathBuf {
    root.join("chronicle").join(DAY).join(STREAM).join(key)
}

/// A segment whose `audio` transcript holds `count` lines, each stored with
/// its statement ordinal as its `sentence_id`, after a header row.
fn segment(root: &Path, key: &str, count: i64) -> PathBuf {
    let mut rows = vec![json!({"raw": "audio.flac"})];
    rows.extend((1..=count).map(|id| {
        json!({"start": format!("00:00:{id:02}"), "text": format!("line {id}"), "sentence_id": id})
    }));
    segment_with(root, key, &[("audio.jsonl", &jsonl(&rows))])
}

fn segment_with(root: &Path, key: &str, files: &[(&str, &str)]) -> PathBuf {
    let directory = segment_dir(root, key);
    fs::create_dir_all(&directory).unwrap();
    for (name, contents) in files {
        fs::write(directory.join(name), contents).unwrap();
    }
    directory
}

fn jsonl(rows: &[Value]) -> String {
    rows.iter().map(|row| format!("{row}\n")).collect()
}

fn line(segment: &Path, sentence_id: i64) -> OwnerLine {
    OwnerLine {
        segment_dir: segment.to_path_buf(),
        source: "audio".to_owned(),
        sentence_id,
    }
}

fn person(id: &str) -> LineSpeaker {
    LineSpeaker::Person(id.to_owned())
}

fn entries(segment: &Path) -> Vec<OwnerLineEntry> {
    match read_owner_line_labels(segment) {
        OwnerLineLabels::Present(entries) => entries,
        other => panic!("expected entries, found {other:?}"),
    }
}

/// Every path under `root` with its bytes (`None` for a directory).
fn tree(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    fn walk(root: &Path, directory: &Path, out: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
        for entry in fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            let relative = path.strip_prefix(root).unwrap().to_path_buf();
            if path.is_dir() {
                out.insert(relative, None);
                walk(root, &path, out);
            } else {
                out.insert(relative, Some(fs::read(&path).unwrap()));
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

fn changed_paths(
    before: &BTreeMap<PathBuf, Option<Vec<u8>>>,
    after: &BTreeMap<PathBuf, Option<Vec<u8>>>,
) -> BTreeSet<PathBuf> {
    before
        .keys()
        .chain(after.keys())
        .filter(|path| before.get(*path) != after.get(*path))
        .cloned()
        .collect()
}

fn encoder() -> EncoderIdentity {
    EncoderIdentity {
        id: "test".into(),
        sha256: "a".repeat(64),
        width: 256,
    }
}

fn voice(axis: usize) -> Vec<f32> {
    let mut values = vec![0.0_f32; 256];
    values[axis] = 1.0;
    values
}

fn merge(root: &Path, source: &str, target: &str) {
    solstone_core_entity::commit_entity_merge(
        root,
        source,
        target,
        EntityMergeOptions::default(),
        &encoder(),
    )
    .unwrap();
}

#[test]
fn entries_round_trip_and_removing_an_absent_line_changes_nothing() {
    let root = journal();
    let segment = segment(root.path(), "120000_300", 3);
    set_owner_line_labels(root.path(), &[line(&segment, 1)], &person("ada")).unwrap();
    set_owner_line_labels(root.path(), &[line(&segment, 2)], &LineSpeaker::Unknown).unwrap();
    set_owner_line_labels(root.path(), &[line(&segment, 3)], &LineSpeaker::Owner).unwrap();

    let stored = entries(&segment);
    let speakers = stored
        .iter()
        .map(|entry| (entry.sentence_id, entry.speaker.clone()))
        .collect::<Vec<_>>();
    assert_eq!(
        speakers,
        vec![
            (1, Some("ada".to_owned())),
            (2, None),
            (3, Some("owner".to_owned()))
        ]
    );
    assert_eq!(stored[1].fingerprint.start, json!("00:00:02"));

    let path = owner_line_labels_path(&segment);
    let before = fs::read(&path).unwrap();
    assert_eq!(
        remove_owner_line_labels(root.path(), &[line(&segment, 9)]).unwrap(),
        0
    );
    assert_eq!(fs::read(&path).unwrap(), before);

    let all = [line(&segment, 1), line(&segment, 2), line(&segment, 3)];
    assert_eq!(remove_owner_line_labels(root.path(), &all).unwrap(), 3);
    assert_eq!(entries(&segment), Vec::new());
}

#[test]
fn removing_where_nothing_was_set_creates_nothing() {
    let root = journal();
    let segment = segment(root.path(), "120000_300", 2);
    let before = tree(root.path());
    assert_eq!(
        remove_owner_line_labels(root.path(), &[line(&segment, 1)]).unwrap(),
        0
    );
    fs::create_dir(segment.join("talents")).unwrap();
    let with_talents = tree(root.path());
    assert_eq!(
        remove_owner_line_labels(root.path(), &[line(&segment, 1)]).unwrap(),
        0
    );
    assert_eq!(tree(root.path()), with_talents);
    assert_eq!(changed_paths(&before, &with_talents).len(), 1);
}

#[test]
fn a_never_attributed_segment_takes_answers_without_gaining_speaker_labels() {
    let root = journal();
    let segment = segment(root.path(), "120000_300", 2);
    set_owner_line_labels(root.path(), &[line(&segment, 2)], &person("bo")).unwrap();
    assert_eq!(entries(&segment)[0].speaker.as_deref(), Some("bo"));
    assert!(!segment.join("talents/speaker_labels.json").exists());
    assert!(!segment.join("talents/speaker_corrections.json").exists());
}

/// Run `set` against a fixture and require the given refusal with every byte
/// of the journal unchanged.
fn assert_set_refused(
    root: &Path,
    lines: &[OwnerLine],
    speaker: &LineSpeaker,
    expected: impl Fn(&OwnerLineLabelError) -> bool,
) {
    let before = tree(root);
    let error = set_owner_line_labels(root, lines, speaker).unwrap_err();
    assert!(expected(&error), "{error:?}");
    assert_eq!(changed_paths(&before, &tree(root)), BTreeSet::new());
}

#[test]
fn segments_whose_transcript_views_disagree_on_the_source_are_refused() {
    let root = journal();
    let rows = jsonl(&[json!({"start": "00:00:01", "text": "hi", "sentence_id": 1})]);
    let part = format!("p0001_{HEX}");
    let disagree = segment_with(
        root.path(),
        "120000_300",
        &[
            ("audio.jsonl", &rows),
            (&format!("{part}.jsonl"), &rows),
            (&format!("{part}.npz"), "npz"),
        ],
    );
    assert_set_refused(
        root.path(),
        &[line(&disagree, 1)],
        &LineSpeaker::Unknown,
        |error| {
            matches!(error, OwnerLineLabelError::SourceUnsupported { labelled, transcript_page }
                if labelled.as_deref() == Some("audio") && transcript_page.as_deref() == Some(part.as_str()))
        },
    );

    let ambiguous = segment_with(
        root.path(),
        "121000_300",
        &[("audio.jsonl", &rows), ("mic_audio.jsonl", &rows)],
    );
    assert_set_refused(
        root.path(),
        &[line(&ambiguous, 1)],
        &LineSpeaker::Unknown,
        |error| {
            *error
                == OwnerLineLabelError::SourceUnsupported {
                    labelled: None,
                    transcript_page: None,
                }
        },
    );

    let agree = segment_with(
        root.path(),
        "122000_300",
        &[
            ("audio.jsonl", &rows),
            ("audio.npz", "npz"),
            (&format!("{part}.jsonl"), &rows),
        ],
    );
    set_owner_line_labels(root.path(), &[line(&agree, 1)], &person("ada")).unwrap();
    assert_eq!(entries(&agree).len(), 1);

    let wrong = OwnerLine {
        source: "mic_audio".to_owned(),
        ..line(&agree, 1)
    };
    assert_set_refused(root.path(), &[wrong], &LineSpeaker::Unknown, |error| {
        matches!(error, OwnerLineLabelError::WrongSource { .. })
    });
}

#[test]
fn bad_lines_and_bad_people_are_refused_with_their_own_errors() {
    let root = journal();
    let segment = segment(root.path(), "120000_300", 2);
    let one = [line(&segment, 1)];
    assert_set_refused(root.path(), &[line(&segment, 3)], &person("ada"), |error| {
        matches!(
            error,
            OwnerLineLabelError::LineNotFound { sentence_id: 3, .. }
        )
    });
    assert_set_refused(root.path(), &[], &person("ada"), |error| {
        *error == OwnerLineLabelError::NoLines
    });
    assert_set_refused(root.path(), &one, &person("nobody"), |error| {
        *error == OwnerLineLabelError::PersonNotFound("nobody".into())
    });
    entity(root.path(), "hidden", json!({"blocked": true}));
    assert_set_refused(root.path(), &one, &person("hidden"), |error| {
        *error == OwnerLineLabelError::PersonHidden("hidden".into())
    });
    entity(root.path(), "acme", json!({"type": "Company"}));
    assert_set_refused(root.path(), &one, &person("acme"), |error| {
        *error == OwnerLineLabelError::NotAPerson("acme".into())
    });
    entity(root.path(), "speaker_2", json!({"name": "Speaker 2"}));
    assert_set_refused(root.path(), &one, &person("speaker_2"), |error| {
        *error == OwnerLineLabelError::NotAPerson("speaker_2".into())
    });
    merge(root.path(), "bo", "ada");
    assert_set_refused(root.path(), &one, &person("bo"), |error| {
        *error
            == OwnerLineLabelError::PersonMerged {
                id: "bo".into(),
                successor: "ada".into(),
            }
    });
    fs::write(root.path().join("entities/ada/entity.json"), b"{").unwrap();
    assert_set_refused(root.path(), &one, &person("ada"), |error| {
        matches!(error, OwnerLineLabelError::EntityStoreUnreadable(_))
    });
}

#[test]
fn an_unreadable_entity_store_is_its_own_refusal() {
    let root = journal();
    let segment = segment(root.path(), "120000_300", 1);
    fs::remove_dir_all(root.path().join("entities")).unwrap();
    fs::write(root.path().join("entities"), b"not a directory").unwrap();
    for speaker in [person("ada"), LineSpeaker::Owner] {
        assert_set_refused(root.path(), &[line(&segment, 1)], &speaker, |error| {
            matches!(error, OwnerLineLabelError::EntityStoreUnreadable(_))
        });
    }
}

#[test]
fn the_owner_is_refused_until_the_owner_identity_is_admitted() {
    let root = journal();
    let segment = segment(root.path(), "120000_300", 1);
    entity(root.path(), "second", json!({"is_principal": true}));
    for speaker in [LineSpeaker::Owner, person("owner")] {
        assert_set_refused(root.path(), &[line(&segment, 1)], &speaker, |error| {
            *error == OwnerLineLabelError::OwnerNotAdmitted
        });
    }
    entity(root.path(), "second", json!({}));
    set_owner_line_labels(root.path(), &[line(&segment, 1)], &person("owner")).unwrap();
    assert_eq!(entries(&segment)[0].speaker.as_deref(), Some("owner"));
}

#[test]
fn a_damaged_owner_file_reads_as_unreadable_and_is_never_healed() {
    let root = journal();
    let segment = segment(root.path(), "120000_300", 1);
    let path = owner_line_labels_path(&segment);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, b"{\"version\":1,\"entries\":[").unwrap();
    assert!(matches!(
        read_owner_line_labels(&segment),
        OwnerLineLabels::Unreadable(_)
    ));
    assert_set_refused(root.path(), &[line(&segment, 1)], &person("ada"), |error| {
        matches!(error, OwnerLineLabelError::FileUnreadable(_))
    });
    let before = tree(root.path());
    assert!(matches!(
        remove_owner_line_labels(root.path(), &[line(&segment, 1)]),
        Err(OwnerLineLabelError::FileUnreadable(_))
    ));
    assert_eq!(changed_paths(&before, &tree(root.path())), BTreeSet::new());
    let names = fs::read_dir(path.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["owner_line_labels.json".to_owned()]);
}

#[test]
fn a_pending_merge_recovery_refuses_every_change() {
    let root = journal();
    let segment = segment(root.path(), "120000_300", 1);
    set_owner_line_labels(root.path(), &[line(&segment, 1)], &person("ada")).unwrap();
    fs::create_dir_all(root.path().join("health/entity-merge-recovery")).unwrap();
    assert_set_refused(root.path(), &[line(&segment, 1)], &person("bo"), |error| {
        *error == OwnerLineLabelError::MergeRecoveryPending
    });
    let before = tree(root.path());
    assert_eq!(
        remove_owner_line_labels(root.path(), &[line(&segment, 1)]),
        Err(OwnerLineLabelError::MergeRecoveryPending)
    );
    let set_at = entries(&segment)[0].set_at.clone();
    assert_eq!(
        remove_owner_line_label_if_set_at(root.path(), &line(&segment, 1), &set_at),
        Err(OwnerLineLabelError::MergeRecoveryPending)
    );
    assert_eq!(changed_paths(&before, &tree(root.path())), BTreeSet::new());
}

#[test]
fn lines_are_addressed_by_statement_ordinal_and_must_be_stored_with_it() {
    let root = journal();
    // A start-bearing row nobody renders comes first, so stored ids trail
    // the statement ordinals; the last stored id names no ordinal at all.
    let offset = segment_with(
        root.path(),
        "120000_300",
        &[(
            "audio.jsonl",
            &jsonl(&[
                json!({"start": "00:00:00"}),
                json!({"start": "00:00:01", "text": "two", "sentence_id": 1}),
                json!({"start": "00:00:02", "text": "three", "sentence_id": 7}),
            ]),
        )],
    );
    assert_set_refused(root.path(), &[line(&offset, 2)], &person("ada"), |error| {
        *error
            == OwnerLineLabelError::LineIdMismatch {
                audio_source: "audio".into(),
                sentence_id: 2,
                stored: Some(1),
            }
    });
    assert_set_refused(root.path(), &[line(&offset, 1)], &person("ada"), |error| {
        matches!(
            error,
            OwnerLineLabelError::LineIdMismatch { stored: None, .. }
        )
    });
    assert_set_refused(root.path(), &[line(&offset, 7)], &person("ada"), |error| {
        matches!(
            error,
            OwnerLineLabelError::LineNotFound { sentence_id: 7, .. }
        )
    });

    let aligned = segment_with(
        root.path(),
        "121000_300",
        &[(
            "audio.jsonl",
            &jsonl(&[
                json!({"raw": "audio.flac"}),
                json!({"start": "00:00:01", "text": "one", "sentence_id": 1}),
                json!({"note": "not a statement"}),
                json!({"start": "00:00:02", "text": "two", "sentence_id": 2}),
            ]),
        )],
    );
    set_owner_line_labels(root.path(), &[line(&aligned, 2)], &person("ada")).unwrap();
    let stored = entries(&aligned);
    let record = json!({"start": "00:00:02", "text": "two"});
    assert_eq!(stored[0].sentence_id, 2);
    assert_eq!(
        stored[0].fingerprint,
        LineFingerprint::of_record(record.as_object().unwrap())
    );
}

#[test]
fn a_multi_segment_set_checks_every_segment_before_writing_any() {
    let root = journal();
    let first = segment(root.path(), "120000_300", 2);
    let second = segment(root.path(), "121000_300", 2);
    assert_set_refused(
        root.path(),
        &[line(&first, 1), line(&second, 5)],
        &person("ada"),
        |error| {
            matches!(
                error,
                OwnerLineLabelError::LineNotFound { sentence_id: 5, .. }
            )
        },
    );

    set_owner_line_labels(root.path(), &[line(&first, 1)], &person("bo")).unwrap();
    let prior = entries(&first)[0].clone();
    let written = set_owner_line_labels(
        root.path(),
        &[line(&second, 2), line(&first, 1), line(&first, 2)],
        &person("ada"),
    )
    .unwrap();
    assert_eq!(written.len(), 3);
    let token = &written[0].set_at;
    assert!(written.iter().all(|line| &line.set_at == token));
    let by_line = written
        .iter()
        .map(|item| ((item.line.segment_dir.clone(), item.line.sentence_id), item))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(by_line[&(first.clone(), 1)].prior, Some(prior));
    assert_eq!(by_line[&(first.clone(), 2)].prior, None);
    assert_eq!(by_line[&(second.clone(), 2)].prior, None);
    assert!(entries(&second).iter().all(|entry| &entry.set_at == token));
}

#[test]
fn a_set_that_fails_partway_puts_earlier_segments_back() {
    let owner_files = |root: &Path| {
        tree(root)
            .into_iter()
            .filter(|(path, _)| path.ends_with("talents/owner_line_labels.json"))
            .collect::<BTreeMap<_, _>>()
    };
    for (failures, failing_call) in (0..).enumerate() {
        let root = journal();
        let first = segment(root.path(), "120000_300", 2);
        let second = segment(root.path(), "121000_300", 2);
        set_owner_line_labels(root.path(), &[line(&first, 1)], &person("bo")).unwrap();
        let before = owner_files(root.path());
        let calls = std::cell::Cell::new(0);
        let checkpoint = || {
            let call = calls.get();
            calls.set(call + 1);
            if call == failing_call {
                Err("stopped".to_owned())
            } else {
                Ok(())
            }
        };
        let result = set_owner_line_labels_checked(
            root.path(),
            &[line(&first, 1), line(&first, 2), line(&second, 1)],
            &person("ada"),
            &checkpoint,
        );
        if calls.get() <= failing_call {
            result.unwrap();
            assert!(failures > 5, "the failure reached every step");
            break;
        }
        assert!(result.is_err(), "call {failing_call}");
        assert_eq!(owner_files(root.path()), before, "call {failing_call}");
    }
}

#[test]
fn a_conditional_remove_removes_only_the_answer_it_names() {
    let root = journal();
    let segment = segment(root.path(), "120000_300", 1);
    let first = set_owner_line_labels(root.path(), &[line(&segment, 1)], &person("ada")).unwrap();
    let second = set_owner_line_labels(root.path(), &[line(&segment, 1)], &person("bo")).unwrap();
    assert_ne!(first[0].set_at, second[0].set_at);

    let path = owner_line_labels_path(&segment);
    let before = fs::read(&path).unwrap();
    let outcome =
        remove_owner_line_label_if_set_at(root.path(), &line(&segment, 1), &first[0].set_at)
            .unwrap();
    assert_eq!(
        outcome,
        ConditionalRemoval::Changed(entries(&segment)[0].clone())
    );
    assert_eq!(fs::read(&path).unwrap(), before);

    assert_eq!(
        remove_owner_line_label_if_set_at(root.path(), &line(&segment, 1), &second[0].set_at),
        Ok(ConditionalRemoval::Removed)
    );
    assert_eq!(entries(&segment), Vec::new());
    assert_eq!(
        remove_owner_line_label_if_set_at(root.path(), &line(&segment, 1), &second[0].set_at),
        Ok(ConditionalRemoval::Absent)
    );
}

#[test]
fn a_restore_puts_the_prior_answer_back_verbatim_under_its_surviving_id() {
    let root = journal();
    entity(root.path(), "cy", json!({}));
    let segment = segment(root.path(), "120000_300", 1);
    set_owner_line_labels(root.path(), &[line(&segment, 1)], &person("bo")).unwrap();
    let replaced = set_owner_line_labels(root.path(), &[line(&segment, 1)], &person("ada"))
        .unwrap()
        .remove(0);
    let prior = replaced.prior.unwrap();
    merge(root.path(), "bo", "cy");
    entity(root.path(), "cy", json!({"blocked": true}));

    let outcomes = restore_owner_line_labels(
        root.path(),
        &[LineRestore {
            segment_dir: segment.clone(),
            entry: prior.clone(),
        }],
    )
    .unwrap();
    assert_eq!(outcomes, vec![RestoreOutcome::Restored]);
    assert_eq!(
        entries(&segment),
        vec![OwnerLineEntry {
            speaker: Some("cy".to_owned()),
            ..prior.clone()
        }]
    );

    let gone = segment_with(root.path(), "121000_300", &[("tombstone.json", "{}")]);
    let before = tree(root.path());
    let outcomes = restore_owner_line_labels(
        root.path(),
        &[LineRestore {
            segment_dir: gone,
            entry: prior,
        }],
    )
    .unwrap();
    assert_eq!(outcomes, vec![RestoreOutcome::SegmentGone]);
    assert_eq!(changed_paths(&before, &tree(root.path())), BTreeSet::new());
}

#[test]
fn answers_never_touch_voice_evidence_or_owner_voice_state() {
    let root = journal();
    let segment = segment_dir(root.path(), "120000_300");
    fs::create_dir_all(&segment).unwrap();
    let statements = 4_usize;
    let payload = segment.join("payload.f32");
    let mut raw = Vec::new();
    for axis in 0..statements {
        for value in voice(axis) {
            raw.extend_from_slice(&value.to_le_bytes());
        }
    }
    fs::write(&payload, raw).unwrap();
    let request = json!({
        "schema": "solstone-speaker-transcript-write-request-v1",
        "output": {
            "jsonl_path": segment.join("audio.jsonl").display().to_string(),
            "npz_path": segment.join("audio.npz").display().to_string(),
            "redo": true,
        },
        "base_time_us_of_day": 100_000_u64,
        "source": "audio",
        "statements": (1..=statements as i64)
            .map(|id| json!({"id": id, "start_offset_us": (id - 1) * 2_000_000, "text": format!("statement {id}")}))
            .collect::<Vec<_>>(),
        "header": {
            "raw": "audio.wav", "model": "model", "device": "cpu", "compute_type": "int8",
            "speaker_evidence": "single", "speaker_evidence_version": "windowed-slots-v1",
            "speaker_evidence_multi_fraction": 0.0,
        },
        "embeddings": {
            "payload_path": payload, "payload_format": "raw-f32le-row-major-v1",
            "dtype": "float32-le", "shape": [statements, 256], "byte_count": statements * 256 * 4,
            "statement_ids": (1..=statements as i64).collect::<Vec<_>>(),
            "durations_s": vec![2.0; statements], "encoder": "test",
        }
    });
    solstone_core_speaker_id::writer::write_request(&serde_json::to_vec(&request).unwrap())
        .unwrap();
    for (id, sentence_id) in [("owner", 1), ("ada", 2), ("bo", 3)] {
        let metadata = json!({"day": DAY, "stream_layout": "named", "stream": STREAM,
            "segment_key": "120000_300", "source": "audio", "sentence_id": sentence_id,
            "added_at": 1, "last_seen_ts": 1});
        solstone_core_entity::save_voiceprints_batch(
            root.path(),
            id,
            &[VoiceprintItem {
                embedding: voice(sentence_id as usize - 1),
                metadata,
            }],
            &encoder(),
        )
        .unwrap();
    }
    crate::owner_centroid::write_owner_centroid(
        root.path(),
        "owner",
        &crate::owner_centroid::OwnerCentroidWriteInput {
            centroid: voice(0),
            cluster_size: 36,
            timestamp: "2026-08-08T12:00:00+00:00".to_owned(),
            evidence_tier: "standard".to_owned(),
        },
    )
    .unwrap();
    crate::owner_voice::update_voiceprint_state(root.path(), json!({"status": "confirmed"}))
        .unwrap();
    fs::create_dir_all(segment.join("talents")).unwrap();
    fs::write(
        segment.join("talents/speaker_labels.json"),
        json!({"labels": [
            {"sentence_id": 1, "speaker": "owner", "confidence": "high", "method": "user_assigned"},
            {"sentence_id": 2, "speaker": "ada", "confidence": "medium", "method": "acoustic"},
        ], "owner_centroid_last_refreshed_at": null, "voiceprint_versions": {}, "candidate_evidence": []})
        .to_string(),
    )
    .unwrap();
    let tier = format!(
        "{:?}",
        crate::owner_provisional::resolve_owner_tier(root.path())
    );

    let before = tree(root.path());
    set_owner_line_labels(root.path(), &[line(&segment, 1)], &LineSpeaker::Owner).unwrap();
    set_owner_line_labels(root.path(), &[line(&segment, 2)], &person("bo")).unwrap();
    set_owner_line_labels(root.path(), &[line(&segment, 3)], &LineSpeaker::Unknown).unwrap();
    let between = tree(root.path());
    let all = [line(&segment, 1), line(&segment, 2), line(&segment, 3)];
    assert_eq!(remove_owner_line_labels(root.path(), &all).unwrap(), 3);
    let after = tree(root.path());

    let store = PathBuf::from("chronicle/20260808/mic/120000_300/talents/owner_line_labels.json");
    let lock = store.with_file_name("owner_line_labels.json.lock");
    let allowed = BTreeSet::from([store, lock]);
    for (from, to) in [(&before, &between), (&between, &after), (&before, &after)] {
        assert!(changed_paths(from, to).is_subset(&allowed));
    }
    assert_eq!(changed_paths(&before, &between), allowed);
    assert_eq!(
        format!(
            "{:?}",
            crate::owner_provisional::resolve_owner_tier(root.path())
        ),
        tier
    );
}

#[test]
fn a_merge_waiting_on_a_set_carries_its_answer_to_the_surviving_id() {
    let root = journal();
    let segment = segment(root.path(), "120000_300", 1);
    // The set holds the trust lock while a merge of the person it names is
    // waiting for it; the merge then runs and must carry the new answer.
    let trust = hold_entity_trust_lock(root.path()).unwrap();
    let journal_path = root.path().to_path_buf();
    let merging = std::thread::spawn(move || merge(&journal_path, "bo", "ada"));
    set_owner_line_labels(root.path(), &[line(&segment, 1)], &person("bo")).unwrap();
    drop(trust);
    merging.join().unwrap();
    assert_eq!(entries(&segment)[0].speaker.as_deref(), Some("ada"));
    // In the other order the merged id is refused
    // (`bad_lines_and_bad_people_are_refused_with_their_own_errors`).
}

#[derive(Clone, Copy, Debug)]
enum Removal {
    Tombstone,
    Staged,
}

fn clear(directory: &Path) {
    for entry in fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            fs::remove_dir_all(path).unwrap();
        } else {
            fs::remove_file(path).unwrap();
        }
    }
}

/// What segment removal leaves at each of its stages, or `None` where Windows
/// refuses to stage it: Windows will not rename a directory while a handle beneath
/// it is open, such as the lock a write holds, so there a staged removal cannot
/// begin under the write and that refusal is what keeps the race from happening.
fn remove_segment(segment: &Path, removal: Removal) -> Option<PathBuf> {
    match removal {
        Removal::Tombstone => {
            clear(segment);
            fs::write(segment.join("tombstone.json"), b"{}").unwrap();
            Some(segment.to_path_buf())
        }
        Removal::Staged => {
            let staged = segment.with_file_name(format!(
                ".removing_{}",
                segment.file_name().unwrap().to_str().unwrap()
            ));
            match fs::rename(segment, &staged) {
                Ok(()) => {}
                #[cfg(windows)]
                Err(error) => {
                    assert_eq!(
                        error.kind(),
                        std::io::ErrorKind::PermissionDenied,
                        "staging {}: {error}",
                        segment.display()
                    );
                    return None;
                }
                #[cfg(not(windows))]
                Err(error) => panic!("staging {}: {error}", segment.display()),
            }
            clear(&staged);
            Some(staged)
        }
    }
}

#[test]
fn a_set_or_remove_racing_a_segment_delete_refuses_and_recreates_nothing() {
    let mut refusals = 0;
    for removal in [Removal::Tombstone, Removal::Staged] {
        for removing in [false, true] {
            for injected_call in 0.. {
                let root = journal();
                let segment = segment(root.path(), "120000_300", 1);
                if removing {
                    set_owner_line_labels(root.path(), &[line(&segment, 1)], &person("ada"))
                        .unwrap();
                }
                let calls = std::cell::Cell::new(0);
                let left = std::cell::RefCell::new(None);
                let refused = std::cell::Cell::new(false);
                let checkpoint = || {
                    if calls.get() == injected_call {
                        match remove_segment(&segment, removal) {
                            Some(path) => *left.borrow_mut() = Some(path),
                            None => refused.set(true),
                        }
                    }
                    calls.set(calls.get() + 1);
                    Ok(())
                };
                let lines = [line(&segment, 1)];
                let result = if removing {
                    remove_owner_line_labels_checked(root.path(), &lines, &checkpoint).map(|_| ())
                } else {
                    set_owner_line_labels_checked(root.path(), &lines, &person("bo"), &checkpoint)
                        .map(|_| ())
                };
                let Some(left) = left.into_inner() else {
                    result.unwrap();
                    if refused.get() {
                        // Windows refused to stage the segment: it is still present
                        // and the write landed in it.
                        refusals += 1;
                        assert!(segment.exists());
                        let labels = entries(&segment);
                        if removing {
                            assert_eq!(labels, Vec::new());
                        } else {
                            assert_eq!(labels[0].speaker.as_deref(), Some("bo"));
                        }
                        continue;
                    }
                    assert!(injected_call > 2, "{removal:?} reached every step");
                    break;
                };
                assert!(
                    result.is_err(),
                    "{removal:?} removing={removing} call {injected_call}"
                );
                let remaining = fs::read_dir(&left)
                    .unwrap()
                    .map(|entry| entry.unwrap().file_name().into_string().unwrap())
                    .collect::<Vec<_>>();
                match removal {
                    Removal::Tombstone => assert_eq!(remaining, vec!["tombstone.json"]),
                    Removal::Staged => {
                        assert_eq!(remaining, Vec::<String>::new());
                        assert!(!segment.exists());
                    }
                }
            }
        }
    }
    // Windows must have refused staging at least once: the write holds its lock
    // beneath the segment across a checkpoint. Elsewhere staging always succeeds.
    assert_eq!(cfg!(windows), refusals > 0, "{refusals} staging refusals");
}

fn file_bytes(segment: &Path) -> Option<Vec<u8>> {
    fs::read(owner_line_labels_path(segment)).ok()
}

#[test]
fn a_failed_change_after_a_set_puts_every_file_back_exactly() {
    let root = journal();
    let answered = segment(root.path(), "120000_300", 2);
    let fresh = segment(root.path(), "120500_300", 2);
    set_owner_line_labels(root.path(), &[line(&answered, 1)], &person("bo")).unwrap();
    let before = [file_bytes(&answered), file_bytes(&fresh)];
    assert!(before[1].is_none());
    let result = set_owner_line_labels_then(
        root.path(),
        &[line(&answered, 1), line(&fresh, 2)],
        &person("ada"),
        |set| {
            assert_eq!(set.len(), 2);
            assert_eq!(entries(&fresh).len(), 1);
            Err::<(), _>("the edit wasn't saved")
        },
    )
    .unwrap();
    assert_eq!(result, Err("the edit wasn't saved"));
    assert_eq!([file_bytes(&answered), file_bytes(&fresh)], before);
}

#[test]
fn a_revert_puts_back_only_lines_still_carrying_a_token() {
    let root = journal();
    let segment = segment(root.path(), "120000_300", 3);
    let gone = segment_with(root.path(), "121000_300", &[]);
    let prior = set_owner_line_labels(root.path(), &[line(&segment, 3)], &person("bo")).unwrap();
    let first = set_owner_line_labels(
        root.path(),
        &[line(&segment, 1), line(&segment, 2), line(&segment, 3)],
        &person("ada"),
    )
    .unwrap();
    let token = first[0].set_at.clone();
    // Line 2 has been answered again since.
    set_owner_line_labels(root.path(), &[line(&segment, 2)], &LineSpeaker::Unknown).unwrap();
    merge(root.path(), "bo", "ada");
    fs::remove_dir_all(&gone).unwrap();
    let revert = |sentence_id, restore: Option<OwnerLineEntry>, dir: &Path| LineRevert {
        line: line(dir, sentence_id),
        tokens: BTreeSet::from([token.clone()]),
        restore,
    };
    let outcomes = revert_owner_line_labels(
        root.path(),
        &[
            revert(1, None, &segment),
            revert(2, None, &segment),
            revert(3, first[2].prior.clone(), &segment),
            revert(1, None, &gone),
        ],
    )
    .unwrap();
    assert_eq!(
        outcomes,
        [
            RevertOutcome::Reverted,
            RevertOutcome::Left,
            RevertOutcome::Reverted,
            RevertOutcome::SegmentGone
        ]
    );
    let stored = entries(&segment)
        .into_iter()
        .map(|entry| (entry.sentence_id, entry.speaker, entry.set_at))
        .collect::<Vec<_>>();
    let restored = prior[0].set_at.clone();
    assert_eq!(stored.len(), 2);
    assert_eq!(stored[0].0, 2);
    assert_eq!(stored[0].1, None);
    // The prior answer named someone merged away since: it comes back as the survivor.
    assert_eq!(stored[1], (3, Some("ada".to_owned()), restored));
}

#[test]
fn the_tagged_speaker_form_reads_only_what_it_writes() {
    for speaker in [LineSpeaker::Owner, LineSpeaker::Unknown, person("ada")] {
        assert_eq!(LineSpeaker::from_tagged(&speaker.tagged()), Some(speaker));
    }
    for value in [
        json!({"kind": "person"}),
        json!({"kind": "person", "id": ""}),
        json!({"kind": "owner", "id": "ada"}),
        json!({"kind": "robot"}),
        json!("owner"),
    ] {
        assert_eq!(LineSpeaker::from_tagged(&value), None, "{value}");
    }
}
