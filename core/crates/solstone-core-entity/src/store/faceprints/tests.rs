// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
#![allow(clippy::disallowed_methods, clippy::unwrap_used)]
use super::*;
use panel::{CrossEntity, State, panel};

fn encoder() -> Encoder {
    Encoder {
        id: "synthetic".into(),
        sha256: "0".repeat(64),
        width: WIDTH,
    }
}
fn face(i: usize) -> Face {
    let mut embedding = vec![0.; WIDTH];
    embedding[0] = 0.8;
    embedding[i + 1] = 0.6;
    Face {
        embedding,
        metadata: Metadata {
            schema_version: 1,
            key: FaceKey {
                day: "20261101".into(),
                stream: "screen".into(),
                stream_layout: "named".into(),
                segment_key: format!("{i:06}_300"),
                source_name: "screen.webm".into(),
                frame_id: i as u64,
                face_idx: 0,
            },
            activities: BTreeSet::from(["activity".into()]),
            source: Source::Anchored,
            label_text: None,
            quality: i as f32 / 10.,
            det_score: 1.,
            face_w: 100.,
            category: "meeting".into(),
            crop: None,
            added_at: "now".into(),
            rejected: false,
        },
    }
}
fn pool(n: usize) -> Vec<Face> {
    (0..n).map(face).collect()
}
fn run(f: &[Face]) -> panel::Panel {
    panel(f, &FacesState::default(), false, None, None)
}

