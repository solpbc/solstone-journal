// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::*;
use crate::body_segments::tests::facet_tests::entries;

const OTHER: &str = "123e4567-e89b-42d3-a456-426614174000";
const HEALTH: &str = "223e4567-e89b-42d3-a456-426614174000";

fn facets(root: &Path) {
    for (name, id) in [("other", OTHER), ("health", HEALTH)] {
        fs::write(
            root.join(format!("facets/{name}/facet.json")),
            json!({
                "id": id, "title": name, "description": name
            })
            .to_string(),
        )
        .unwrap();
    }
}

fn unfiled_tile_fixture(repeat: bool) -> (tempfile::TempDir, tempfile::TempDir, PathBuf, String) {
    let journal = journal();
    let root = journal.path();
    facets(root);
    let scratch = tempfile::tempdir().unwrap();
    let first = stage(
        &scratch.path().join("first"),
        &[(
            "workout",
            vec![workout(
                "private",
                "running",
                "2026-01-05T06:00:00-07:00",
                "2026-01-05T06:15:00-07:00",
            )],
        )],
    );
    let original = run(root, &first, "20261104_120000", Some("other"));
    // Independently construct the legacy state: the parent was filed, but a
    // no-facet extension left its later tile unfiled beside another workout.
    fs::remove_file(window(root, "20260105", "061000_300").join("talents/facets.json")).unwrap();
    let unrelated = workout(
        "unfiled",
        "walking",
        "2026-01-05T06:11:00-07:00",
        "2026-01-05T06:14:00-07:00",
    );
    let unfiled = stage(
        &scratch.path().join("unfiled"),
        &[("workout", vec![unrelated.clone()])],
    );
    run(root, &unfiled, "20261104_130000", None);
    let mut workouts = vec![
        workout(
            "private",
            "running",
            "2026-01-05T06:00:00-07:00",
            "2026-01-05T06:20:00-07:00",
        ),
        workout(
            "new",
            "yoga",
            "2026-01-05T06:12:00-07:00",
            "2026-01-05T06:14:00-07:00",
        ),
    ];
    if repeat {
        workouts.push(unrelated);
    }
    let next = stage(&scratch.path().join("next"), &[("workout", workouts)]);
    (journal, scratch, next, original.import_id)
}

fn assert_repair_visibility(root: &Path) {
    assert!(
        !window(root, "20260105", "061000_300")
            .join("talents/facets.json")
            .exists()
    );
    let other = entries(root, OTHER, "20260105", "Oura · imported")
        .join("\n")
        .replace('\\', "");
    let health = entries(root, HEALTH, "20260105", "Oura · imported")
        .join("\n")
        .replace('\\', "");
    assert!(
        !other.contains("walking") && !other.contains("06:11"),
        "{other}"
    );
    assert!(
        !health.contains("running") && !health.contains("walking") && !health.contains("06:20"),
        "{health}"
    );
}

#[test]
fn repairing_an_unfiled_tile_reserves_its_old_window_at_every_interruption() {
    for repeat in [false, true] {
        for facet in [Some("health"), None] {
            for point in ["pending", "written", "marking", "published"] {
                let (journal, _scratch, next, origin) = unfiled_tile_fixture(repeat);
                let root = journal.path();
                let stop = stopping_at(point, 1);
                let mut saving = request(&next, "20261104_140000", facet);
                saving.stop_at = Some(&stop);
                run_oura_segments(root, &saving).unwrap_err();
                let Read::Trusted(state) = read_state(root, &OURA) else {
                    panic!("no checkpoint");
                };
                assert_repair_visibility(root);
                saving.stop_at = None;
                let done = run_oura_segments(root, &saving).unwrap();
                assert_eq!(done.import_id, state.import_id);
                assert_repair_visibility(root);
                let visible = entries(root, HEALTH, "20260105", "Oura · imported").join("\n");
                assert_eq!(visible.contains("yoga"), facet.is_some(), "{visible}");
                assert!(
                    done.summary
                        .contains("earlier entries stayed without a facet"),
                    "{}",
                    done.summary
                );
                let parent_id = placed(root, "private")[0].2.id.clone();
                let rows = stored_windows_of_day(root, &OURA, "20260105")
                    .unwrap()
                    .into_iter()
                    .flat_map(|window| {
                        stored_rows(root, &OURA, &window)
                            .unwrap()
                            .unwrap()
                            .1
                            .into_iter()
                            .map(move |row| (window.clone(), row))
                    })
                    .filter(|(_, row)| {
                        row.kind == RowKind::ExperienceTile
                            && row.of.as_deref() == Some(parent_id.as_str())
                            && row.index == Some(2)
                    })
                    .collect::<Vec<_>>();
                assert_eq!(rows.len(), 1, "{rows:?}");
                assert_eq!(rows[0].1.run.as_deref(), Some(origin.as_str()));
                assert_eq!(
                    fs::read_to_string(
                        window(root, &rows[0].0.0, &rows[0].0.1).join("talents/facets.json")
                    )
                    .unwrap(),
                    facet_file_text("other")
                );
            }
        }
    }
}

