// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
#![allow(clippy::disallowed_methods, clippy::unwrap_used)]
use serde_json::{Map, Value, json};
use solstone_core_entity::faceprints::{self, accumulation};
use std::{fs, path::Path};

fn fixture(root: &Path) {
    crate::create_facet(root, "work", "Work", "", "", "", None).unwrap();
    fs::create_dir_all(root.join("config")).unwrap();
    fs::write(
        root.join("config/journal.json"),
        br#"{"faces":{"enabled":true}}"#,
    )
    .unwrap();
    for (id, name, owner) in [
        ("owner", "Owner", true),
        ("a", "Person A", false),
        ("b", "Person B", false),
    ] {
        let dir = root.join("entities").join(id);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("entity.json"),serde_json::to_vec(&json!({"id":id,"name":name,"type":"Person","is_principal":owner,"aka":if owner {vec!["Owner"]}else{vec!["Tile Alias"]}})).unwrap()).unwrap();
    }
    let seg = root.join("chronicle/20261101/screen/090000_300");
    fs::create_dir_all(&seg).unwrap();
    let screen = seg.join("screen.jsonl");
    fs::write(&screen,format!("{{}}\n{}\n",json!({"frame_id":1,"content":{"meeting":{"participants":[{"name":"Tile Alias","box_2d":[0,0,500,500]},{"name":"Owner","box_2d":[500,500,1000,1000]}]}}}))).unwrap();
    let prefix = accumulation::source_prefix("screen.webm");
    let mut body = format!(
        "{}\n",
        json!({"schema_version":1,"source_name":"screen.webm","embeddings":format!("{prefix}-test.npy"),"screen_body_sha256":solstone_core_segment::screen_body_digest(&screen).unwrap(),"embedder":{"id":"synthetic","sha256":"0".repeat(64),"width":128}})
    );
    let mut values = vec![];
    fs::create_dir(seg.join(format!("{prefix}-crops"))).unwrap();
    for (i, name) in ["Tile Alias", "Owner"].iter().enumerate() {
        let crop = format!("{prefix}-crops/1-{i}-synthetic.jpg");
        fs::write(seg.join(&crop), [1, 2, 3]).unwrap();
        body += &format!(
            "{}\n",
            json!({"frame_id":1,"face_idx":i,"emb_row":i,"box":[0,0,100,100],"quality":0.9,"det_score":0.99,"category":"meeting","label":{"participant_name":name,"text":name},"in_box":i,"crop":crop})
        );
        let mut e = vec![0f32; 128];
        e[i] = 1.;
        values.extend(e.into_iter().flat_map(f32::to_le_bytes));
    }
    fs::write(
        seg.join(format!("{prefix}-test.npy")),
        solstone_core_npy::write_npy("<f4", "(2, 128)", &values),
    )
    .unwrap();
    fs::write(seg.join(format!("{prefix}.jsonl")), body).unwrap();
}
fn rows(root: &Path, id: &str) -> usize {
    faceprints::load(root, id)
        .unwrap()
        .0
        .map_or(0, |a| a.faces.len())
}
fn patch(id: &str) -> Map<String, Value> {
    Map::from_iter([(
        "participation".into(),
        json!([{"name":"Full Person Name","entity_id":id}]),
    )])
}