#[test]
fn support_floor_requires_three_rows_two_segments_and_two_anchors() {
    assert_eq!(run(&pool(2)).state, State::Hidden);
    let mut f = pool(3);
    for v in &mut f {
        v.metadata.key.segment_key = "one".into();
    }
    assert_eq!(run(&f).state, State::Hidden);
    let mut f = pool(3);
    f[1].metadata.source = Source::Box;
    f[2].metadata.source = Source::Box;
    assert_eq!(run(&f).state, State::Hidden);
    f[1].metadata.source = Source::Anchored;
    assert_eq!(run(&f).state, State::Possible);
}
#[test]
fn state_split_and_quality_default_use_the_look_count() {
    assert_eq!(run(&pool(3)).state, State::Possible);
    assert_eq!(run(&pool(4)).state, State::Possible);
    let f = pool(5);
    let p = run(&f);
    assert_eq!(p.state, State::Suggested);
    assert_eq!(p.default, Some(f[4].metadata.key.clone()));
}
#[test]
fn looks_collapse_only_across_segments_and_rejection_removes_similar_rows() {
    let mut f = pool(3);
    f[1].embedding = f[0].embedding.clone();
    assert_eq!(run(&f).state, State::Hidden);
    f[1].metadata.key.segment_key = f[0].metadata.key.segment_key.clone();
    assert_eq!(run(&f).state, State::Possible);
    let mut f = pool(5);
    f[4].embedding = f[0].embedding.clone();
    f[4].metadata.rejected = true;
    let p = run(&f);
    assert_eq!(p.state, State::Possible);
    assert!(!p.faces.contains(&f[0].metadata.key));
}
#[test]
fn owner_exclusion_does_not_exclude_the_principal_and_centroid_uses_anchor_pool() {
    let f = pool(5);
    let mut owner = vec![0.; WIDTH];
    owner[0] = 1.;
    assert_eq!(
        panel(&f, &FacesState::default(), false, Some(&owner), None).state,
        State::Hidden
    );
    assert_eq!(
        panel(&f, &FacesState::default(), true, Some(&owner), None).state,
        State::Suggested
    );
    let mut f = pool(4);
    f[3].metadata.source = Source::Box;
    f[3].embedding[0] = -0.8;
    let c = panel::owner_centroid(&f).unwrap();
    let expected = panel::centroid(&f[..3].iter().collect::<Vec<_>>()).unwrap();
    assert_eq!(c, expected);
    for f in &mut f {
        f.metadata.source = Source::Box;
    }
    assert!(panel::owner_centroid(&f).is_some());
}
#[test]
fn ranking_takes_distinct_segments_before_filling_and_stops_at_six() {
    let mut f = pool(8);
    f[7].metadata.key.segment_key = f[6].metadata.key.segment_key.clone();
    let p = run(&f);
    assert_eq!(p.faces.len(), 6);
    assert_eq!(p.faces[0], f[7].metadata.key);
    assert!(!p.faces.contains(&f[6].metadata.key));
}
#[test]
fn confirmed_anchor_beats_a_larger_other_cluster_and_survives_empty_evidence() {
    let mut f = pool(3);
    for i in 3..8 {
        let mut other = face(i);
        other.embedding[0] = -0.8;
        f.push(other);
    }
    let chosen = ChosenPhoto {
        face: f[0].clone(),
        encoder: encoder(),
        chosen_at: "now".into(),
        jpeg: vec![1, 2, 3],
    };
    let mut s = FacesState::default();
    s.decision.photo = Some(chosen);
    let p = panel(&f, &s, false, None, None);
    assert_eq!(p.state, State::Confirmed);
    assert!(p.faces.iter().all(|k| k.frame_id < 3));
    let p = panel(
        &[],
        &s,
        false,
        None,
        Some(CrossEntity {
            contested: true,
            exclusivity_drop: true,
        }),
    );
    assert_eq!(p.state, State::Confirmed);
    assert!(p.default.is_some());
}
#[test]
fn cross_entity_input_hides_unconfirmed_panels() {
    for c in [
        CrossEntity {
            contested: true,
            exclusivity_drop: false,
        },
        CrossEntity {
            contested: false,
            exclusivity_drop: true,
        },
    ] {
        assert_eq!(
            panel(&pool(5), &FacesState::default(), false, None, Some(c)).state,
            State::Hidden
        );
    }
}
#[test]
fn dismissal_waits_for_a_new_look_instead_of_a_new_observation() {
    let mut f = pool(5);
    let mut s = FacesState::default();
    s.decision.dismissed_at = Some("now".into());
    s.decision.dismissed_looks = f.iter().map(|f| f.embedding.clone()).collect();
    assert_eq!(panel(&f, &s, false, None, None).state, State::Dismissed);
    assert_eq!(panel(&[], &s, false, None, None).state, State::Dismissed);
    assert_eq!(
        panel(&f[..2], &s, false, None, None).state,
        State::Dismissed
    );
    let mut duplicate = f[0].clone();
    duplicate.metadata.key.frame_id = 100;
    f.push(duplicate);
    assert_eq!(panel(&f, &s, false, None, None).state, State::Dismissed);
    f.push(face(6));
    assert_eq!(panel(&f, &s, false, None, None).state, State::Suggested);
}
#[test]
fn mapping_uses_resolved_participation_and_the_absent_principal_but_refuses_ambiguity() {
    let person = |id: &str, name: &str, principal: bool| crate::JournalEntity {
        id: id.into(),
        value: serde_json::json!({"type":"Person","name":name,"is_principal":principal,"aka":["tile alias"]}),
    };
    let mut people = vec![
        person("owner", "Synthetic Owner", true),
        person("a", "Person A", false),
    ];
    let participation = vec![serde_json::json!({"name":"Meeting Name","entity_id":"a"})];
    assert_eq!(
        accumulation::participant_entity("synthetic owner (host)", &participation, &people),
        Some("owner".into())
    );
    assert_eq!(
        accumulation::participant_entity("meeting name", &participation, &people),
        Some("a".into())
    );
    assert_eq!(
        accumulation::participant_entity("tile alias", &participation, &people),
        None
    );
    people[0].value["aka"] = serde_json::json!([]);
    assert_eq!(
        accumulation::participant_entity("tile alias", &participation, &people),
        Some("a".into())
    );
    assert_eq!(
        accumulation::participant_entity("Person A", &[], &people),
        None
    );
}
#[test]
fn codec_is_self_describing_and_refuses_mismatched_shapes_or_duplicate_keys() {
    let a = Archive {
        envelope: Archive::empty(encoder()).envelope,
        faces: pool(3),
    };
    assert_eq!(codec::read(&codec::write(&a).unwrap()).unwrap(), a);
    let mut bad = a.clone();
    bad.envelope.encoder.width = 256;
    assert!(codec::write(&bad).is_err());
    let mut bad = a.clone();
    bad.faces.push(bad.faces[0].clone());
    assert!(codec::write(&bad).is_err());
    let mut bad = a;
    bad.faces[0].embedding[0] = f32::NAN;
    assert!(codec::write(&bad).is_err());
}