#[test]
fn a_repair_resume_refuses_a_changed_assignment_on_its_reserved_source() {
    let (journal, _scratch, next, _) = unfiled_tile_fixture(true);
    let root = journal.path();
    let stop = stopping_at("pending", 1);
    let mut saving = request(&next, "20261104_140000", Some("health"));
    saving.stop_at = Some(&stop);
    run_oura_segments(root, &saving).unwrap_err();
    let Read::Trusted(state) = read_state(root, &OURA) else {
        panic!("no checkpoint");
    };
    let repair = state
        .pending
        .as_ref()
        .unwrap()
        .tile_repairs
        .first()
        .unwrap();
    let old = window(root, "20260105", &repair.source_key);
    let before = fs::read(old.join(BODY_RECORDS_FILE)).unwrap();
    fs::write(old.join("talents/facets.json"), facet_file_text("health")).unwrap();
    saving.stop_at = None;
    assert!(run_oura_segments(root, &saving).is_err());
    assert_eq!(fs::read(old.join(BODY_RECORDS_FILE)).unwrap(), before);
}

#[test]
fn a_refused_repair_destination_never_retires_the_old_live_tile() {
    for deleted in [false, true] {
        let (journal, _scratch, next, origin) = unfiled_tile_fixture(true);
        let root = journal.path();
        let stop = stopping_at("pending", 1);
        let mut saving = request(&next, "20261104_140000", Some("health"));
        saving.stop_at = Some(&stop);
        run_oura_segments(root, &saving).unwrap_err();
        let Read::Trusted(state) = read_state(root, &OURA) else {
            panic!("no checkpoint");
        };
        let repair = state
            .pending
            .as_ref()
            .unwrap()
            .tile_repairs
            .first()
            .unwrap();
        let target = window(root, "20260105", &repair.destination_key);
        fs::create_dir_all(&target).unwrap();
        if deleted {
            fs::write(target.join("tombstone.json"), "{}").unwrap();
        } else {
            fs::write(target.join(BODY_RECORDS_FILE), "unreadable old input").unwrap();
        }
        saving.stop_at = None;
        assert!(run_oura_segments(root, &saving).is_err());
        let old = stored_rows(root, &OURA, &("20260105".into(), repair.source_key.clone()))
            .unwrap()
            .unwrap()
            .1;
        let tile = old.iter().find(|row| row.id == repair.mark.id).unwrap();
        assert_eq!(tile.kind, RowKind::ExperienceTile);
        assert_eq!(tile.run.as_deref(), Some(origin.as_str()));
        assert_repair_visibility(root);
    }
}

