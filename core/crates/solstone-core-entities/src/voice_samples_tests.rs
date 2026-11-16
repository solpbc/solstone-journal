// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::json;
use solstone_core_entity::{
    EncoderIdentity, VoiceprintItem, load_entity_voiceprints_file, save_entity_identity,
    save_voiceprints_batch,
};

#[test]
fn sample_review_removes_only_the_selected_provenance_and_updates_the_archive_count() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    save_entity_identity(
        root,
        "person",
        &json!({"id":"person","name":"Person","type":"Person"}),
        None,
    )
    .unwrap();
    let encoder = EncoderIdentity {
        id: "test".into(),
        sha256: "a".repeat(64),
        width: 256,
    };
    let items: Vec<_> = (0..2).map(|id| VoiceprintItem {
        embedding: vec![1.; 256],
        metadata: json!({"day":"20261112", "stream":"mic", "stream_layout":"named", "segment_key":"120000_300", "source":"audio", "sentence_id":id, "added_at":"2026-11-16T12:00:00Z"}),
    }).collect();
    save_voiceprints_batch(root, "person", &items, &encoder).unwrap();
    let before = crate::voice_samples::list(root, "person", 0).unwrap();
    assert_eq!(before["total"], 2);
    assert_eq!(before["items"][0]["day"], "20261112");
    assert!(before["items"][0].get("embedding").is_none());
    let key = before["items"][0]["key"].as_str().unwrap();
    assert_eq!(
        crate::voice_samples::remove(root, "person", key).unwrap()["removed"],
        1
    );
    assert_eq!(
        crate::voice_samples::remove(root, "person", key).unwrap()["removed"],
        0
    );
    let remaining = crate::voice_samples::list(root, "person", 0).unwrap();
    assert_eq!(remaining["total"], 1);
    assert_eq!(remaining["items"][0]["sentence_id"], 1);
    assert_eq!(
        load_entity_voiceprints_file(root, "person").unwrap().rows,
        1
    );
    assert!(crate::voice_samples::remove(root, "person", "invalid!").is_err());
}