#[test]
fn participation_writer_reconciles_owner_edit_and_undo_with_principal_absent_from_participation() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fixture(root);
    let _ = crate::append_activity_record(root,"work","20261101",json!({"id":"meeting_090000","activity":"meeting","segments":["090000_300"],"stream":"screen"}).as_object().unwrap().clone()).unwrap();
    assert_eq!(rows(root, "owner"), 1);
    assert_eq!(rows(root, "a"), 0);
    crate::update_activity_record(
        root,
        "work",
        "20261101",
        "meeting_090000",
        &patch("a"),
        "participation",
        "post hook",
        "now",
    )
    .unwrap();
    assert_eq!(rows(root, "a"), 1);
    assert_eq!(rows(root, "owner"), 1);
    crate::update_activity_record_with(
        root,
        "work",
        "20261101",
        "meeting_090000",
        |_| Some(patch("b")),
        "speaker-resolution",
        "resolved",
        "later",
    )
    .unwrap();
    assert_eq!(rows(root, "a"), 0);
    assert_eq!(rows(root, "b"), 1);
    let update = crate::update_activity_record(
        root,
        "work",
        "20261101",
        "meeting_090000",
        &patch("a"),
        "owner:update",
        "choose",
        "later-still",
    )
    .unwrap()
    .unwrap();
    assert_eq!(rows(root, "a"), 1);
    assert_eq!(rows(root, "b"), 0);
    let edit = update["edits"].as_array().unwrap().last().unwrap();
    let pos = update["edits"].as_array().unwrap().len() - 1;
    crate::remove_activity_edit(
        root,
        "work",
        "20261101",
        "meeting_090000",
        pos,
        edit["timestamp"].as_str().unwrap(),
        edit["actor"].as_str().unwrap(),
        "removed",
    )
    .unwrap();
    assert_eq!(rows(root, "a"), 0);
    assert_eq!(rows(root, "b"), 1);
}

#[test]
fn stale_binding_never_fails_publication_and_maintenance_replays_current_assignment() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fixture(root);
    let seg = root.join("chronicle/20261101/screen/090000_300");
    let screen = seg.join("screen.jsonl");
    let original = fs::read(&screen).unwrap();
    let original_text = std::str::from_utf8(&original).unwrap();
    let mut changed: Value = serde_json::from_str(original_text.lines().nth(1).unwrap()).unwrap();
    changed["revision"] = json!(1);
    fs::write(&screen, format!("{{}}\n{changed}\n")).unwrap();
    let _ = crate::append_activity_record(root,"work","20261101",json!({"id":"meeting_090000","activity":"meeting","segments":["090000_300"],"stream":"screen","participation":[{"name":"Tile Alias","entity_id":"a"}]}).as_object().unwrap().clone()).unwrap();
    assert_eq!(rows(root, "a"), 0);
    fs::write(&screen, original).unwrap();
    crate::reconcile_faceprints(root).unwrap();
    assert_eq!(rows(root, "a"), 1);
    assert_eq!(rows(root, "owner"), 1);
    assert_eq!(crate::reconcile_faceprints(root).unwrap(), 0);
    fs::write(
        root.join("config/journal.json"),
        br#"{"faces":{"enabled":false}}"#,
    )
    .unwrap();
    crate::update_activity_record(
        root,
        "work",
        "20261101",
        "meeting_090000",
        &patch("b"),
        "owner:update",
        "choose",
        "now",
    )
    .unwrap();
    assert_eq!(rows(root, "a"), 1);
    assert_eq!(rows(root, "b"), 0);
    fs::write(
        root.join("config/journal.json"),
        br#"{"faces":{"enabled":true}}"#,
    )
    .unwrap();
    crate::reconcile_faceprints(root).unwrap();
    assert_eq!(rows(root, "a"), 0);
    assert_eq!(rows(root, "b"), 1);
}