#[test]
fn takeover_keeps_the_reached_repair_copy_and_the_sources_first_pass_rows() {
    let (journal, scratch, next, _) = unfiled_tile_fixture(true);
    let root = journal.path();
    let stop = stopping_at("marking", 1);
    let mut saving = request(&next, "20261104_140000", None);
    saving.stop_at = Some(&stop);
    run_oura_segments(root, &saving).unwrap_err();
    let Read::Trusted(state) = read_state(root, &OURA) else {
        panic!("no checkpoint");
    };
    let pending = state.pending.as_ref().unwrap();
    let repair = pending.tile_repairs.first().unwrap();
    let source = pending
        .windows
        .iter()
        .find(|window| window.key == repair.source_key)
        .unwrap();
    let actual =
        fs::read_to_string(window(root, "20260105", &repair.source_key).join(BODY_RECORDS_FILE))
            .unwrap();
    assert_eq!(
        source.first_sha.as_deref(),
        Some(text_sha(&actual).as_str())
    );
    assert_ne!(source.written_sha.as_deref(), source.first_sha.as_deref());
    assert_repair_visibility(root);
    let takeover = stage(
        &scratch.path().join("takeover"),
        &[
            (
                "workout",
                vec![
                    workout(
                        "private",
                        "running",
                        "2026-01-05T06:00:00-07:00",
                        "2026-01-05T06:20:00-07:00",
                    ),
                    workout(
                        "unfiled",
                        "walking",
                        "2026-01-05T06:11:00-07:00",
                        "2026-01-05T06:14:00-07:00",
                    ),
                    workout(
                        "new",
                        "yoga",
                        "2026-01-05T06:12:00-07:00",
                        "2026-01-05T06:14:00-07:00",
                    ),
                ],
            ),
            (
                "heartrate",
                vec![heart("2026-01-05T06:13:00-07:00", 8333, "awake")],
            ),
        ],
    );
    let done = run(root, &takeover, "20261104_150000", Some("health"));
    assert_ne!(done.import_id, state.import_id);
    assert_repair_visibility(root);
    let listing = fs::read_to_string(
        root.join("imports")
            .join(&state.import_id)
            .join(LISTING_FILE),
    )
    .unwrap();
    assert!(
        listing.contains(&repair.source_key) && listing.contains(&repair.destination_key),
        "{listing}"
    );
    let entry: Value = serde_json::from_str(listing.lines().last().unwrap()).unwrap();
    assert!(
        !entry["moved"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|moved| moved[0] == repair.source_key)
    );
    let new = placed(root, "new");
    assert_eq!(new.len(), 1);
    assert_eq!(new[0].2.run.as_deref(), Some(state.import_id.as_str()));
}

#[test]
fn a_no_facet_extension_never_files_an_unrelated_unfiled_workout() {
    let journal = journal();
    let root = journal.path();
    facets(root);
    let scratch = tempfile::tempdir().unwrap();
    let first = stage(
        &scratch.path().join("first"),
        &[(
            "workout",
            vec![workout(
                "private",
                "running",
                "2026-01-05T06:00:00-07:00",
                "2026-01-05T06:04:00-07:00",
            )],
        )],
    );
    run(root, &first, "20261104_120000", Some("other"));
    let second = stage(
        &scratch.path().join("unfiled"),
        &[(
            "workout",
            vec![workout(
                "unfiled",
                "walking",
                "2026-01-05T06:11:00-07:00",
                "2026-01-05T06:14:00-07:00",
            )],
        )],
    );
    run(root, &second, "20261104_130000", None);
    let next = stage(
        &scratch.path().join("next"),
        &[(
            "workout",
            vec![workout(
                "private",
                "running",
                "2026-01-05T06:00:00-07:00",
                "2026-01-05T06:20:00-07:00",
            )],
        )],
    );
    run(root, &next, "20261104_140000", None);
    assert_repair_visibility(root);
    let visible = entries(root, OTHER, "20260105", "Oura · imported").join("\n");
    assert!(visible.contains("running"), "{visible}");
}

#[test]
fn a_no_facet_extension_keeps_new_heart_readings_out_of_its_inherited_tiles() {
    let journal = journal();
    let root = journal.path();
    facets(root);
    let scratch = tempfile::tempdir().unwrap();
    let first = stage(
        &scratch.path().join("first"),
        &[(
            "workout",
            vec![workout(
                "private",
                "running",
                "2026-01-05T06:00:00-07:00",
                "2026-01-05T06:04:00-07:00",
            )],
        )],
    );
    run(root, &first, "20261104_120000", Some("other"));
    let next = stage(
        &scratch.path().join("next"),
        &[
            (
                "workout",
                vec![workout(
                    "private",
                    "running",
                    "2026-01-05T06:00:00-07:00",
                    "2026-01-05T06:20:00-07:00",
                )],
            ),
            (
                "heartrate",
                vec![heart("2026-01-05T06:12:00-07:00", 8333, "awake")],
            ),
        ],
    );
    run(root, &next, "20261104_130000", None);
    let other = entries(root, OTHER, "20260105", "Oura · imported").join("\n");
    assert!(
        other.contains("running") && !other.contains("8333"),
        "{other}"
    );
    let original = window(root, "20260105", "061000_300");
    assert!(!original.join("talents/facets.json").exists());
    assert!(
        fs::read_to_string(original.join(BODY_RECORDS_FILE))
            .unwrap()
            .contains("8333")
    );
}

