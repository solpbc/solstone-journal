// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::json;
use solstone_core_speaker_resolve::backfill::{
    SpeakerLabelsState, classify_speaker_labels_text, merge_user_labels, plan_backfill_segments,
};

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Temp(PathBuf);

impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "solstone-backfill-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn audio_segment(root: &Path, key: &str, labels: &str) {
    let path = root.join("chronicle/20260808/mic").join(key);
    fs::create_dir_all(path.join("talents")).unwrap();
    fs::write(path.join("audio.npz"), b"placeholder").unwrap();
    fs::write(path.join("talents/speaker_labels.json"), labels).unwrap();
}

#[test]
fn ac6_stub_discriminator_matches_literal_and_rejects_empty_negative_twins() {
    let literal = r#"{"labels": [], "skipped": true, "reason": "no_owner_centroid"}"#;
    assert_eq!(
        classify_speaker_labels_text(literal),
        SpeakerLabelsState::Stubbed
    );
    assert_eq!(
        classify_speaker_labels_text(r#"{"labels": [], "skipped": false}"#),
        SpeakerLabelsState::Labelled
    );
    assert_eq!(
        classify_speaker_labels_text(r#"{"labels": []}"#),
        SpeakerLabelsState::Labelled
    );
}

#[test]
fn ac5_default_backfill_reprocesses_stub_and_skips_real_labelled_segment() {
    let temporary = Temp::new();
    audio_segment(
        temporary.path(),
        "120000_300",
        r#"{"labels": [], "skipped": true, "reason": "no_owner_centroid"}"#,
    );
    audio_segment(
        temporary.path(),
        "120500_300",
        r#"{"labels": [{"sentence_id": 1, "speaker": "person"}]}"#,
    );

    let plan = plan_backfill_segments(temporary.path(), false).unwrap();
    assert_eq!(plan.total_segments, 2);
    assert_eq!(plan.total_eligible, 2);
    assert_eq!(plan.already_labeled, 1);
    assert_eq!(plan.to_process.len(), 1);
    assert_eq!(plan.to_process[0].segment_key, "120000_300");
}

#[test]
fn ac2_user_method_prefix_preserves_unknown_user_labels_and_replaces_non_user_rows() {
    let preserved_unknown = json!({
        "sentence_id": 1,
        "speaker": "person-user",
        "method": "user_zzz_test",
        "opaque": {"preserve": [1, 2, 3]},
    });
    let preserved_known = json!({
        "sentence_id": 3,
        "speaker": "person-known",
        "method": "user_identified",
    });
    let current = json!({
        "labels": [
            preserved_unknown.clone(),
            {"sentence_id": 2, "speaker": "stale", "method": "cluster"},
            preserved_known.clone(),
        ],
    });
    let fresh = vec![
        json!({"sentence_id": 1, "speaker": "replacement", "method": "acoustic"}),
        json!({"sentence_id": 2, "speaker": "fresh", "method": "acoustic"}),
        json!({"sentence_id": 4, "speaker": "new", "method": "cluster"}),
    ];

    assert_eq!(
        merge_user_labels(Some(&current), &fresh),
        vec![
            preserved_unknown,
            fresh[1].clone(),
            fresh[2].clone(),
            preserved_known,
        ]
    );
}
