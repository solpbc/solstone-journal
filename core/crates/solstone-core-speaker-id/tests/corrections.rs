// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use solstone_core_journal_io::{LockError, LockOptions, hold_lock};
use solstone_core_speaker_id::corrections::{
    CorrectionsError, append_correction, read_corrections,
};

const CORRECTIONS_ONE: &[u8] = b"{\n  \"corrections\": [\n    {\n      \"sentence_id\": 1,\n      \"original_speaker\": \"Jos\\u00e9 \\ud83d\\ude3a\",\n      \"corrected_speaker\": \"Zo\\u00eb \\ud83e\\udded\",\n      \"reason\": \"Caf\\u00e9 \\ud83e\\uddea\"\n    }\n  ]\n}";

const CORRECTIONS_TWO: &[u8] = b"{\n  \"corrections\": [\n    {\n      \"sentence_id\": 1,\n      \"original_speaker\": \"Jos\\u00e9 \\ud83d\\ude3a\",\n      \"corrected_speaker\": \"Zo\\u00eb \\ud83e\\udded\",\n      \"reason\": \"Caf\\u00e9 \\ud83e\\uddea\"\n    },\n    {\n      \"sentence_id\": 2,\n      \"original_speaker\": null,\n      \"corrected_speaker\": \"Ren\\u00e9e\",\n      \"reason\": \"second \\ud83c\\udf1f\"\n    }\n  ]\n}";