#[test]
fn a_new_oura_experience_tile_cannot_reveal_its_label_or_span_under_the_run_facet() {
    let journal = journal();
    let root = journal.path();
    facets(root);
    let scratch = tempfile::tempdir().unwrap();
    let first = stage(
        &scratch.path().join("first"),
        &[(
            "workout",
            vec![workout(
                "private",
                "running",
                "2026-01-05T06:00:00-07:00",
                "2026-01-05T06:04:00-07:00",
            )],
        )],
    );
    run(root, &first, "20261104_120000", Some("other"));
    let next = stage(
        &scratch.path().join("next"),
        &[
            (
                "workout",
                vec![workout(
                    "private",
                    "running",
                    "2026-01-05T06:00:00-07:00",
                    "2026-01-05T06:20:00-07:00",
                )],
            ),
            (
                "heartrate",
                vec![heart("2026-01-05T06:12:00-07:00", 7333, "awake")],
            ),
        ],
    );
    let done = run(root, &next, "20261104_130000", Some("health"));
    let chosen = entries(root, HEALTH, "20260105", "Oura · imported")
        .join("\n")
        .replace('\\', "");
    let earlier = entries(root, OTHER, "20260105", "Oura · imported")
        .join("\n")
        .replace('\\', "");
    assert!(chosen.contains("7333"), "{chosen}");
    assert!(
        !chosen.contains("running") && !chosen.contains("06:20"),
        "a new tile revealed the saved experience: {chosen}"
    );
    assert!(
        earlier.contains("running") && earlier.contains("06:20"),
        "{earlier}"
    );
    assert!(
        done.summary
            .contains("1 updated entry stayed under your other facet"),
        "{}",
        done.summary
    );
    assert!(
        done.summary.contains("1 new entry was saved separately"),
        "{}",
        done.summary
    );
}

#[test]
fn an_oura_revision_keeps_its_facet_and_new_readings_get_the_run_facet() {
    let journal = journal();
    let root = journal.path();
    facets(root);
    let scratch = tempfile::tempdir().unwrap();
    let initial = stage(
        &scratch.path().join("first"),
        &[(
            "heartrate",
            vec![heart("2026-01-05T06:11:00-07:00", 7111, "awake")],
        )],
    );
    let first = run(root, &initial, "20261104_120000", Some("other"));
    let pages = stage(
        &scratch.path().join("next"),
        &[(
            "heartrate",
            vec![
                heart("2026-01-05T06:11:00-07:00", 7222, "awake"),
                heart("2026-01-05T06:42:00-07:00", 7333, "awake"),
            ],
        )],
    );
    let stop = |point: &str| point == "written-all";
    let mut saving = request(&pages, "20261104_130000", Some("health"));
    saving.stop_at = Some(&stop);
    run_oura_segments(root, &saving).unwrap_err();
    let Read::Trusted(state) = read_state(root, &OURA) else {
        panic!("no saved run");
    };
    assert_eq!(state.totals.split_rows, 1);
    assert_eq!(state.totals.retained_revisions.values().sum::<u64>(), 1);
    let old = entries(root, OTHER, "20260105", "Oura · imported");
    let new = entries(root, HEALTH, "20260105", "Oura · imported");
    assert_eq!(old.len(), 1, "{old:?}");
    assert_eq!(new.len(), 1, "{new:?}");
    assert!(old[0].contains("7222") && !old[0].contains("7333"));
    assert!(new[0].contains("7333") && !new[0].contains("7222"));
    let old_rows = decode_rows(
        &fs::read_to_string(window(root, "20260105", "060000_3600").join(BODY_RECORDS_FILE))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(old_rows[0].run.as_deref(), Some(first.import_id.as_str()));
    let result = run(root, &pages, "20261104_140000", Some("health"));
    assert!(
        result
            .summary
            .contains("1 updated entry stayed under your other facet"),
        "{}",
        result.summary
    );
    assert_eq!(
        result.import_id, state.import_id,
        "ordinary resume keeps its import"
    );
    let repeat = run(root, &pages, "20261104_150000", Some("health"));
    assert_eq!(repeat.published, 0, "{}", repeat.summary);
}

#[test]
fn a_sync_leaves_out_a_deleted_oura_sibling_and_keeps_the_other_tag() {
    let journal = journal();
    let root = journal.path();
    facets(root);
    let scratch = tempfile::tempdir().unwrap();
    let initial = stage(
        &scratch.path().join("first"),
        &[(
            "enhanced_tag",
            vec![tag("old", "2026-01-05T06:11:00-07:00")],
        )],
    );
    run(root, &initial, "20261104_120000", Some("other"));
    let next = stage(
        &scratch.path().join("next"),
        &[(
            "enhanced_tag",
            vec![tag("new", "2026-01-05T06:42:00-07:00")],
        )],
    );
    run(root, &next, "20261104_130000", Some("health"));
    let found = placed(root, "new");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].1, "060000_3602");
    delete(root, "20260105", "060000_3602");
    // Staged pages stand in for an ordinary sync: it fetched the tag before, so
    // the owner's delete keeps it out, and nothing else of that hour moves.
    let result = run(root, &next, "20261104_140000", Some("health"));
    assert!(result.finished, "{}", result.summary);
    assert!(placed(root, "new").is_empty(), "{}", result.summary);
    assert_eq!(placed(root, "old").len(), 1);
}

