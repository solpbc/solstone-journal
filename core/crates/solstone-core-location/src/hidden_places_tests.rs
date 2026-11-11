// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! A place the owner hid: what each day's location words and an activity's
//! "at" leave out, and when the days are written again.

use std::fs;
use std::path::Path;

use crate::beside::{DayBeside, location_beside, place_at};
use crate::day::tests::{directory, fix, iphone_header, tier, write};
use crate::day::{DayLocation, read_day};
use crate::places::{Place, PlaceStore, store_path};
use crate::projection::{Reconciler, document_path, render_day_document};

const DAY: &str = "20261001";

/// Readings near Golden.
const GOLDEN: (f64, f64) = (39.7555, -105.2211);
/// The visit, near Denver.
const VISIT: (f64, f64) = (39.7527, -105.0001);
/// The stay, near Boulder.
const STAY: (f64, f64) = (40.0150, -105.2705);

fn visit_line(iso_day: &str, h_acc: f64, (lat, lon): (f64, f64)) -> String {
    format!(
        r#"{{"arrival":"{iso_day}T21:02:00Z","departure":"{iso_day}T22:40:00Z","h_acc":{h_acc},"lat":{lat},"lon":{lon},"schema":"solstone.location.visit/1"}}"#
    )
}

/// Readings near Golden 14:00 to 14:04, a visit near Denver 15:02 to 16:40
/// and a stay near Boulder 18:00 to 18:09, all from the phone.
fn seed(root: &Path, visit_accuracy: f64) {
    let header = iphone_header("full", "full");
    write(
        root,
        DAY,
        "phone_mobile-segment",
        "140000_300",
        &format!(
            "{header}\n{}\n{}\n",
            fix("2026-10-01T20:00:10Z", GOLDEN.0, GOLDEN.1, 10.0),
            fix("2026-10-01T20:04:10Z", GOLDEN.0 + 0.0001, GOLDEN.1, 10.0)
        ),
    );
    write(
        root,
        DAY,
        "phone_mobile-segment",
        "150000_300",
        &format!(
            "{header}\n{}\n",
            visit_line("2026-10-01", visit_accuracy, VISIT)
        ),
    );
    let dwell: Vec<String> = (0..10)
        .map(|minute| {
            fix(
                &format!("2026-10-02T00:{minute:02}:10Z"),
                STAY.0 + f64::from(minute % 3) * 0.00002,
                STAY.1,
                8.0,
            )
        })
        .collect();
    write(
        root,
        DAY,
        "phone_mobile-segment",
        "180000_300",
        &format!("{header}\n{}\n", dwell.join("\n")),
    );
}

fn day(root: &Path) -> DayLocation {
    read_day(
        root,
        DAY,
        tier(),
        None,
        &directory(),
        chrono_tz::America::Denver,
    )
}

fn place(id: &str, (latitude, longitude): (f64, f64), name: Option<&str>, hidden: bool) -> Place {
    Place {
        id: id.to_owned(),
        latitude,
        longitude,
        name: name.map(str::to_owned),
        alias: None,
        hidden,
        days: Vec::new(),
        dormant: false,
    }
}

fn store(places: Vec<Place>) -> PlaceStore {
    PlaceStore { version: 1, places }
}

fn at(h: u32, m: u32) -> u32 {
    h * 3600 + m * 60
}

fn lines_with<'a>(text: &'a str, words: &str) -> Vec<&'a str> {
    text.lines().filter(|line| line.contains(words)).collect()
}

#[test]
fn every_entry_is_named_with_no_place_hidden() {
    let root = tempfile::tempdir().unwrap();
    seed(root.path(), 40.0);
    let text = render_day_document(&day(root.path()), &PlaceStore::default());
    assert_eq!(lines_with(&text, "Golden").len(), 1, "{text}");
    assert_eq!(lines_with(&text, "a visit").len(), 1, "{text}");
    assert_eq!(lines_with(&text, "Boulder").len(), 1, "{text}");
}

