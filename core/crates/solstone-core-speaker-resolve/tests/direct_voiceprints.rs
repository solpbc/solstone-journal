// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::fs;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::json;
use solstone_core_entity::{EncoderIdentity, load_entity_voiceprints_file, save_voiceprints_batch};
use solstone_core_journal_io::segment_path;
use solstone_core_npy::write_npy;
use solstone_core_speaker_resolve::direct_voiceprints::{
    DirectVoiceprintsError, execute_direct_voiceprints_phase, plan_direct_voiceprints,
};
use solstone_core_speaker_resolve::identify_operations::MemberProvenance;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Temp(PathBuf);

impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "solstone-direct-voiceprints-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
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

fn content_snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn collect(root: &Path, directory: &Path, snapshot: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(directory).expect("journal directory reads") {
            let entry = entry.expect("journal entry reads");
            let path = entry.path();
            if path.is_dir() {
                collect(root, &path, snapshot);
            } else if path.is_file() {
                snapshot.insert(
                    path.strip_prefix(root)
                        .expect("snapshot path is relative")
                        .to_path_buf(),
                    fs::read(path).expect("journal file reads"),
                );
            }
        }
    }

    let mut snapshot = BTreeMap::new();
    collect(root, root, &mut snapshot);
    snapshot
}

fn encoder() -> EncoderIdentity {
    EncoderIdentity {
        id: "test".to_owned(),
        sha256: "a".repeat(64),
        width: 256,
    }
}

fn vector(x: f32, y: f32) -> Vec<f32> {
    let mut values = vec![0.0; 256];
    values[0] = x;
    values[1] = y;
    values
}

fn entity(root: &Path, id: &str) {
    let path = root.join("entities").join(id);
    fs::create_dir_all(&path).unwrap();
    fs::write(
        path.join("entity.json"),
        json!({"id": id, "name": id, "type": "Person"}).to_string(),
    )
    .unwrap();
}

fn admitted_owner(root: &Path) {
    let path = root.join("entities/owner");
    fs::create_dir_all(&path).unwrap();
    fs::write(
        path.join("entity.json"),
        json!({"id":"owner","name":"Owner","type":"Person","is_principal":true}).to_string(),
    )
    .unwrap();
}

fn member_embeddings(root: &Path) -> MemberProvenance {
    let member = MemberProvenance {
        day: "20260808".to_owned(),
        stream: "mic".to_owned(),
        segment_key: "120000_300".to_owned(),
        source: "audio".to_owned(),
        sentence_id: 7,
    };
    let path = segment_path(root, &member.day, &member.segment_key, &member.stream, true).unwrap();
    let mut archive = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    archive.start_file("embeddings.npy", options).unwrap();
    archive
        .write_all(&write_npy("<f4", "(1, 256)", &floats(&vector(0.0, 1.0))))
        .unwrap();
    archive.start_file("statement_ids.npy", options).unwrap();
    archive
        .write_all(&write_npy("<i4", "(1,)", &ints(&[7])))
        .unwrap();
    fs::write(
        path.join("audio.npz"),
        archive.finish().unwrap().into_inner(),
    )
    .unwrap();
    member
}

fn floats(values: &[f32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

fn ints(values: &[i32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

#[test]
fn ac8_direct_voiceprint_replay_reports_a_crash_saved_key_without_rewriting() {
    let temporary = Temp::new();
    admitted_owner(temporary.path());
    entity(temporary.path(), "target");
    let member = member_embeddings(temporary.path());
    let encoder = encoder();
    let planned = plan_direct_voiceprints(temporary.path(), "target", &[member], 123).unwrap();
    assert_eq!(planned.plan.entries_to_add.len(), 1);
    save_voiceprints_batch(temporary.path(), "target", &planned.items, &encoder).unwrap();
    let path = temporary.path().join("entities/target/voiceprints.npz");
    let before = fs::read(&path).unwrap();
    let resnapshot = plan_direct_voiceprints(
        temporary.path(),
        "target",
        &[planned.plan.entries_to_add[0].source_member.clone()],
        123,
    )
    .unwrap();
    assert_eq!(resnapshot.plan.preexisting_keys.len(), 1);
    assert!(resnapshot.plan.entries_to_add.is_empty());

    let replay =
        execute_direct_voiceprints_phase(temporary.path(), &planned.plan, &encoder).unwrap();

    assert_eq!(replay.saved_count, 1);
    assert_eq!(replay.skipped_existing_count, 0);
    assert_eq!(
        replay.saved_keys,
        vec![planned.plan.entries_to_add[0].key.clone()]
    );
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(
        load_entity_voiceprints_file(temporary.path(), "target")
            .unwrap()
            .rows,
        1
    );
}

#[test]
fn ac8_direct_voiceprint_replay_writes_a_new_key_and_reports_it_saved() {
    let temporary = Temp::new();
    admitted_owner(temporary.path());
    entity(temporary.path(), "target");
    let member = member_embeddings(temporary.path());
    let encoder = encoder();
    let planned = plan_direct_voiceprints(temporary.path(), "target", &[member], 123).unwrap();

    let replay =
        execute_direct_voiceprints_phase(temporary.path(), &planned.plan, &encoder).unwrap();

    assert_eq!(replay.saved_count, 1);
    assert_eq!(replay.skipped_existing_count, 0);
    assert_eq!(
        replay.saved_keys,
        vec![planned.plan.entries_to_add[0].key.clone()]
    );
    assert_eq!(
        load_entity_voiceprints_file(temporary.path(), "target")
            .unwrap()
            .rows,
        1
    );
}

#[test]
fn direct_voiceprints_refuse_invalid_owner_identity_before_writing() {
    let temporary = Temp::new();
    entity(temporary.path(), "target");
    let member = member_embeddings(temporary.path());
    let before = content_snapshot(temporary.path());

    let error = plan_direct_voiceprints(temporary.path(), "target", &[member], 123)
        .expect_err("missing admitted owner refuses planning");

    assert!(matches!(
        error,
        DirectVoiceprintsError::OwnerIdentityInvalid
    ));
    assert!(
        !temporary
            .path()
            .join("entities/target/voiceprints.npz")
            .exists()
    );
    assert_eq!(content_snapshot(temporary.path()), before);
}