#[test]
fn deleting_the_original_does_not_drop_a_revision_to_a_live_sibling() {
    let journal = journal();
    let root = journal.path();
    facets(root);
    let scratch = tempfile::tempdir().unwrap();
    let first = stage(
        &scratch.path().join("first"),
        &[(
            "enhanced_tag",
            vec![
                tag("old", "2026-01-05T06:11:00-07:00"),
                tag("blocked", "2026-01-05T07:11:00-07:00"),
            ],
        )],
    );
    run(root, &first, "20261104_120000", Some("other"));
    let next = stage(
        &scratch.path().join("next"),
        &[(
            "enhanced_tag",
            vec![tag("new", "2026-01-05T06:42:00-07:00")],
        )],
    );
    run(root, &next, "20261104_130000", Some("health"));
    delete(root, "20260105", "060000_3600");
    delete(root, "20260105", "070000_3600");
    let revised = stage(
        &scratch.path().join("revised"),
        &[(
            "enhanced_tag",
            vec![tag("new", "2026-01-05T06:43:00-07:00")],
        )],
    );
    let result = run(root, &revised, "20261104_140000", Some("other"));
    let found = placed(root, "new");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].1, "060000_3602");
    assert!(
        found[0].2.start.as_deref().unwrap().contains("06:43"),
        "{} {:?}",
        result.summary,
        found[0].2
    );
    assert_eq!(
        entries(root, HEALTH, "20260105", "Oura · imported").len(),
        1
    );
    assert!(entries(root, OTHER, "20260105", "Oura · imported").is_empty());
    for (id, at) in [
        ("brand-new", "2026-01-05T06:44:00-07:00"),
        ("new", "2026-01-05T07:43:00-07:00"),
    ] {
        let refused = stage(
            &scratch.path().join(id),
            &[("enhanced_tag", vec![tag(id, at)])],
        );
        run(root, &refused, "20261104_150000", Some("health"));
        assert!(placed(root, "brand-new").is_empty());
        assert!(
            placed(root, "new")[0]
                .2
                .start
                .as_deref()
                .unwrap()
                .contains("06:43")
        );
    }
}