#[test]
fn a_stay_at_a_hidden_place_is_left_out_and_the_rest_of_the_day_stays() {
    let root = tempfile::tempdir().unwrap();
    seed(root.path(), 40.0);
    let day = day(root.path());
    // An unnamed hidden place: the map's own rule, by distance alone.
    let hidden = store(vec![place("p-stay", STAY, None, true)]);
    let text = render_day_document(&day, &hidden);
    assert!(!text.contains("a stay"), "{text}");
    assert!(!text.contains("Boulder"), "{text}");
    // Twin: the visit away from it, and the readings, keep their words.
    assert_eq!(lines_with(&text, "a visit").len(), 1, "{text}");
    assert!(lines_with(&text, "a visit")[0].contains("Denver"), "{text}");
    assert_eq!(lines_with(&text, "Golden").len(), 1, "{text}");
    // Shown again, the stay is back.
    let shown = store(vec![place("p-stay", STAY, None, false)]);
    assert_eq!(
        lines_with(&render_day_document(&day, &shown), "Boulder").len(),
        1
    );
}

#[test]
fn a_visit_at_a_hidden_place_loses_its_name_venue_and_area() {
    for accuracy in [40.0, 500.0] {
        let root = tempfile::tempdir().unwrap();
        seed(root.path(), accuracy);
        let day = day(root.path());
        let named = store(vec![place(
            "p-visit",
            VISIT,
            Some("the climbing gym"),
            true,
        )]);
        let text = render_day_document(&day, &named);
        assert!(!text.contains("a visit"), "{accuracy}: {text}");
        assert!(!text.contains("climbing gym"), "{accuracy}: {text}");
        assert!(!text.contains("Denver"), "{accuracy}: {text}");
        assert_eq!(lines_with(&text, "Boulder").len(), 1, "{accuracy}: {text}");
    }
    // Twin: shown, a precise visit is said by the owner's name.
    let root = tempfile::tempdir().unwrap();
    seed(root.path(), 40.0);
    let shown = store(vec![place(
        "p-visit",
        VISIT,
        Some("the climbing gym"),
        false,
    )]);
    let text = render_day_document(&day(root.path()), &shown);
    assert!(text.contains("the climbing gym"), "{text}");
}

#[test]
fn readings_all_within_reach_go_whole_and_partly_within_reach_lose_their_area() {
    let root = tempfile::tempdir().unwrap();
    seed(root.path(), 40.0);
    let day = day(root.path());
    let whole = store(vec![place("p-golden", GOLDEN, None, true)]);
    let text = render_day_document(&day, &whole);
    assert!(!text.contains("Golden"), "{text}");
    assert!(!text.contains("14:00"), "{text}");
    assert_eq!(lines_with(&text, "Boulder").len(), 1, "{text}");

    // A run whose first reading is within reach and whose last is well
    // beyond it keeps its times and count, without its area.
    let partial = tempfile::tempdir().unwrap();
    let header = iphone_header("full", "full");
    write(
        partial.path(),
        DAY,
        "phone_mobile-segment",
        "140000_300",
        &format!(
            "{header}\n{}\n{}\n",
            fix("2026-10-01T20:00:10Z", GOLDEN.0, GOLDEN.1, 10.0),
            fix("2026-10-01T20:04:10Z", GOLDEN.0 + 0.004, GOLDEN.1, 10.0)
        ),
    );
    let partial_day = self::day(partial.path());
    let open = render_day_document(&partial_day, &PlaceStore::default());
    assert_eq!(lines_with(&open, "Golden").len(), 1, "{open}");
    let text = render_day_document(&partial_day, &whole);
    assert!(!text.contains("Golden"), "{text}");
    let kept = lines_with(&text, "14:00 to 14:04");
    assert_eq!(kept.len(), 1, "{text}");
    assert!(kept[0].contains("2 readings"), "{text}");
}

#[test]
fn nothing_is_beside_an_activity_at_a_hidden_place() {
    let root = tempfile::tempdir().unwrap();
    seed(root.path(), 40.0);
    let day = day(root.path());
    let golden = (at(14, 0), at(14, 4));
    let visit = (at(15, 10), at(16, 0));
    let stay = (at(18, 2), at(18, 6));
    let open = PlaceStore::default();
    for span in [golden, visit, stay] {
        assert!(
            location_beside(&day, &open, span, None).is_some(),
            "{span:?}"
        );
    }
    for (hidden, span, away) in [
        (GOLDEN, golden, visit),
        (VISIT, visit, stay),
        (STAY, stay, golden),
    ] {
        let store = store(vec![place("p", hidden, Some("somewhere"), true)]);
        assert_eq!(location_beside(&day, &store, span, None), None, "{span:?}");
        // Twin: an activity away from it keeps its words.
        let kept = location_beside(&day, &store, away, None).unwrap();
        assert!(!kept.words.contains("somewhere"), "{kept:?}");
    }
}

