// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value};
use solstone_core_journal_io::{LockError, LockOptions, hold_lock};
use solstone_core_speaker_id::labels::{
    LabelsError, patch_labels, write_full_labels, write_stub_labels,
};

const FULL: &[u8] = b"{\n  \"unknown_top\": {\n    \"preserve\": \"yes\"\n  },\n  \"labels\": [\n    {\n      \"sentence_id\": 1,\n      \"speaker\": \"Jos\\u00e9 \\ud83d\\ude3a\",\n      \"confidence\": \"medium\",\n      \"method\": \"acoustic\",\n      \"owner_margin_declined\": true,\n      \"unknown_row\": \"kept\"\n    },\n    {\n      \"sentence_id\": 2,\n      \"speaker\": null,\n      \"confidence\": \"low\",\n      \"method\": \"context\",\n      \"unknown_row\": \"also_kept\"\n    }\n  ],\n  \"owner_centroid_last_refreshed_at\": \"2026-08-08T00:00:00Z\",\n  \"voiceprint_versions\": {\n    \"jos\\u00e9\": 2\n  },\n  \"candidate_evidence\": [\n    {\n      \"name\": \"Jos\\u00e9 \\ud83d\\ude3a\"\n    }\n  ],\n  \"candidate_evidence_gaps\": [\n    {\n      \"source\": \"screen\",\n      \"reason\": \"Caf\\u00e9 \\ud83e\\uddea\"\n    }\n  ]\n}\n";

const STUB: &[u8] =
    b"{\n  \"labels\": [],\n  \"skipped\": true,\n  \"reason\": \"d\\u00e9j\\u00e0 \\ud83c\\udf1f\"\n}\n";

