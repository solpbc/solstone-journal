// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::support;

use std::fs;

use serde_json::Value;

#[test]
fn populated_journal_has_the_frozen_speakers_corpus_structure() {
    let journal = support::build_populated_journal();
    assert_eq!(journal.entity_ids.len(), 11);
    assert_eq!(
        fs::read_dir(journal.root().join("entities"))
            .expect("entities directory reads")
            .count(),
        11
    );

    let ada: Value = serde_json::from_slice(
        &fs::read(journal.root().join("entities/ada_lovelace/entity.json"))
            .expect("Ada entity reads"),
    )
    .expect("Ada entity parses");
    assert_eq!(ada["name"], "Ada Lovelace");
    assert_eq!(ada["is_principal"], true);

    let blocked: Value = serde_json::from_slice(
        &fs::read(journal.root().join("entities/blocked_person/entity.json"))
            .expect("blocked entity reads"),
    )
    .expect("blocked entity parses");
    assert_eq!(blocked["name"], "Blocked Person");
    assert_eq!(blocked["blocked"], true);

    for day in 1..=31 {
        assert!(
            journal
                .root()
                .join(format!("chronicle/202607{day:02}"))
                .is_dir()
        );
    }
    for path in [
        "chronicle/20260731/field/090000_300",
        "chronicle/20260731/desk/140000_600",
        "chronicle/20260730/field/101500_120",
        "chronicle/20260729/field/173000_240",
        "chronicle/20260728/desk/080000_180",
    ] {
        assert!(journal.root().join(path).is_dir(), "missing {path}");
    }
    for path in [
        "chronicle/20260731/field/090000_300/mic_audio.jsonl",
        "chronicle/20260731/field/090000_300/mic_audio.npz",
        "chronicle/20260731/field/090000_300/mic_audio.flac",
        "chronicle/20260731/field/090000_300/mic_audio.xyz",
        "chronicle/20260731/field/090000_300/talents/speaker_labels.json",
        "chronicle/20260731/desk/140000_600/sys_audio.jsonl",
        "chronicle/20260731/desk/140000_600/sys_audio.npz",
        "chronicle/20260731/desk/140000_600/sys_audio.flac",
        "chronicle/20260730/field/101500_120/mic_audio.jsonl",
        "chronicle/20260730/field/101500_120/mic_audio.flac",
        "chronicle/20260729/field/173000_240/mic_audio.jsonl",
        "chronicle/20260729/field/173000_240/talents/speaker_labels.json",
        "chronicle/20260728/desk/080000_180/mic_audio.jsonl",
        "chronicle/20260728/desk/080000_180/talents/speaker_labels.json",
        "awareness/discovery_clusters.json",
    ] {
        assert!(journal.root().join(path).is_file(), "missing {path}");
    }
}

#[test]
fn populated_journal_retains_oracle_sensitive_voiceprint_vectors_exactly() {
    let journal = support::build_populated_journal();
    let oracle = support::oracle_voiceprints();

    for entity_id in ["grace_hopper", "alan_turing"] {
        assert_eq!(
            support::read_embeddings_npz(
                &journal
                    .root()
                    .join(format!("entities/{entity_id}/voiceprints.npz")),
            ),
            oracle[entity_id],
            "{entity_id} vectors must remain bit-exact"
        );
    }
}
