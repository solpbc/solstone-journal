// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::*;
use crate::body_stream::APPLE;
use solstone_core_format::content::body_records::{BODY_RECORDS_FILE, encode_rows};

fn row(id: &str, hour: &str, value: &str) -> BodyRow {
    let mut row = BodyRow::new(
        RowKind::Reading,
        id.to_owned(),
        "Watch",
        "heart",
        "heart rate".to_owned(),
    );
    row.at = Some(format!("2026-10-10 {hour}:10:00 -0600"));
    row.value = Some(value.to_owned());
    row
}

fn put(root: &Path, key: &str, rows: &[BodyRow], facet: Option<&str>) {
    let dir = stream_dir(root, &APPLE, "20261010").join(key);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(BODY_RECORDS_FILE), encode_rows(rows)).unwrap();
    if let Some(facet) = facet {
        fs::create_dir_all(dir.join("talents")).unwrap();
        fs::write(dir.join("talents/facets.json"), facet_file_text(facet)).unwrap();
    }
}

#[test]
fn a_carried_revision_never_shares_an_unfiled_original_or_changes_its_planned_facet() {
    // Both identity orders, and a source that omits the old unfiled reading.
    for (carried, old, repeat) in [("a", "c", true), ("z", "a", true), ("a", "c", false)] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        put(
            root,
            "050000_3600",
            &[row(carried, "05", "60")],
            Some("body"),
        );
        put(root, "060000_3600", &[row(old, "06", "70")], None);
        let mut incoming = BTreeMap::from([(carried.to_owned(), row(carried, "06", "61"))]);
        let other = if repeat { old } else { "new" };
        incoming.insert(other.to_owned(), row(other, "06", "80"));
        let filed = DayPlacement::load(root, &APPLE, "20261010")
            .unwrap()
            .file(
                vec![(("20261010".to_owned(), "060000_3600".to_owned()), incoming)],
                Some("work"),
                false,
            )
            .unwrap();
        let destination = |id: &str| {
            filed
                .windows
                .iter()
                .find(|(_, rows)| rows.contains_key(id))
                .unwrap()
                .0
                .1
                .as_str()
        };
        let moved_key = destination(carried);
        assert_ne!(
            moved_key, "060000_3600",
            "a carried facet must not claim old unfiled rows"
        );
        assert_eq!(filed.filings[moved_key], facet_file_text("body"));
        assert_eq!(filed.filings[destination(other)], facet_file_text("work"));
        assert_ne!(moved_key, destination(other));
        assert_eq!(
            filed.revisions[moved_key][&facet_file_text("body")].len(),
            1
        );
        if !repeat {
            assert_eq!(
                filed.split_rows.values().map(BTreeSet::len).sum::<usize>(),
                1,
                "new entries share a logical slot with the retained foreign revision"
            );
        }
    }
}

#[test]
fn a_deleted_original_refuses_new_rows_but_not_a_live_siblings_revision() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    put(root, "060000_3600", &[row("old", "06", "60")], Some("work"));
    fs::write(
        stream_dir(root, &APPLE, "20261010").join("060000_3600/tombstone.json"),
        "{}",
    )
    .unwrap();
    put(root, "060000_3602", &[row("a", "06", "70")], Some("body"));
    let incoming = BTreeMap::from([
        ("a".to_owned(), row("a", "06", "80")),
        ("new".to_owned(), row("new", "06", "90")),
    ]);
    let filed = DayPlacement::load(root, &APPLE, "20261010")
        .unwrap()
        .file(
            vec![(("20261010".to_owned(), "060000_3600".to_owned()), incoming)],
            Some("work"),
            false,
        )
        .unwrap();
    assert!(filed.windows.iter().any(|((_, key), rows)| {
        key == "060000_3602"
            && rows
                .get("a")
                .is_some_and(|row| row.value.as_deref() == Some("80"))
    }));
    let (original, rows) = filed
        .windows
        .iter()
        .find(|((_, key), _)| key == "060000_3600")
        .unwrap();
    assert!(rows.contains_key("new") && !rows.contains_key("a"));
    let refused = crate::apple_health::decide(
        root,
        &APPLE,
        original,
        rows.clone(),
        Some("work"),
        false,
        None,
    )
    .unwrap();
    assert!(refused.stayed_deleted && refused.contents.is_none());
}