#[test]
fn real_removal_door_drops_inferred_rows_but_keeps_choice_and_clear_removes_all_face_data() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fixture(root);
    let _ = crate::append_activity_record(root,"work","20261101",json!({"id":"meeting","activity":"meeting","segments":["090000_300"],"stream":"screen","participation":[{"name":"Tile Alias","entity_id":"a"}]}).as_object().unwrap().clone()).unwrap();
    let face = faceprints::load(root, "a").unwrap().0.unwrap().faces[0].clone();
    faceprints::choose(root, "a", &face.metadata.key).unwrap();
    let outcome = solstone_core_retention::door::remove_segments(
        root,
        &[solstone_core_retention::receipt::Target {
            day: "20261101".into(),
            stream: "screen".into(),
            dir: "090000_300".into(),
        }],
        "now",
        solstone_core_retention::RemovalReason::OwnerSegmentDelete,
        "test",
    );
    assert!(
        outcome.targets.iter().all(|t| t.not_removed.is_empty()),
        "{outcome:?}"
    );
    assert_eq!(rows(root, "a"), 0);
    assert_eq!(
        faceprints::confirmed_photo(root, "a").unwrap(),
        Some(vec![1, 2, 3])
    );
    // Replaying the activity cannot restore a removed recording's rows.
    crate::reconcile_faceprints(root).unwrap();
    assert_eq!(rows(root, "a"), 0);
    // Include retained sidecars in a second live segment for the clear operation.
    let other = root.join("chronicle/20261101/screen/100000_300");
    fs::create_dir_all(&other).unwrap();
    fs::write(other.join("screen.jsonl"), b"{}\n").unwrap();
    fs::write(other.join(".faces-v1-synthetic.jsonl"), b"{}").unwrap();
    fs::write(root.join("entities/a/keep.txt"), b"keep").unwrap();
    fs::write(
        root.join("config/journal.json"),
        br#"{"faces":{"enabled":false}}"#,
    )
    .unwrap();
    fs::write(
        root.join("entities/a/.tmp_123_2.tmp"),
        b"private face state",
    )
    .unwrap();
    fs::write(
        other.join(".describe-stage-test-2.tmp"),
        b"private embeddings",
    )
    .unwrap();
    assert!(accumulation::clear(root, solstone_core_removals::Surface::Cli).unwrap() > 0);
    assert!(!root.join("entities/a/.tmp_123_2.tmp").exists());
    assert!(!other.join(".describe-stage-test-2.tmp").exists());
    assert!(root.join("entities/a/keep.txt").is_file());
    assert!(faceprints::load(root, "a").unwrap().0.is_none());
    assert_eq!(faceprints::confirmed_photo(root, "a").unwrap(), None);
    assert!(!other.join(".faces-v1-synthetic.jsonl").exists());
    let history = solstone_core_removals::history(root, 100).unwrap();
    assert!(
        history
            .entries
            .iter()
            .any(|e| e.unit == solstone_core_removals::Unit::Faces)
    );
}

#[test]
fn choosing_a_missing_crop_refuses_without_committing_a_photo() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fixture(root);
    let _ = crate::append_activity_record(root,"work","20261101",json!({"id":"meeting","activity":"meeting","segments":["090000_300"],"stream":"screen","participation":[{"name":"Tile Alias","entity_id":"a"}]}).as_object().unwrap().clone()).unwrap();
    let f = faceprints::load(root, "a").unwrap().0.unwrap().faces[0].clone();
    fs::remove_file(
        faceprints::crop_path(root, &f.metadata.key, f.metadata.crop.as_deref().unwrap()).unwrap(),
    )
    .unwrap();
    assert!(faceprints::choose(root, "a", &f.metadata.key).is_err());
    assert!(
        faceprints::load(root, "a")
            .unwrap()
            .1
            .decision
            .photo
            .is_none()
    );
}

