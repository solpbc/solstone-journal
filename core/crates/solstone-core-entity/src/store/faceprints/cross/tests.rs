// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use super::*;
fn s(id: &str, name: &str, support: usize, cos: f32) -> Summary {
    Summary {
        id: id.into(),
        names: vec![name.into()],
        anchored: support,
        centroid: vec![cos, (1. - cos * cos).max(0.).sqrt()],
    }
}
#[test]
fn compatible_names_cover_each_rule_and_word_boundaries() {
    for (a, b) in [
        ("Casey Lane", "casey lane"),
        ("Casey", "Casey Lane"),
        ("Marianne", "Marianne K"),
        ("Casey Lane", "Kasey Lane"),
        ("abcdefghijklmnopqrst", "abcdefghijklmnopqxyz"),
        ("Jon North", "Jonathan North"),
        ("Casey Lane (Guest)|Company", "casey lane"),
    ] {
        assert!(compatible(a, b), "{a}/{b}");
        assert!(compatible(b, a));
    }
    for (a, b) in [
        ("Case", "Casey Lane"),
        ("Jon North", "Jane South"),
        ("abcdefghijklmnopqrst", "abcdefghijklmnopyzvw"),
        ("", ""),
        ("...", "?"),
    ] {
        assert!(!compatible(a, b), "{a}/{b}");
    }
    assert!(names_compatible(
        &["unrelated".into(), "Jon North".into()],
        &["Jonathan North".into()]
    ));
}
#[test]
fn exclusivity_requires_cosine_support_ratio_and_incompatible_names() {
    let a = s("a", "Casey Lane", 2, 1.);
    assert!(exclusivity_drop(&a, &[s("b", "Avery West", 6, 0.50)]));
    assert!(!exclusivity_drop(&a, &[s("b", "Avery West", 5, 0.9)]));
    assert!(!exclusivity_drop(&a, &[s("b", "Avery West", 6, 0.499)]));
    assert!(!exclusivity_drop(&a, &[s("b", "Casey", 6, 0.9)]));
    assert!(!exclusivity_drop(&a, std::slice::from_ref(&a)));
}
#[test]
fn contested_hides_both_and_compatible_merge_uses_its_own_threshold() {
    let a = s("a", "Casey Lane", 4, 1.);
    let b = s("b", "Avery West", 4, 0.80);
    let r = rules(&[a.clone(), b.clone()], &[a.clone(), b]);
    assert_eq!(r.contested_pairs.len(), 1);
    assert!(r.entities["a"].contested && r.entities["b"].contested);
    for (cos, count) in [(0.55, 1), (0.549, 0)] {
        let b = s("b", "Casey", 4, cos);
        let r = rules(&[a.clone(), b.clone()], &[a.clone(), b]);
        assert_eq!(r.merge_pairs.len(), count);
        assert!(r.contested_pairs.is_empty());
    }
    let b = s("b", "Avery West", 4, 0.799);
    assert!(
        rules(&[a.clone(), b.clone()], &[a, b])
            .contested_pairs
            .is_empty()
    );
}
#[test]
fn a_dropped_panel_cannot_contest_a_surviving_panel() {
    let a = s("a", "Casey Lane", 2, 1.);
    let b = s("b", "Avery West", 6, 0.9);
    let r = rules(&[a.clone(), b.clone()], &[a, b]);
    assert!(r.entities["a"].exclusivity_drop);
    assert!(!r.entities["b"].contested);
    assert!(r.contested_pairs.is_empty());
}