fn temporary_segment(name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_nanos();
    let segment = std::env::temp_dir().join(format!(
        "solstone-core-speaker-id-labels-{name}-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir_all(segment.join("talents")).expect("temporary talents directory is created");
    segment
}

fn labels_path(segment: &Path) -> PathBuf {
    segment.join("talents").join("speaker_labels.json")
}

fn corrections_path(segment: &Path) -> PathBuf {
    segment.join("talents").join("speaker_corrections.json")
}

#[test]
fn ac1_patch_unknown_sentence_without_insert_does_not_create_file() {
    let segment = temporary_segment("patch-unknown");
    let path = labels_path(&segment);
    let patches = [(
        7,
        serde_json::json!({"speaker": "owner"})
            .as_object()
            .expect("fixture is an object")
            .clone(),
    )];

    let error = patch_labels(&segment, &patches, false).expect_err("unknown sentence is refused");

    assert!(matches!(error, LabelsError::SentenceIdNotFound(7)));
    assert!(!path.exists());
    fs::remove_dir_all(segment).expect("temporary segment is removed");
}

#[test]
fn ac2_full_rewrite_matches_python_bytes_and_creates_file() {
    let segment = temporary_segment("full-bytes");
    let path = labels_path(&segment);
    fs::write(&path, b"{\"unknown_top\": {\"preserve\": \"yes\"}}").expect("seed is written");
    let fresh_labels = serde_json::from_str::<Value>(
        r#"[
          {"sentence_id": 1, "speaker": "José 😺", "confidence": "medium", "method": "acoustic", "owner_margin_declined": true, "unknown_row": "kept"},
          {"sentence_id": 2, "speaker": null, "confidence": "low", "method": "context", "unknown_row": "also_kept"}
        ]"#,
    )
    .expect("fixture is valid JSON")
    .as_array()
    .expect("fixture is an array")
    .clone();
    let metadata = serde_json::from_str::<Value>(
        r#"{
          "owner_centroid_last_refreshed_at": "2026-08-08T00:00:00Z",
          "voiceprint_versions": {"josé": 2},
          "candidate_evidence": [{"name": "José 😺"}],
          "candidate_evidence_gaps": [{"source": "screen", "reason": "Café 🧪"}]
        }"#,
    )
    .expect("fixture is valid JSON")
    .as_object()
    .expect("fixture is an object")
    .clone();

    write_full_labels(&segment, fresh_labels, &metadata).expect("full rewrite succeeds");

    assert_eq!(fs::read(&path).expect("labels are readable"), FULL);
    assert!(path.exists());
    #[cfg(unix)]
    assert_eq!(
        fs::metadata(&path)
            .expect("metadata is readable")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    let result: Value = serde_json::from_slice(FULL).expect("fixture is valid JSON");
    assert!(
        result["labels"][0]
            .get("acoustic_margin_declined")
            .is_none()
    );
    fs::remove_dir_all(segment).expect("temporary segment is removed");
}

#[test]
fn ac3_stub_matches_python_bytes_and_creates_file() {
    let segment = temporary_segment("stub");
    let path = labels_path(&segment);

    write_stub_labels(&segment, "déjà 🌟").expect("stub write succeeds");

    assert_eq!(fs::read(&path).expect("labels are readable"), STUB);
    assert!(path.exists());
    #[cfg(unix)]
    assert_eq!(
        fs::metadata(&path)
            .expect("metadata is readable")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    fs::remove_dir_all(segment).expect("temporary segment is removed");
}

#[test]
fn ac4_full_rewrite_preserves_unknown_top_and_user_row_key_order() {
    let segment = temporary_segment("order");
    let path = labels_path(&segment);
    fs::write(
        &path,
        br#"{"z_top":{"value":"keep"},"labels":[{"sentence_id":1,"z_row":"keep","speaker":"owner","method":"user_assigned"}],"a_top":true}"#,
    )
    .expect("seed is written");
    let fresh_labels = serde_json::from_str::<Value>(
        r#"[{"sentence_id":1,"speaker":"pipeline","method":"acoustic"}]"#,
    )
    .expect("fixture is valid JSON")
    .as_array()
    .expect("fixture is an array")
    .clone();

    write_full_labels(&segment, fresh_labels, &Map::new()).expect("full rewrite succeeds");

    let bytes = fs::read(&path).expect("labels are readable");
    assert_eq!(
        bytes,
        b"{\n  \"z_top\": {\n    \"value\": \"keep\"\n  },\n  \"labels\": [\n    {\n      \"sentence_id\": 1,\n      \"z_row\": \"keep\",\n      \"speaker\": \"owner\",\n      \"method\": \"user_assigned\"\n    }\n  ],\n  \"a_top\": true,\n  \"owner_centroid_last_refreshed_at\": null,\n  \"voiceprint_versions\": {},\n  \"candidate_evidence\": []\n}\n"
    );
    let result: Value = serde_json::from_slice(&bytes).expect("result is valid JSON");
    let top_keys: Vec<_> = result
        .as_object()
        .expect("result is an object")
        .keys()
        .collect();
    assert_eq!(top_keys[..3], ["z_top", "labels", "a_top"]);
    let row = result["labels"][0].as_object().expect("row is an object");
    let row_keys: Vec<_> = row.keys().collect();
    assert_eq!(row_keys[..2], ["sentence_id", "z_row"]);
    assert_eq!(row["z_row"], "keep");
    fs::remove_dir_all(segment).expect("temporary segment is removed");
}

#[test]
fn ac5_full_rewrite_keeps_only_user_rows_missing_from_fresh_tail() {
    let segment = temporary_segment("user-tail");
    let path = labels_path(&segment);
    fs::write(
        &path,
        br#"{"labels":[{"sentence_id":3,"speaker":"owner","method":"user_assigned"},{"sentence_id":4,"speaker":"pipeline","method":"acoustic"}]}"#,
    )
    .expect("seed is written");
    let fresh_labels = serde_json::from_str::<Value>(
        r#"[{"sentence_id":1,"speaker":"fresh","method":"acoustic"}]"#,
    )
    .expect("fixture is valid JSON")
    .as_array()
    .expect("fixture is an array")
    .clone();

    write_full_labels(&segment, fresh_labels, &Map::new()).expect("full rewrite succeeds");

    let result: Value = serde_json::from_slice(&fs::read(&path).expect("labels are readable"))
        .expect("result is valid JSON");
    assert_eq!(result["labels"][0]["sentence_id"], 1);
    assert_eq!(result["labels"][1]["sentence_id"], 3);
    assert_eq!(
        result["labels"]
            .as_array()
            .expect("labels is an array")
            .len(),
        2
    );
    fs::remove_dir_all(segment).expect("temporary segment is removed");
}

#[test]
fn ac6_current_user_label_wins_over_correction_for_same_sentence() {
    let segment = temporary_segment("user-wins");
    let path = labels_path(&segment);
    fs::write(
        &path,
        br#"{"labels":[{"sentence_id":1,"speaker":"owner","method":"user_assigned","unknown_row":"keep"}]}"#,
    )
    .expect("seed is written");
    fs::write(
        corrections_path(&segment),
        br#"{"corrections":[{"sentence_id":1,"original_speaker":"pipeline","corrected_speaker":"correction"}]}"#,
    )
    .expect("corrections are written");
    let fresh_labels = serde_json::from_str::<Value>(
        r#"[{"sentence_id":1,"speaker":"pipeline","method":"acoustic"}]"#,
    )
    .expect("fixture is valid JSON")
    .as_array()
    .expect("fixture is an array")
    .clone();

    write_full_labels(&segment, fresh_labels, &Map::new()).expect("full rewrite succeeds");

    let result: Value = serde_json::from_slice(&fs::read(&path).expect("labels are readable"))
        .expect("result is valid JSON");
    assert_eq!(result["labels"][0]["speaker"], "owner");
    assert_eq!(result["labels"][0]["unknown_row"], "keep");
    fs::remove_dir_all(segment).expect("temporary segment is removed");
}

#[test]
fn ac7_last_correction_for_sentence_wins() {
    let segment = temporary_segment("last-correction");
    let path = labels_path(&segment);
    fs::write(
        corrections_path(&segment),
        br#"{"corrections":[{"sentence_id":1,"original_speaker":"pipeline","corrected_speaker":"first"},{"sentence_id":1,"original_speaker":"pipeline","corrected_speaker":"last"}]}"#,
    )
    .expect("corrections are written");
    let fresh_labels = serde_json::from_str::<Value>(
        r#"[{"sentence_id":1,"speaker":"pipeline","confidence":"low","method":"acoustic"}]"#,
    )
    .expect("fixture is valid JSON")
    .as_array()
    .expect("fixture is an array")
    .clone();

    write_full_labels(&segment, fresh_labels, &Map::new()).expect("full rewrite succeeds");

    let result: Value = serde_json::from_slice(&fs::read(&path).expect("labels are readable"))
        .expect("result is valid JSON");
    assert_eq!(result["labels"][0]["speaker"], "last");
    fs::remove_dir_all(segment).expect("temporary segment is removed");
}

#[test]
fn ac8_null_correction_is_noop_unless_identify_undo() {
    let segment = temporary_segment("null-correction");
    let path = labels_path(&segment);
    fs::write(
        corrections_path(&segment),
        br#"{"corrections":[{"sentence_id":1,"corrected_speaker":null},{"sentence_id":2,"corrected_speaker":null,"correction_kind":"identify_undo"}]}"#,
    )
    .expect("corrections are written");
    let fresh_labels = serde_json::from_str::<Value>(
        r#"[
          {"sentence_id":1,"speaker":"one","confidence":"low","method":"acoustic"},
          {"sentence_id":2,"speaker":"two","confidence":"medium","method":"context"}
        ]"#,
    )
    .expect("fixture is valid JSON")
    .as_array()
    .expect("fixture is an array")
    .clone();

    write_full_labels(&segment, fresh_labels, &Map::new()).expect("full rewrite succeeds");

    let result: Value = serde_json::from_slice(&fs::read(&path).expect("labels are readable"))
        .expect("result is valid JSON");
    assert_eq!(result["labels"][0]["speaker"], "one");
    assert_eq!(result["labels"][0]["confidence"], "low");
    assert_eq!(result["labels"][1]["speaker"], Value::Null);
    assert_eq!(result["labels"][1]["confidence"], Value::Null);
    assert_eq!(result["labels"][1]["method"], Value::Null);
    fs::remove_dir_all(segment).expect("temporary segment is removed");
}

#[test]
fn ac9_correction_overlay_sets_high_confidence_and_user_methods() {
    let segment = temporary_segment("overlay-methods");
    let path = labels_path(&segment);
    fs::write(
        corrections_path(&segment),
        br#"{"corrections":[{"sentence_id":1,"original_speaker":"same","corrected_speaker":"same"},{"sentence_id":2,"corrected_speaker":"assigned"},{"sentence_id":3,"original_speaker":"before","corrected_speaker":"corrected"}]}"#,
    )
    .expect("corrections are written");
    let fresh_labels = serde_json::from_str::<Value>(
        r#"[
          {"sentence_id":1,"speaker":"pipeline","confidence":"low","method":"acoustic"},
          {"sentence_id":2,"speaker":"pipeline","confidence":"low","method":"acoustic"},
          {"sentence_id":3,"speaker":"pipeline","confidence":"low","method":"acoustic"}
        ]"#,
    )
    .expect("fixture is valid JSON")
    .as_array()
    .expect("fixture is an array")
    .clone();

    write_full_labels(&segment, fresh_labels, &Map::new()).expect("full rewrite succeeds");

    let result: Value = serde_json::from_slice(&fs::read(&path).expect("labels are readable"))
        .expect("result is valid JSON");
    assert_eq!(result["labels"][0]["method"], "user_confirmed");
    assert_eq!(result["labels"][1]["method"], "user_assigned");
    assert_eq!(result["labels"][2]["method"], "user_corrected");
    assert!(
        result["labels"]
            .as_array()
            .expect("labels is an array")
            .iter()
            .all(|label| label["confidence"] == "high")
    );
    fs::remove_dir_all(segment).expect("temporary segment is removed");
}