#[test]
fn an_orphan_unfiled_tile_never_lends_its_window_to_a_run_facet() {
    for deleted in [false, true] {
        for repeat in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path();
            crate::apple_health::read_or_create_zone(root, &APPLE).unwrap();
            let mut tile = BodyRow::new(
                RowKind::ExperienceTile,
                "orphan-tile".into(),
                "Watch",
                "sleep",
                "private night".into(),
            );
            tile.of = Some("missing-parent".into());
            tile.start = Some("2026-10-10 05:00:00 -0600".into());
            tile.end = Some("2026-10-10 07:00:00 -0600".into());
            if deleted {
                let mut parent = tile.clone();
                parent.kind = RowKind::Experience;
                parent.id = "missing-parent".into();
                put(root, "050000_3600", &[parent], Some("work"));
                fs::write(
                    stream_dir(root, &APPLE, "20261010").join("050000_3600/tombstone.json"),
                    "{}",
                )
                .unwrap();
            }
            put(root, "060000_3600", &[tile.clone()], None);
            let hour = if repeat { "07" } else { "06" };
            let mut incoming = BTreeMap::from([("new".to_owned(), row("new", hour, "7333"))]);
            if repeat {
                tile.end = Some("2026-10-10 08:00:00 -0600".into());
                incoming.insert(tile.id.clone(), tile);
            }
            let filed = DayPlacement::load(root, &APPLE, "20261010")
                .unwrap()
                .file(
                    vec![(("20261010".into(), format!("{hour}0000_3600")), incoming)],
                    Some("body"),
                    false,
                )
                .unwrap();
            assert!(filed.reserved_unfiled.contains("060000_3600"));
            assert!(!filed.filings.contains_key("060000_3600"));
            assert!(filed.tile_repairs.is_empty());
            if repeat {
                let ((_, key), _) = filed
                    .windows
                    .iter()
                    .find(|(_, rows)| rows.contains_key("orphan-tile"))
                    .unwrap();
                assert_eq!(key, "060000_3600", "an orphan keeps its physical window");
            }
            let ((_, destination), _) = filed
                .windows
                .iter()
                .find(|(_, rows)| rows.contains_key("new"))
                .unwrap();
            assert_ne!(destination, "060000_3600");
            assert_eq!(filed.filings[destination], facet_file_text("body"));
        }
    }
}

#[test]
fn a_no_facet_tile_never_claims_an_absent_original_before_or_after_a_reading() {
    for (tile_id, reading_id) in [("a-tile", "z-reading"), ("z-tile", "a-reading")] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        crate::apple_health::read_or_create_zone(root, &APPLE).unwrap();
        let mut parent = BodyRow::new(
            RowKind::Experience,
            "parent".into(),
            "Watch",
            "workout",
            "private workout".into(),
        );
        parent.start = Some("2026-10-10 05:00:00 -0600".into());
        parent.end = Some("2026-10-10 07:00:00 -0600".into());
        put(root, "050000_3600", &[parent.clone()], Some("other"));
        let mut tile = parent;
        tile.kind = RowKind::ExperienceTile;
        tile.id = tile_id.into();
        tile.of = Some("parent".into());
        let incoming = BTreeMap::from([
            (tile_id.to_owned(), tile),
            (reading_id.to_owned(), row(reading_id, "06", "8333")),
        ]);
        let filed = DayPlacement::load(root, &APPLE, "20261010")
            .unwrap()
            .file(
                vec![(("20261010".into(), "060000_3600".into()), incoming)],
                None,
                false,
            )
            .unwrap();
        let destination = |id: &str| {
            &filed
                .windows
                .iter()
                .find(|(_, rows)| rows.contains_key(id))
                .unwrap()
                .0
                .1
        };
        assert_eq!(destination(reading_id), "060000_3600");
        assert!(!filed.filings.contains_key("060000_3600"));
        assert_ne!(destination(tile_id), "060000_3600");
        assert_eq!(
            filed.filings[destination(tile_id)],
            facet_file_text("other")
        );
    }
}