#[cfg(all(test, feature = "full-tests"))]
mod disk {
    // Synthetic journal before-images are fixtures, not entity mutations.
    #![allow(clippy::disallowed_methods)]
    use super::*;
    use serde_json::json;
    #[test]
    fn daily_reconcile_is_idempotent_and_clearing_preserves_other_suggestions() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        std::fs::create_dir_all(root.join("config")).unwrap();
        std::fs::write(
            root.join("config").join("journal.json"),
            json!({"faces":{"enabled":true}}).to_string(),
        )
        .unwrap();
        for (id, name) in [("a", "Casey Lane"), ("b", "Casey"), ("c", "Avery West")] {
            crate::save_entity_identity(
                root,
                id,
                &json!({"id":id,"name":name,"type":"Person"}),
                None,
            )
            .unwrap();
        }
        let coordinate = serde_json::to_string(&["work", "20261101", "meeting"]).unwrap();
        let encoder = crate::faceprints::Encoder {
            id: "synthetic".into(),
            sha256: "0".repeat(64),
            width: crate::faceprints::WIDTH,
        };
        let rows: Vec<_> = (0..5).map(|i| observation(i, "work", "20261101")).collect();
        crate::faceprints::reconcile(
            root,
            &coordinate,
            &encoder,
            &BTreeMap::from([("a".into(), rows.clone()), ("b".into(), rows)]),
        )
        .unwrap();
        let rules = reconcile(root).unwrap();
        assert_eq!(rules.merge_pairs.len(), 1);
        let proposals = crate::load_merge_candidates(root, Some("work"), Some("open")).unwrap();
        assert_eq!(proposals.len(), 1);
        let before =
            std::fs::read(crate::store::paths::review_candidates_path(root).unwrap()).unwrap();
        let derived = std::fs::read(path(root).unwrap()).unwrap();
        reconcile(root).unwrap();
        assert_eq!(
            std::fs::read(crate::store::paths::review_candidates_path(root).unwrap()).unwrap(),
            before
        );
        assert_eq!(std::fs::read(path(root).unwrap()).unwrap(), derived);
        crate::dismiss_merge_candidate(root, "work", "a", "b").unwrap();
        reconcile(root).unwrap();
        assert!(
            crate::load_merge_candidates(root, None, Some("open"))
                .unwrap()
                .is_empty()
        );
        crate::record_merge_candidate(
            root,
            "work",
            "20261101",
            "Casey Lane",
            "a",
            "Avery West",
            "c",
            "synthetic unrelated suggestion",
            None,
            None,
            None,
        )
        .unwrap();
        std::fs::write(
            root.join("config").join("journal.json"),
            json!({"faces":{"enabled":false}}).to_string(),
        )
        .unwrap();
        crate::faceprints::accumulation::clear(root, solstone_core_removals::Surface::Cli).unwrap();
        let left = crate::load_merge_candidates(root, None, None).unwrap();
        assert_eq!(left.len(), 1);
        assert_ne!(left[0]["evidence"]["basis"], MERGE_BASIS);
    }
    #[test]
    fn disabled_activity_hook_never_builds_a_census() {
        use crate::store::test_counters::test_instrumentation as counters;
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        crate::save_entity_identity(
            root,
            "a",
            &json!({"id":"a","name":"Synthetic Person","type":"Person"}),
            None,
        )
        .unwrap();
        std::fs::create_dir_all(root.join("config")).unwrap();
        let record = json!({"id":"meeting","activity":"meeting","segments":[]});
        for enabled in [true, false] {
            std::fs::write(
                root.join("config").join("journal.json"),
                json!({"faces":{"enabled":enabled}}).to_string(),
            )
            .unwrap();
            counters::reset_test_counters();
            crate::faceprints::accumulation::reconcile_activity(
                root,
                "work",
                "20261101",
                record.as_object().unwrap(),
            )
            .unwrap();
            if enabled {
                assert!(counters::entities_dir_listing_count() > 0);
            } else {
                assert_eq!(counters::entities_dir_listing_count(), 0);
                assert_eq!(counters::identity_read_count(), 0);
            }
        }
    }
    #[test]
    fn reads_do_not_create_and_clear_erases_cross_state_and_skips() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        assert_eq!(load(root).unwrap(), Derived::default());
        assert!(!root.join("entities").exists());
        std::fs::create_dir_all(root.join("config")).unwrap();
        std::fs::write(
            root.join("config").join("journal.json"),
            json!({"faces":{"enabled":true}}).to_string(),
        )
        .unwrap();
        let backfill = Backfill {
            cursor: Some("chronicle/20260101/screen.webm".into()),
            unsupported: BTreeMap::from([("raw".into(), "binding".into())]),
        };
        update_backfill(root, backfill.clone()).unwrap();
        assert_eq!(load(root).unwrap().backfill, backfill);
        std::fs::write(
            root.join("config").join("journal.json"),
            json!({"faces":{"enabled":false}}).to_string(),
        )
        .unwrap();
        super::super::super::accumulation::clear(root, solstone_core_removals::Surface::Cli)
            .unwrap();
        assert_eq!(load(root).unwrap(), Derived::default());
        update_backfill(root, backfill).unwrap();
        assert!(!path(root).unwrap().exists());
    }
    #[cfg(unix)]
    #[test]
    fn clear_accepts_a_journal_alias_with_canonical_derived_paths() {
        let t = tempfile::tempdir().unwrap();
        let actual = t.path().join("actual");
        let alias = t.path().join("alias");
        std::fs::create_dir_all(actual.join("config")).unwrap();
        std::os::unix::fs::symlink(&actual, &alias).unwrap();
        std::fs::write(
            actual.join("config").join("journal.json"),
            json!({"faces":{"enabled":true}}).to_string(),
        )
        .unwrap();
        update_backfill(&alias, Backfill::default()).unwrap();
        let derived = path(&alias).unwrap();
        assert!(!derived.starts_with(&alias));
        assert!(derived.exists());
        std::fs::write(
            actual.join("config").join("journal.json"),
            json!({"faces":{"enabled":false}}).to_string(),
        )
        .unwrap();
        super::super::super::accumulation::clear(&alias, solstone_core_removals::Surface::Cli)
            .unwrap();
        assert!(!derived.exists());
    }
    #[cfg(unix)]
    fn test_link(target: &Path, link: &Path, _directory: bool) {
        std::os::unix::fs::symlink(target, link).unwrap();
    }
    #[cfg(windows)]
    fn test_link(target: &Path, link: &Path, directory: bool) {
        if directory {
            let output = std::process::Command::new("cmd")
                .args(["/d", "/c", "mklink", "/J"])
                .arg(link)
                .arg(target)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "junction fixture failed: {output:?}"
            );
        } else {
            std::os::windows::fs::symlink_file(target, link).unwrap();
        }
    }
    #[cfg(any(unix, windows))]
    fn refuse_linked_entry(directory: bool) {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        std::fs::create_dir_all(root.join("config")).unwrap();
        std::fs::create_dir_all(root.join("entities")).unwrap();
        std::fs::write(
            root.join("config").join("journal.json"),
            json!({"faces":{"enabled":false}}).to_string(),
        )
        .unwrap();
        let target = root.join("protected");
        let payload = if directory {
            std::fs::create_dir(&target).unwrap();
            target.join("record")
        } else {
            target.clone()
        };
        std::fs::write(&payload, b"keep this owner data").unwrap();
        let link = root.join("entities").join(FILE);
        test_link(&target, &link, directory);
        let result =
            super::super::super::accumulation::clear(root, solstone_core_removals::Surface::Cli);
        assert!(payload.exists(), "clear deleted the linked target");
        assert!(result.is_err(), "clear followed a linked face entry");
        assert_eq!(std::fs::read(payload).unwrap(), b"keep this owner data");
        assert!(std::fs::symlink_metadata(link).is_ok());
    }
    #[cfg(any(unix, windows))]
    #[test]
    fn clear_refuses_a_file_symlink_and_preserves_its_target() {
        refuse_linked_entry(false);
    }
    #[cfg(any(unix, windows))]
    #[test]
    fn clear_refuses_a_directory_symlink_and_preserves_its_target() {
        refuse_linked_entry(true);
    }
    #[cfg(any(unix, windows))]
    #[test]
    fn clear_refuses_a_linked_namespace_before_recovery() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        std::fs::create_dir_all(root.join("config")).unwrap();
        std::fs::write(
            root.join("config").join("journal.json"),
            json!({"faces":{"enabled":false}}).to_string(),
        )
        .unwrap();
        let target = root.join("protected");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join(FILE), b"keep this owner data").unwrap();
        test_link(&target, &root.join("entities"), true);
        assert!(
            super::super::super::accumulation::clear(root, solstone_core_removals::Surface::Cli)
                .is_err()
        );
        assert_eq!(
            std::fs::read(target.join(FILE)).unwrap(),
            b"keep this owner data"
        );
    }
}