#[test]
fn ac10_full_rewrite_refuses_corrupt_corrections_but_absent_proceeds() {
    let corrupt_segment = temporary_segment("corrupt-corrections");
    let corrupt_path = labels_path(&corrupt_segment);
    let before = b"{\"labels\":[{\"sentence_id\":1}]}";
    fs::write(&corrupt_path, before).expect("labels seed is written");
    fs::write(corrections_path(&corrupt_segment), b"not json")
        .expect("corrupt corrections are written");
    let fresh_labels = serde_json::from_str::<Value>(r#"[{"sentence_id":1,"speaker":"fresh"}]"#)
        .expect("fixture is valid JSON")
        .as_array()
        .expect("fixture is an array")
        .clone();

    let error = write_full_labels(&corrupt_segment, fresh_labels, &Map::new())
        .expect_err("corrupt corrections are refused");

    assert!(matches!(error, LabelsError::Corrections(_)));
    assert_eq!(
        fs::read(&corrupt_path).expect("labels are readable"),
        before
    );
    fs::remove_dir_all(corrupt_segment).expect("temporary segment is removed");

    let absent_segment = temporary_segment("absent-corrections");
    let absent_path = labels_path(&absent_segment);
    let fresh_labels = serde_json::from_str::<Value>(r#"[{"sentence_id":1,"speaker":"fresh"}]"#)
        .expect("fixture is valid JSON")
        .as_array()
        .expect("fixture is an array")
        .clone();
    write_full_labels(&absent_segment, fresh_labels, &Map::new())
        .expect("absent corrections proceed");
    assert!(absent_path.exists());
    fs::remove_dir_all(absent_segment).expect("temporary segment is removed");
}

#[test]
fn ac11_corrupt_corrections_do_not_block_stub_or_patch() {
    let segment = temporary_segment("corrupt-ignored");
    let path = labels_path(&segment);
    fs::write(corrections_path(&segment), b"not json").expect("corrupt corrections are written");

    write_stub_labels(&segment, "skip").expect("stub succeeds without corrections read");
    let patches = [(
        4,
        serde_json::json!({"speaker": "patched"})
            .as_object()
            .expect("fixture is an object")
            .clone(),
    )];
    patch_labels(&segment, &patches, true).expect("patch succeeds without corrections read");

    let result: Value = serde_json::from_slice(&fs::read(&path).expect("labels are readable"))
        .expect("result is valid JSON");
    assert_eq!(result["labels"][0]["sentence_id"], 4);
    assert_eq!(result["labels"][0]["speaker"], "patched");
    fs::remove_dir_all(segment).expect("temporary segment is removed");
}

#[test]
fn ac12_each_labels_write_refuses_a_held_lock_without_writing() {
    let full_segment = temporary_segment("full-lock");
    let full_path = labels_path(&full_segment);
    let _full_lock = hold_lock(&full_path, LockOptions::default()).expect("lock is held");
    let error = write_full_labels(&full_segment, Vec::new(), &Map::new())
        .expect_err("full write times out");
    assert!(matches!(error, LabelsError::Lock(LockError::Timeout(_))));
    assert!(!full_path.exists());

    let stub_segment = temporary_segment("stub-lock");
    let stub_path = labels_path(&stub_segment);
    let _stub_lock = hold_lock(&stub_path, LockOptions::default()).expect("lock is held");
    let error = write_stub_labels(&stub_segment, "skip").expect_err("stub write times out");
    assert!(matches!(error, LabelsError::Lock(LockError::Timeout(_))));
    assert!(!stub_path.exists());

    let patch_segment = temporary_segment("patch-lock");
    let patch_path = labels_path(&patch_segment);
    let _patch_lock = hold_lock(&patch_path, LockOptions::default()).expect("lock is held");
    let error = patch_labels(&patch_segment, &[], true).expect_err("patch write times out");
    assert!(matches!(error, LabelsError::Lock(LockError::Timeout(_))));
    assert!(!patch_path.exists());

    fs::remove_dir_all(full_segment).expect("temporary segment is removed");
    fs::remove_dir_all(stub_segment).expect("temporary segment is removed");
    fs::remove_dir_all(patch_segment).expect("temporary segment is removed");
}

#[test]
fn ac13_patch_insert_sorts_valid_ids_before_sidless_rows() {
    let segment = temporary_segment("insert-sort");
    let path = labels_path(&segment);
    fs::write(
        &path,
        br#"{"labels":[{"sentence_id":10,"speaker":"ten"},{"speaker":"sidless"},{"sentence_id":2,"speaker":"two"}]}"#,
    )
    .expect("seed is written");
    let patches = [(
        5,
        serde_json::json!({"speaker": "five"})
            .as_object()
            .expect("fixture is an object")
            .clone(),
    )];

    patch_labels(&segment, &patches, true).expect("insert patch succeeds");

    let result: Value = serde_json::from_slice(&fs::read(&path).expect("labels are readable"))
        .expect("result is valid JSON");
    let labels = result["labels"].as_array().expect("labels is an array");
    assert_eq!(labels[0]["sentence_id"], 2);
    assert_eq!(labels[1]["sentence_id"], 5);
    assert_eq!(labels[2]["sentence_id"], 10);
    assert!(labels[3].get("sentence_id").is_none());
    assert!(path.exists());
    #[cfg(unix)]
    assert_eq!(
        fs::metadata(&path)
            .expect("metadata is readable")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    fs::remove_dir_all(segment).expect("temporary segment is removed");
}

#[test]
fn ac14_patch_targets_sentence_id_without_reordering_existing_rows() {
    let segment = temporary_segment("patch-by-id");
    let path = labels_path(&segment);
    fs::write(
        &path,
        br#"{"labels":[{"sentence_id":9,"speaker":"nine"},{"sentence_id":1,"speaker":"one"},{"sentence_id":5,"speaker":"five"},{"sentence_id":2,"speaker":"two"},{"sentence_id":7,"speaker":"seven"}]}"#,
    )
    .expect("seed is written");
    let patches = [(
        5,
        serde_json::json!({"speaker": "patched"})
            .as_object()
            .expect("fixture is an object")
            .clone(),
    )];

    patch_labels(&segment, &patches, false).expect("patch succeeds");

    let result: Value = serde_json::from_slice(&fs::read(&path).expect("labels are readable"))
        .expect("result is valid JSON");
    let labels = result["labels"].as_array().expect("labels is an array");
    assert_eq!(labels[2]["sentence_id"], 5);
    assert_eq!(labels[2]["speaker"], "patched");
    assert_eq!(labels[0]["sentence_id"], 9);
    assert_eq!(labels[4]["sentence_id"], 7);
    fs::remove_dir_all(segment).expect("temporary segment is removed");
}

#[test]
fn ac15_full_rewrite_preserves_margin_flags_without_normalizing_absence() {
    let segment = temporary_segment("margin-flags");
    let path = labels_path(&segment);
    let fresh_labels = serde_json::from_str::<Value>(
        r#"[
          {"sentence_id":1,"owner_margin_declined":true},
          {"sentence_id":2,"acoustic_margin_declined":true},
          {"sentence_id":3}
        ]"#,
    )
    .expect("fixture is valid JSON")
    .as_array()
    .expect("fixture is an array")
    .clone();
    let metadata = serde_json::from_str::<Value>(
        r#"{
          "owner_centroid_last_refreshed_at":"now",
          "voiceprint_versions":{"owner":1},
          "candidate_evidence":[{"source":"audio"}],
          "candidate_evidence_gaps":[{"reason":"gap"}]
        }"#,
    )
    .expect("fixture is valid JSON")
    .as_object()
    .expect("fixture is an object")
    .clone();

    write_full_labels(&segment, fresh_labels, &metadata).expect("full rewrite succeeds");

    let result: Value = serde_json::from_slice(&fs::read(&path).expect("labels are readable"))
        .expect("result is valid JSON");
    assert_eq!(result["labels"][0]["owner_margin_declined"], true);
    assert!(
        result["labels"][0]
            .get("acoustic_margin_declined")
            .is_none()
    );
    assert_eq!(result["labels"][1]["acoustic_margin_declined"], true);
    assert!(result["labels"][1].get("owner_margin_declined").is_none());
    assert!(result["labels"][2].get("owner_margin_declined").is_none());
    assert!(
        result["labels"][2]
            .get("acoustic_margin_declined")
            .is_none()
    );
    assert_eq!(result["owner_centroid_last_refreshed_at"], "now");
    assert_eq!(result["voiceprint_versions"]["owner"], 1);
    assert_eq!(result["candidate_evidence"][0]["source"], "audio");
    assert_eq!(result["candidate_evidence_gaps"][0]["reason"], "gap");
    fs::remove_dir_all(segment).expect("temporary segment is removed");
}