#[test]
fn an_instant_at_a_hidden_place_finds_nothing_beside_it() {
    let root = tempfile::tempdir().unwrap();
    seed(root.path(), 40.0);
    let mut reconciler = Reconciler::default();
    let report = reconcile(root.path(), &mut reconciler);
    assert!(report.failures.is_empty(), "{report:?}");
    let instant = |iso: &str| {
        chrono::DateTime::parse_from_rfc3339(iso)
            .unwrap()
            .timestamp()
    };
    let visit = instant("2026-10-01T21:30:00Z");
    let golden = instant("2026-10-01T20:02:00Z");
    let mut days = std::collections::BTreeMap::new();
    assert!(place_at(root.path(), visit, &mut days).is_some());
    store(vec![place("p", VISIT, None, true)])
        .write(root.path())
        .unwrap();
    let mut days = std::collections::BTreeMap::new();
    assert_eq!(place_at(root.path(), visit, &mut days), None);
    // Twin: the readings away from it are still beside their instant.
    let kept = place_at(root.path(), golden, &mut days).unwrap();
    assert!(kept.words.starts_with("near Golden"), "{kept:?}");
}

#[test]
fn a_damaged_places_store_describes_nothing_beside_an_activity() {
    let root = tempfile::tempdir().unwrap();
    seed(root.path(), 40.0);
    let mut reconciler = Reconciler::default();
    reconcile(root.path(), &mut reconciler);
    let visit = (at(15, 10), at(16, 0));
    assert!(
        DayBeside::read(root.path(), DAY)
            .beside(visit, None)
            .is_some()
    );
    fs::write(store_path(root.path()), b"{ not a store").unwrap();
    assert_eq!(DayBeside::read(root.path(), DAY).beside(visit, None), None);
    // Twin: repaired, it describes again.
    PlaceStore::default().write(root.path()).unwrap();
    assert!(
        DayBeside::read(root.path(), DAY)
            .beside(visit, None)
            .is_some()
    );
}

fn reconcile(root: &Path, reconciler: &mut Reconciler) -> crate::projection::ReconcileReport {
    reconciler.reconcile(root, tier(), None, &directory(), chrono_tz::America::Denver)
}

/// Visits to one spot on two days, which make a place.
fn seed_place_days(root: &Path) {
    let header = iphone_header("full", "light");
    for (day, iso) in [("20261001", "2026-10-01"), ("20261002", "2026-10-02")] {
        write(
            root,
            day,
            "phone_mobile-segment",
            "150000_300",
            &format!("{header}\n{}\n", visit_line(iso, 20.0, VISIT)),
        );
    }
    // A day elsewhere, which no hide reaches.
    write(
        root,
        "20261003",
        "phone_mobile-segment",
        "140000_300",
        &format!(
            "{header}\n{}\n",
            fix("2026-10-03T20:00:10Z", GOLDEN.0, GOLDEN.1, 10.0)
        ),
    );
}

fn document(root: &Path, day: &str) -> String {
    fs::read_to_string(document_path(root, day)).unwrap()
}

#[test]
fn hiding_and_showing_a_place_rewrites_each_of_its_days_once() {
    let root = tempfile::tempdir().unwrap();
    seed_place_days(root.path());
    let mut reconciler = Reconciler::default();
    let first = reconcile(root.path(), &mut reconciler);
    assert_eq!((first.written, first.failures.len()), (3, 0), "{first:?}");
    assert_eq!(PlaceStore::read(root.path()).places.len(), 1);
    let elsewhere = document(root.path(), "20261003");

    PlaceStore::update(root.path(), |store| store.places[0].hidden = true).unwrap();
    let hidden = reconcile(root.path(), &mut reconciler);
    assert_eq!(hidden.written, 2, "{hidden:?}");
    for day in ["20261001", "20261002"] {
        let text = document(root.path(), day);
        assert!(!text.contains("a visit"), "{day}: {text}");
        assert!(!text.contains("Denver"), "{day}: {text}");
    }
    assert_eq!(document(root.path(), "20261003"), elsewhere);
    assert_eq!(reconcile(root.path(), &mut reconciler).written, 0);
    // A fresh start finds every day's record current.
    assert_eq!(
        reconcile(root.path(), &mut Reconciler::default()).written,
        0
    );

    // Twin: shown again, each of its days is written once more, with its words.
    PlaceStore::update(root.path(), |store| store.places[0].hidden = false).unwrap();
    assert_eq!(reconcile(root.path(), &mut reconciler).written, 2);
    for day in ["20261001", "20261002"] {
        assert!(document(root.path(), day).contains("near Denver"), "{day}");
    }
    assert_eq!(reconcile(root.path(), &mut reconciler).written, 0);
}