fn temporary_segment(name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_nanos();
    let segment = std::env::temp_dir().join(format!(
        "solstone-core-speaker-id-{name}-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir_all(segment.join("talents")).expect("temporary talents directory is created");
    segment
}

fn corrections_path(segment: &Path) -> PathBuf {
    segment.join("talents").join("speaker_corrections.json")
}

#[test]
fn ac1_append_creates_file_with_single_correction_matching_python_bytes() {
    let segment = temporary_segment("single");
    let path = corrections_path(&segment);

    append_correction(
        &segment,
        serde_json::from_str::<Value>(
            r#"{"sentence_id": 1, "original_speaker": "José 😺", "corrected_speaker": "Zoë 🧭", "reason": "Café 🧪"}"#,
        )
        .expect("fixture is valid JSON")
        .as_object()
        .expect("fixture is an object")
        .clone(),
    )
    .expect("append succeeds");

    assert_eq!(
        fs::read(&path).expect("corrections are readable"),
        CORRECTIONS_ONE
    );
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
fn ac2_second_append_preserves_first_matching_python_bytes() {
    let segment = temporary_segment("two");
    let path = corrections_path(&segment);

    append_correction(
        &segment,
        serde_json::from_str::<Value>(
            r#"{"sentence_id": 1, "original_speaker": "José 😺", "corrected_speaker": "Zoë 🧭", "reason": "Café 🧪"}"#,
        )
        .expect("fixture is valid JSON")
        .as_object()
        .expect("fixture is an object")
        .clone(),
    )
    .expect("first append succeeds");
    append_correction(
        &segment,
        serde_json::from_str::<Value>(
            r#"{"sentence_id": 2, "original_speaker": null, "corrected_speaker": "Renée", "reason": "second 🌟"}"#,
        )
        .expect("fixture is valid JSON")
        .as_object()
        .expect("fixture is an object")
        .clone(),
    )
    .expect("second append succeeds");

    assert_eq!(
        fs::read(&path).expect("corrections are readable"),
        CORRECTIONS_TWO
    );
    fs::remove_dir_all(segment).expect("temporary segment is removed");
}

#[test]
fn ac3_append_to_absent_file_creates_it() {
    let segment = temporary_segment("absent");
    let path = corrections_path(&segment);

    assert!(!path.exists());
    append_correction(
        &segment,
        serde_json::from_str::<Value>(
            r#"{"sentence_id": 1, "original_speaker": "José 😺", "corrected_speaker": "Zoë 🧭", "reason": "Café 🧪"}"#,
        )
        .expect("fixture is valid JSON")
        .as_object()
        .expect("fixture is an object")
        .clone(),
    )
    .expect("append succeeds");
    assert!(path.exists());

    fs::remove_dir_all(segment).expect("temporary segment is removed");
}

#[test]
fn ac4_append_to_corrupt_file_is_refused_bytes_unchanged() {
    let segment = temporary_segment("corrupt");
    let path = corrections_path(&segment);
    let before = b"not json";
    fs::write(&path, before).expect("corrupt fixture is written");

    let error = append_correction(
        &segment,
        serde_json::from_str::<Value>(
            r#"{"sentence_id": 1, "original_speaker": "José 😺", "corrected_speaker": "Zoë 🧭", "reason": "Café 🧪"}"#,
        )
        .expect("fixture is valid JSON")
        .as_object()
        .expect("fixture is an object")
        .clone(),
    )
    .expect_err("corrupt data is refused");

    assert!(matches!(error, CorrectionsError::Malformed { .. }));
    assert_eq!(fs::read(&path).expect("corrections are readable"), before);
    fs::remove_dir_all(segment).expect("temporary segment is removed");
}

#[test]
fn ac5_append_to_wellformed_file_preserves_all_prior_rows() {
    let segment = temporary_segment("prior-rows");
    let path = corrections_path(&segment);

    append_correction(
        &segment,
        serde_json::from_str::<Value>(
            r#"{"sentence_id": 1, "original_speaker": "José 😺", "corrected_speaker": "Zoë 🧭", "reason": "Café 🧪"}"#,
        )
        .expect("fixture is valid JSON")
        .as_object()
        .expect("fixture is an object")
        .clone(),
    )
    .expect("first append succeeds");
    append_correction(
        &segment,
        serde_json::from_str::<Value>(
            r#"{"sentence_id": 2, "original_speaker": null, "corrected_speaker": "Renée", "reason": "second 🌟"}"#,
        )
        .expect("fixture is valid JSON")
        .as_object()
        .expect("fixture is an object")
        .clone(),
    )
    .expect("second append succeeds");

    let corrections = read_corrections(&segment).expect("corrections are readable");
    assert_eq!(corrections.len(), 2);
    assert_eq!(
        fs::read(&path).expect("corrections are readable"),
        CORRECTIONS_TWO
    );
    fs::remove_dir_all(segment).expect("temporary segment is removed");
}

#[test]
fn ac6_read_corrections_distinguishes_absent_wellformed_and_corrupt() {
    let segment = temporary_segment("read");
    let path = corrections_path(&segment);

    assert_eq!(
        read_corrections(&segment).expect("absent file is empty"),
        Vec::<Value>::new()
    );
    fs::write(&path, CORRECTIONS_ONE).expect("well-formed fixture is written");
    assert_eq!(
        read_corrections(&segment)
            .expect("well-formed file is read")
            .len(),
        1
    );
    fs::write(&path, b"not json").expect("corrupt fixture is written");
    assert!(matches!(
        read_corrections(&segment),
        Err(CorrectionsError::Malformed { .. })
    ));

    fs::remove_dir_all(segment).expect("temporary segment is removed");
}

#[test]
fn ac7_append_refuses_when_lock_times_out_without_changing_bytes() {
    let segment = temporary_segment("lock");
    let path = corrections_path(&segment);
    fs::write(&path, CORRECTIONS_ONE).expect("fixture is written");
    let before = fs::read(&path).expect("fixture is readable");
    let _held_lock = hold_lock(&path, LockOptions::default()).expect("lock is held");

    let error = append_correction(
        &segment,
        serde_json::from_str::<Value>(
            r#"{"sentence_id": 2, "original_speaker": null, "corrected_speaker": "Renée", "reason": "second 🌟"}"#,
        )
        .expect("fixture is valid JSON")
        .as_object()
        .expect("fixture is an object")
        .clone(),
    )
    .expect_err("second lock times out");

    assert!(matches!(
        error,
        CorrectionsError::Lock(LockError::Timeout(_))
    ));
    assert_eq!(fs::read(&path).expect("corrections are readable"), before);
    fs::remove_dir_all(segment).expect("temporary segment is removed");
}