fn publish_meeting(root: &Path) {
    let _ = crate::append_activity_record(root, "work", "20261101", json!({"id":"meeting","activity":"meeting","segments":["090000_300"],"stream":"screen","participation":[{"name":"Tile Alias","entity_id":"a"}]}).as_object().unwrap().clone()).unwrap();
}
fn target() -> solstone_core_retention::receipt::Target {
    solstone_core_retention::receipt::Target {
        day: "20261101".into(),
        stream: "screen".into(),
        dir: "090000_300".into(),
    }
}
#[test]
fn recovery_scan_ignores_merge_backups_and_removes_their_stale_activity_refs() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fixture(root);
    publish_meeting(root);
    let backup = root.join("facets/.facet-merge-test.source/activities");
    fs::create_dir_all(&backup).unwrap();
    fs::copy(
        root.join("facets/work/facet.json"),
        backup.parent().unwrap().join("facet.json"),
    )
    .unwrap();
    fs::rename(
        root.join("facets/work/activities/20261101.jsonl"),
        backup.join("20261101.jsonl"),
    )
    .unwrap();
    crate::reconcile_faceprints(root).unwrap();
    assert_eq!(rows(root, "a"), 0);
    assert_eq!(rows(root, "owner"), 0);
}
#[test]
fn restoring_a_person_after_recording_deletion_prunes_inference_while_disabled() {
    use solstone_core_removals::{Action, Surface, Unit};
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fixture(root);
    publish_meeting(root);
    let face = faceprints::load(root, "a").unwrap().0.unwrap().faces[0].clone();
    let undo = faceprints::choose(root, "a", &face.metadata.key).unwrap();
    let index = solstone_core_retention::notify::NoIndex;
    let entities = crate::entity_trash::Entities::new(&index);
    let person = entities
        .trash(root, "a", Action::EntityDelete, Surface::Cli)
        .unwrap();
    let recordings = solstone_core_retention::recording_trash::Recordings::new(&index);
    recordings
        .trash(
            root,
            &target(),
            Unit::Segment {
                day: "20261101".into(),
                stream: "screen".into(),
                segment: "090000_300".into(),
            },
            Action::SegmentDelete,
            Surface::Cli,
        )
        .unwrap();
    assert_eq!(rows(root, "owner"), 0);
    fs::write(
        root.join("config/journal.json"),
        br#"{"faces":{"enabled":false}}"#,
    )
    .unwrap();
    solstone_core_trash::attach(
        root,
        &[&entities],
        &person.id,
        Action::Recover,
        Surface::Cli,
    )
    .unwrap();
    assert_eq!(rows(root, "a"), 0);
    assert_eq!(
        faceprints::confirmed_photo(root, "a").unwrap(),
        Some(vec![1, 2, 3])
    );
    faceprints::undo(root, "a", &undo).unwrap();
    assert_eq!(faceprints::confirmed_photo(root, "a").unwrap(), None);
}
#[test]
fn clearing_scrubs_trashed_people_and_recordings_before_recovery() {
    use solstone_core_removals::{Action, Surface, Unit};
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fixture(root);
    publish_meeting(root);
    let face = faceprints::load(root, "a").unwrap().0.unwrap().faces[0].clone();
    faceprints::choose(root, "a", &face.metadata.key).unwrap();
    let index = solstone_core_retention::notify::NoIndex;
    let entities = crate::entity_trash::Entities::new(&index);
    fs::write(
        root.join("entities/a/.tmp_123_1.tmp"),
        b"private face state",
    )
    .unwrap();
    fs::write(
        root.join("chronicle/20261101/screen/090000_300/.describe-stage-test-1.tmp"),
        b"private embeddings",
    )
    .unwrap();
    let person = entities
        .trash(root, "a", Action::EntityDelete, Surface::Cli)
        .unwrap();
    let recordings = solstone_core_retention::recording_trash::Recordings::new(&index);
    let recording = recordings
        .trash(
            root,
            &target(),
            Unit::Segment {
                day: "20261101".into(),
                stream: "screen".into(),
                segment: "090000_300".into(),
            },
            Action::SegmentDelete,
            Surface::Cli,
        )
        .unwrap();
    fs::write(
        root.join("config/journal.json"),
        br#"{"faces":{"enabled":false}}"#,
    )
    .unwrap();
    accumulation::clear(root, Surface::Cli).unwrap();
    solstone_core_trash::attach(
        root,
        &[&entities],
        &person.id,
        Action::Recover,
        Surface::Cli,
    )
    .unwrap();
    solstone_core_trash::attach(
        root,
        &[&recordings],
        &recording.id,
        Action::Recover,
        Surface::Cli,
    )
    .unwrap();
    assert!(faceprints::load(root, "a").unwrap().0.is_none());
    assert_eq!(faceprints::confirmed_photo(root, "a").unwrap(), None);
    assert!(!root.join("entities/a/.tmp_123_1.tmp").exists());
    assert!(
        !root
            .join("chronicle/20261101/screen/090000_300/.describe-stage-test-1.tmp")
            .exists()
    );
    for entry in fs::read_dir(root.join("chronicle/20261101/screen/090000_300")).unwrap() {
        let entry = entry.unwrap();
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with(".faces-v1-")
        {
            // Empty retained crop directories contain no face material.
            if entry.file_type().unwrap().is_dir() {
                assert!(fs::read_dir(entry.path()).unwrap().next().is_none());
            } else {
                // Permanent coordination sidecars contain no face payload.
                assert!(entry.file_name().to_string_lossy().ends_with(".lock"));
                assert_eq!(entry.metadata().unwrap().len(), 0);
            }
        }
    }
}
#[test]
fn clear_fences_first_publication_and_refuses_busy_trash_before_deleting() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fixture(root);
    publish_meeting(root);
    let other = root.join("chronicle/20261101/screen/100000_300");
    fs::create_dir_all(&other).unwrap();
    fs::write(other.join("screen.jsonl"), b"{}\n").unwrap();
    let busy = solstone_core_segment::output_attempt::OutputClaims::discover(
        &other,
        solstone_core_journal_io::LockOptions::default(),
    )
    .unwrap();
    fs::write(
        root.join("config/journal.json"),
        br#"{"faces":{"enabled":false}}"#,
    )
    .unwrap();
    let busy_trash = solstone_core_trash::hold_payload_scrub(root).unwrap();
    assert!(accumulation::clear(root, solstone_core_removals::Surface::Cli).is_err());
    assert_eq!(rows(root, "a"), 1);
    drop(busy_trash);
    assert!(accumulation::clear(root, solstone_core_removals::Surface::Cli).is_err());
    drop(busy);
    accumulation::clear(root, solstone_core_removals::Surface::Cli).unwrap();
    assert!(faceprints::load(root, "a").unwrap().0.is_none());
}
#[test]
fn deletion_cleanup_refuses_coordinator_and_raw_lock_contention_even_before_first_archive() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fixture(root);
    let (ready, wait) = std::sync::mpsc::channel();
    let (release, done) = std::sync::mpsc::channel();
    let other_root = root.to_owned();
    let worker = std::thread::spawn(move || {
        let _trust = solstone_core_entity::hold_entity_trust_lock(&other_root).unwrap();
        ready.send(()).unwrap();
        let _ = done.recv_timeout(std::time::Duration::from_secs(5));
    });
    wait.recv().unwrap();
    let before = std::time::Instant::now();
    assert!(faceprints::remove_segment_rows(root, "chronicle/20261101/screen/090000_300").is_err());
    assert!(before.elapsed() < std::time::Duration::from_secs(1));
    release.send(()).unwrap();
    worker.join().unwrap();
    let _raw = solstone_core_entity::hold_entity_trust_lock_raw_for_test(root).unwrap();
    assert!(faceprints::remove_segment_rows(root, "chronicle/20261101/screen/090000_300").is_err());
}