#[test]
fn carried_oura_revisions_do_not_file_unrelated_unfiled_rows_or_lose_their_facet() {
    for repeat in [false, true] {
        let journal = journal();
        let root = journal.path();
        facets(root);
        let scratch = tempfile::tempdir().unwrap();
        let mut a = tag("a", "2026-01-05T05:10:00-07:00");
        a["comment"] = json!("carried marker");
        let first = stage(&scratch.path().join("first"), &[("enhanced_tag", vec![a])]);
        run(root, &first, "20261104_120000", Some("other"));
        let mut c = tag("c", "2026-01-05T06:10:00-07:00");
        c["comment"] = json!("unfiled marker");
        let unfiled = stage(
            &scratch.path().join("unfiled"),
            &[("enhanced_tag", vec![c.clone()])],
        );
        run(root, &unfiled, "20261104_130000", None);
        let mut a = tag("a", "2026-01-05T06:20:00-07:00");
        a["comment"] = json!("revised marker");
        let other = if repeat {
            c
        } else {
            tag("z", "2026-01-05T06:30:00-07:00")
        };
        let next = stage(
            &scratch.path().join("next"),
            &[("enhanced_tag", vec![a, other])],
        );
        let result = run(root, &next, "20261104_140000", Some("health"));
        let prior = entries(root, OTHER, "20260105", "Oura · imported").join("\n");
        let chosen = entries(root, HEALTH, "20260105", "Oura · imported").join("\n");
        assert!(
            prior.contains("revised marker") && !prior.contains("unfiled marker"),
            "{prior}"
        );
        assert!(
            chosen.contains("unfiled marker") && !chosen.contains("revised marker"),
            "{chosen}"
        );
        assert_eq!(placed(root, "a").len(), 1);
        assert!(
            result
                .summary
                .contains("1 updated entry stayed under your other facet"),
            "{}",
            result.summary
        );
        if !repeat {
            assert!(
                result.summary.contains("saved separately"),
                "{}",
                result.summary
            );
        }
    }
}

#[test]
fn takeover_lists_a_two_pass_windows_reached_first_write() {
    let journal = journal();
    let root = journal.path();
    facets(root);
    let scratch = tempfile::tempdir().unwrap();
    let moving = workout(
        "moving",
        "running",
        "2026-01-05T06:00:00-07:00",
        "2026-01-05T06:10:00-07:00",
    );
    let staying = workout(
        "staying",
        "running",
        "2026-01-05T06:01:00-07:00",
        "2026-01-05T06:04:00-07:00",
    );
    let first_pages = stage(
        &scratch.path().join("first"),
        &[("workout", vec![moving, staying])],
    );
    let first = run(root, &first_pages, "20261104_120000", Some("other"));
    let moving = workout(
        "moving",
        "running",
        "2026-01-05T06:05:00-07:00",
        "2026-01-05T06:15:00-07:00",
    );
    let staying = workout(
        "staying",
        "walking",
        "2026-01-05T06:01:00-07:00",
        "2026-01-05T06:04:00-07:00",
    );
    let next_pages = stage(
        &scratch.path().join("next"),
        &[
            ("workout", vec![moving.clone(), staying.clone()]),
            (
                "heartrate",
                vec![heart("2026-01-05T06:02:00-07:00", 7555, "workout")],
            ),
        ],
    );
    let stop = stopping_at("marking", 1);
    let mut saving = request(&next_pages, "20261104_130000", Some("health"));
    saving.stop_at = Some(&stop);
    run_oura_segments(root, &saving).unwrap_err();
    let Read::Trusted(state) = read_state(root, &OURA) else {
        panic!("no saved run");
    };
    let pending = state.pending.as_ref().unwrap();
    let old = pending
        .windows
        .iter()
        .find(|window| window.key == "060000_300")
        .unwrap();
    let actual = text_sha(
        &fs::read_to_string(window(root, "20260105", "060000_300").join(BODY_RECORDS_FILE))
            .unwrap(),
    );
    assert_eq!(old.first_sha.as_deref(), Some(actual.as_str()));
    assert_ne!(old.written_sha.as_deref(), Some(actual.as_str()));
    let takeover = stage(
        &scratch.path().join("takeover"),
        &[
            ("workout", vec![moving, staying]),
            (
                "heartrate",
                vec![
                    heart("2026-01-05T06:02:00-07:00", 7555, "workout"),
                    heart("2026-01-05T06:03:00-07:00", 7666, "workout"),
                ],
            ),
        ],
    );
    let result = run(root, &takeover, "20261104_140000", Some("health"));
    assert_ne!(result.import_id, state.import_id);
    let listing = fs::read_to_string(
        root.join("imports")
            .join(&state.import_id)
            .join(LISTING_FILE),
    )
    .unwrap();
    assert!(listing.contains("060000_300"), "{listing}");
    assert_eq!(placed(root, "moving").len(), 1);
    assert_eq!(placed(root, "staying").len(), 1);
    assert_eq!(
        placed(root, "staying")[0].2.run.as_deref(),
        Some(first.import_id.as_str())
    );
}