#[test]
fn a_hidden_place_whose_centre_drifts_rewrites_no_day() {
    let root = tempfile::tempdir().unwrap();
    seed_place_days(root.path());
    let mut reconciler = Reconciler::default();
    reconcile(root.path(), &mut reconciler);
    PlaceStore::update(root.path(), |store| store.places[0].hidden = true).unwrap();
    assert_eq!(reconcile(root.path(), &mut reconciler).written, 2);
    let before = PlaceStore::read(root.path()).names_fingerprint();
    PlaceStore::update(root.path(), |store| {
        store.places[0].latitude += 0.0001;
        store.places[0].longitude -= 0.0001;
    })
    .unwrap();
    assert_eq!(PlaceStore::read(root.path()).names_fingerprint(), before);
    assert_eq!(reconcile(root.path(), &mut reconciler).written, 0);
    assert_eq!(
        reconcile(root.path(), &mut Reconciler::default()).written,
        0
    );
}

#[test]
fn with_no_place_hidden_the_fingerprint_is_the_names_alone() {
    use sha2::{Digest, Sha256};

    let named = store(vec![
        place("a", VISIT, Some("the gym"), false),
        place("b", STAY, None, false),
    ]);
    // The names-only fingerprint every day built before hides counted holds.
    let mut digest = Sha256::new();
    digest.update(b"a\0the gym\0");
    let names_only = format!("{:x}", digest.finalize());
    assert_eq!(named.names_fingerprint(), names_only);
    // Moving a place that isn't hidden changes nothing either.
    let mut moved = named.clone();
    moved.places[1].latitude += 0.01;
    assert_eq!(moved.names_fingerprint(), names_only);
    // Twin: hiding any place, named or not, changes it.
    let mut hidden = named.clone();
    hidden.places[1].hidden = true;
    assert_ne!(hidden.names_fingerprint(), names_only);
    let mut other = named;
    other.places[0].hidden = true;
    assert_ne!(other.names_fingerprint(), hidden.names_fingerprint());
}

#[test]
fn a_damaged_places_store_renders_no_day_and_the_documents_stay() {
    let root = tempfile::tempdir().unwrap();
    seed_place_days(root.path());
    let mut reconciler = Reconciler::default();
    assert_eq!(reconcile(root.path(), &mut reconciler).written, 3);
    let kept = document(root.path(), "20261001");
    let damaged = b"{ not a store";
    fs::write(store_path(root.path()), damaged).unwrap();
    // A new day and a changed day are both waiting.
    let header = iphone_header("full", "light");
    write(
        root.path(),
        "20261004",
        "phone_mobile-segment",
        "140000_300",
        &format!(
            "{header}\n{}\n",
            fix("2026-10-04T20:00:10Z", GOLDEN.0, GOLDEN.1, 10.0)
        ),
    );
    solstone_core_journal_io::bump_stream_marker(root.path(), "20261001").unwrap();
    let report = reconcile(root.path(), &mut Reconciler::default());
    assert_eq!((report.written, report.removed), (0, 0), "{report:?}");
    assert!(!report.failures.is_empty(), "{report:?}");
    assert!(!document_path(root.path(), "20261004").exists());
    assert_eq!(document(root.path(), "20261001"), kept);
    assert_eq!(fs::read(store_path(root.path())).unwrap(), damaged);

    // Twin: once it reads again, the waiting days are written.
    PlaceStore::default().write(root.path()).unwrap();
    let repaired = reconcile(root.path(), &mut Reconciler::default());
    assert!(repaired.failures.is_empty(), "{repaired:?}");
    assert!(document_path(root.path(), "20261004").is_file());
}