#[test]
fn unresolved_merge_recovery_refuses_recording_deletion_before_any_source_moves() {
    use solstone_core_removals::{Action, Surface, Unit};
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fixture(root);
    publish_meeting(root);
    let recovery = root.join("health/entity-merge-recovery");
    fs::create_dir_all(&recovery).unwrap();
    fs::write(recovery.join("state.json"), b"invalid").unwrap();
    let recordings = solstone_core_retention::recording_trash::Recordings::new(
        &solstone_core_retention::notify::NoIndex,
    );
    assert!(
        recordings
            .trash(
                root,
                &target(),
                Unit::Segment {
                    day: "20261101".into(),
                    stream: "screen".into(),
                    segment: "090000_300".into()
                },
                Action::SegmentDelete,
                Surface::Cli
            )
            .is_err()
    );
    assert!(
        root.join("chronicle/20261101/screen/090000_300/screen.jsonl")
            .is_file()
    );
    assert_eq!(rows(root, "a"), 1);
    let direct = solstone_core_retention::door::remove_segments(
        root,
        &[target()],
        "now",
        solstone_core_retention::RemovalReason::OwnerSegmentDelete,
        "test",
    );
    assert!(direct.targets.iter().any(|r| !r.not_removed.is_empty()));
    assert!(
        root.join("chronicle/20261101/screen/090000_300/screen.jsonl")
            .is_file()
    );
}