fn observation(index: usize, facet: &str, day: &str) -> Face {
    let mut embedding = vec![0.; super::super::WIDTH];
    embedding[0] = 0.8;
    embedding[index + 1] = 0.6;
    serde_json::from_value(serde_json::json!({"embedding":embedding,"metadata":{
      "schema_version":1,"day":day,"stream":"_default","stream_layout":"direct","segment_key":format!("{index:06}_300"),"source_name":"screen.webm","frame_id":index,"face_idx":0,"activities":[serde_json::to_string(&[facet,day,"meeting"]).unwrap()],"source":"anchored","label_text":null,"quality":0.8,"det_score":1.,"face_w":100.,"category":"meeting","crop":null,"added_at":"now","rejected":false
    }})).unwrap()
}
#[test]
fn shared_facet_uses_both_peoples_evidence_with_a_stable_newest_choice() {
    let a = [
        observation(0, "work", "20261101"),
        observation(1, "other", "20261103"),
    ];
    let b = [
        observation(0, "work", "20261102"),
        observation(1, "private", "20261104"),
    ];
    assert_eq!(
        shared_facet(&a.iter().collect::<Vec<_>>(), &b.iter().collect::<Vec<_>>()),
        Some(("work".into(), "20261101".into()))
    );
    assert_eq!(shared_facet(&[&a[1]], &[&b[1]]), None);
}
#[test]
fn only_open_face_proposals_are_withdrawn_when_their_evidence_disappears() {
    let mut row = serde_json::json!({"status":"open","evidence":{"basis":MERGE_BASIS},"facet":"work","source_slug":"a","target_slug":"b"});
    assert!(is_stale_proposal(&row, &[]));
    let (a, b) = ("a".to_string(), "b".to_string());
    assert!(!is_stale_proposal(
        &row,
        &[(&a, &b, "work".into(), "20261101".into())]
    ));
    row["status"] = serde_json::json!("dismissed");
    assert!(!is_stale_proposal(&row, &[]));
    row["status"] = serde_json::json!("open");
    row["evidence"]["basis"] = serde_json::json!("name-variant");
    assert!(!is_stale_proposal(&row, &[]));
}
