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
fn deleting_an_oura_logged_first_row_sibling_adds_no_matching_refusal() {
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
    let result = run(root, &next, "20261104_140000", Some("health"));
    assert!(result.finished, "{}", result.summary);
    let found = placed(root, "new");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].1, "060000_3603");
    assert_eq!(placed(root, "old").len(), 1);
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