#[cfg(feature = "full-tests")]
mod disk {
    use super::*;
    use std::{fs, path::Path};
    fn entity(root: &Path, id: &str) {
        let dir = root.join("entities").join(id);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("entity.json"),
            serde_json::to_vec(&serde_json::json!({"id":id,"name":id,"type":"Person"})).unwrap(),
        )
        .unwrap();
    }
    fn publish(dir: &Path, name: &str, bytes: &[u8]) -> Result<()> {
        let binding = solstone_core_journal_io::JournalRoot::open(dir).unwrap();
        storage::publish(dir, &binding, name, bytes)
    }
    fn save(root: &Path, id: &str, faces: Vec<Face>) {
        let _trust = crate::hold_entity_trust_lock(root).unwrap();
        let dir = root.join("entities").join(id);
        let binding = solstone_core_journal_io::JournalRoot::open(&dir).unwrap();
        storage::save_archive(
            &dir,
            &binding,
            &Archive {
                envelope: Archive::empty(encoder()).envelope,
                faces,
            },
        )
        .unwrap();
    }
    fn choose_fixture(root: &Path) -> String {
        entity(root, "a");
        let mut f = face(0);
        let crop = format!(
            "{}-crops/synthetic.jpg",
            accumulation::source_prefix(&f.metadata.key.source_name)
        );
        let path = crop_path(root, &f.metadata.key, &crop).unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"synthetic jpeg").unwrap();
        f.metadata.crop = Some(crop);
        let key = f.metadata.key.clone();
        save(root, "a", vec![f]);
        choose(root, "a", &key).unwrap()
    }
    #[test]
    fn dismiss_after_choose_removes_photo_and_undo_restores_it() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        choose_fixture(root);
        let photo = root.join("entities/a").join(PHOTO);
        assert_eq!(fs::read(&photo).unwrap(), b"synthetic jpeg");
        let token = dismiss(root, "a").unwrap();
        assert!(load(root, "a").unwrap().1.decision.photo.is_none());
        assert!(!photo.exists());
        assert_eq!(confirmed_photo(root, "a").unwrap(), None);
        undo(root, "a", &token).unwrap();
        assert_eq!(fs::read(&photo).unwrap(), b"synthetic jpeg");
        assert_eq!(
            confirmed_photo(root, "a").unwrap(),
            Some(b"synthetic jpeg".to_vec())
        );
    }
    #[test]
    fn undo_choose_to_none_removes_photo() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let token = choose_fixture(root);
        let photo = root.join("entities/a").join(PHOTO);
        assert!(photo.is_file());
        undo(root, "a", &token).unwrap();
        assert!(load(root, "a").unwrap().1.decision.photo.is_none());
        assert!(!photo.exists());
        assert_eq!(confirmed_photo(root, "a").unwrap(), None);
    }
    #[test]
    fn confirmed_photo_repairs_stale_copy_with_no_choice_or_state() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        entity(root, "a");
        let dir = root.join("entities/a");
        for write_state in [false, true] {
            if write_state {
                publish(
                    &dir,
                    STATE,
                    &serde_json::to_vec(&FacesState::default()).unwrap(),
                )
                .unwrap();
            }
            fs::write(dir.join(PHOTO), b"stale jpeg").unwrap();
            assert_eq!(confirmed_photo(root, "a").unwrap(), None);
            assert!(!dir.join(PHOTO).exists());
        }
    }
    #[test]
    fn reconciliation_refreshes_evidence_preserves_rejection_and_shared_activity_references() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        entity(root, "a");
        entity(root, "b");
        let f = face(0);
        let key = f.metadata.key.clone();
        let want = std::collections::BTreeMap::from([("a".into(), vec![f.clone()])]);
        assert_eq!(reconcile(root, "one", &encoder(), &want).unwrap(), 1);
        assert_eq!(reconcile(root, "one", &encoder(), &want).unwrap(), 0);
        reconcile(root, "two", &encoder(), &want).unwrap();
        let token = reject(root, "a", &key).unwrap();
        let mut newer = f;
        newer.metadata.crop = Some("new.jpg".into());
        reconcile(
            root,
            "one",
            &encoder(),
            &std::collections::BTreeMap::from([("b".into(), vec![newer.clone()])]),
        )
        .unwrap();
        assert_eq!(load(root, "a").unwrap().0.unwrap().faces.len(), 1);
        reconcile(root, "two", &encoder(), &std::collections::BTreeMap::new()).unwrap();
        assert!(load(root, "a").unwrap().0.unwrap().faces.is_empty());
        reconcile(
            root,
            "one",
            &encoder(),
            &std::collections::BTreeMap::from([("a".into(), vec![newer])]),
        )
        .unwrap();
        let a = load(root, "a").unwrap().0.unwrap();
        assert!(a.faces[0].metadata.rejected);
        assert_eq!(a.faces[0].metadata.crop.as_deref(), Some("new.jpg"));
        undo(root, "a", &token).unwrap();
        assert!(
            !load(root, "a").unwrap().0.unwrap().faces[0]
                .metadata
                .rejected
        );
        let mut wrong = encoder();
        wrong.id = "other".into();
        assert!(reconcile(root, "one", &wrong, &want).is_err());
    }
    #[test]
    fn removal_distinguishes_stream_layout_and_preserves_confirmed_state_and_undo() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        entity(root, "a");
        let mut f = face(0);
        f.metadata.key.stream = "_default".into();
        let mut direct = f.clone();
        direct.metadata.key.stream_layout = "direct".into();
        save(root, "a", vec![f.clone(), direct.clone()]);
        let chosen = |face: Face, jpeg: Vec<u8>| ChosenPhoto {
            face,
            encoder: encoder(),
            chosen_at: "now".into(),
            jpeg,
        };
        let mut state = FacesState::default();
        state.decision.photo = Some(chosen(f, vec![1]));
        state.last = Some(Undo {
            token: "last".into(),
            decision: Decision {
                photo: Some(chosen(direct.clone(), vec![2])),
                ..Decision::default()
            },
        });
        publish(
            &root.join("entities/a"),
            STATE,
            &serde_json::to_vec(&state).unwrap(),
        )
        .unwrap();
        assert_eq!(
            remove_segment_rows(root, &segment_relative(&direct.metadata.key).unwrap()).unwrap(),
            1
        );
        assert_eq!(load(root, "a").unwrap().0.unwrap().faces.len(), 1);
        assert_eq!(confirmed_photo(root, "a").unwrap(), Some(vec![1]));
        undo(root, "a", "last").unwrap();
        assert_eq!(confirmed_photo(root, "a").unwrap(), Some(vec![2]));
    }
    #[test]
    fn clear_settles_merge_before_images_before_erasing_face_material() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        entity(root, "a");
        save(root, "a", pool(3));
        let _trust = crate::hold_entity_trust_lock(root).unwrap();
        let mut rollback = crate::store::merge_rollback::MergeRollback::begin(root).unwrap();
        rollback.capture(root, "entities/a").unwrap();
        rollback.checkpoint(root).unwrap();
        drop(rollback);
        assert!(crate::entity_merge_recovery_pending(root));
        accumulation::clear(root, solstone_core_removals::Surface::Cli).unwrap();
        assert!(!crate::entity_merge_recovery_pending(root));
        crate::recover_interrupted_entity_merge(root).unwrap();
        assert!(load(root, "a").unwrap().0.is_none());
        assert!(root.join("entities/a/entity.json").is_file());
    }
    #[test]
    fn merge_carries_rows_rejection_photo_and_rolls_back_the_new_phase() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        entity(root, "source");
        entity(root, "target");
        let mut f = face(0);
        f.metadata.rejected = true;
        save(root, "source", vec![f.clone()]);
        save(root, "target", pool(2));
        let mut state = FacesState::default();
        state.decision.photo = Some(ChosenPhoto {
            face: f,
            encoder: encoder(),
            chosen_at: "now".into(),
            jpeg: vec![7, 8],
        });
        publish(
            &root.join("entities/source"),
            STATE,
            &serde_json::to_vec(&state).unwrap(),
        )
        .unwrap();
        let voice = crate::EncoderIdentity {
            id: "test".into(),
            sha256: "0".repeat(64),
            width: 256,
        };
        let before = fs::read(root.join("entities/target").join(ARCHIVE)).unwrap();
        let result = crate::store::merge::commit_entity_merge_with_injector(
            root,
            "source",
            "target",
            crate::EntityMergeOptions::default(),
            &voice,
            Some(&|phase, _| phase == "faceprints"),
        );
        assert!(result.is_err());
        assert_eq!(
            fs::read(root.join("entities/target").join(ARCHIVE)).unwrap(),
            before
        );
        assert!(root.join("entities/source").exists());
        crate::commit_entity_merge(
            root,
            "source",
            "target",
            crate::EntityMergeOptions::default(),
            &voice,
        )
        .unwrap();
        assert!(!root.join("entities/source").exists());
        let (a, s) = load(root, "target").unwrap();
        assert_eq!(a.unwrap().faces.len(), 2);
        assert!(s.decision.photo.is_some());
        assert_eq!(confirmed_photo(root, "target").unwrap(), Some(vec![7, 8]));
    }
}
