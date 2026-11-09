// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::{Path, PathBuf};

use serde_json::json;

use super::*;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/body_old_bundles")
        .join(name)
}

fn copy(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap().flatten() {
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

const NATIVE: &str = "body-01M70HDTD9N4P15X06CRDG2DWZ";

fn with_native(root: &Path) {
    copy(
        &fixture("native").join(NATIVE),
        &root.join("imports").join(NATIVE),
    );
}

fn with_python(root: &Path) {
    copy(
        &fixture("python_era/20260304_080000"),
        &root.join("imports/20260304_080000"),
    );
}

fn with_python_named(root: &Path, id: &str, family: &str, months: &[&str]) {
    let dir = root.join("imports").join(id);
    fs::create_dir_all(dir.join("normalized")).unwrap();
    fs::write(
        dir.join("manifest.json"),
        json!({"source_type": family}).to_string(),
    )
    .unwrap();
    for month in months {
        fs::write(dir.join("normalized").join(format!("{month}.jsonl")), "").unwrap();
    }
}

fn new_import(
    root: &Path,
    id: &str,
    family: &str,
    span: Option<serde_json::Value>,
    listed: &[&str],
) {
    let dir = root.join("imports").join(id);
    fs::create_dir_all(&dir).unwrap();
    let mut manifest = json!({"source_type": family, "entry_count": 1});
    if let Some(span) = span {
        manifest["span"] = span;
    }
    fs::write(dir.join("manifest.json"), manifest.to_string()).unwrap();
    let lines = listed
        .iter()
        .map(|day| json!({"day": day, "windows": []}).to_string())
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(dir.join(TOUCHED_FILE), lines).unwrap();
}

fn window(root: &Path, day: &str, stream: &str, key: &str) -> PathBuf {
    let dir = root.join("chronicle").join(day).join(stream).join(key);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn covers_at(root: &Path, day: &str) -> Vec<(OldSource, CoverScope)> {
    covers_on_day(root, day, None)
        .into_iter()
        .map(|cover| (cover.source, cover.scope))
        .collect()
}

#[test]
fn days_an_old_bundle_covers_leave_out_one_a_new_import_read() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    with_native(root);
    let days = ["20260101", "20270101"];
    assert_eq!(
        covered_days_among(root, days.into_iter()),
        BTreeSet::from(["20260101".to_owned()])
    );
    let import = root.join("imports/20260201_120000");
    fs::create_dir_all(&import).unwrap();
    fs::write(import.join(TOUCHED_FILE), "").unwrap();
    fs::write(
        import.join(MANIFEST_FILE),
        json!({"source_type": "apple_health", "span": [["20260101", "20260102"]]}).to_string(),
    )
    .unwrap();
    assert!(covered_days_among(root, days.into_iter()).is_empty());
}

#[test]
fn a_new_path_import_alone_is_not_an_old_bundle_and_says_nothing() {
    let root = tempfile::tempdir().unwrap();
    let import = root.path().join("imports/20260110_120000");
    fs::create_dir_all(import.join("normalized")).unwrap();
    // The manifest source family alone wouldn't tell: the new path says
    // `apple_health` too.
    fs::write(
        import.join("manifest.json"),
        json!({"source_type": "apple_health", "days_affected": ["20260110"]}).to_string(),
    )
    .unwrap();
    fs::write(import.join(TOUCHED_FILE), "").unwrap();
    assert!(read_old_bundles(root.path()).is_empty());
    assert!(covers_at(root.path(), "20260110").is_empty());

    fs::remove_file(import.join(TOUCHED_FILE)).unwrap();
    fs::write(import.join(PROGRESS_FILE), "{}").unwrap();
    assert!(read_old_bundles(root.path()).is_empty());

    fs::remove_file(import.join(PROGRESS_FILE)).unwrap();
    assert_eq!(
        read_old_bundles(root.path()).len(),
        1,
        "no new-path files: an old bundle"
    );
}

#[test]
fn an_unknown_family_or_unreadable_manifest_is_not_a_body_import() {
    let root = tempfile::tempdir().unwrap();
    with_python_named(root.path(), "garmin", "garmin_connect", &["2026-01"]);
    let broken = root.path().join("imports/broken");
    fs::create_dir_all(broken.join("normalized")).unwrap();
    fs::write(broken.join("manifest.json"), "not json").unwrap();
    assert!(read_old_bundles(root.path()).is_empty());
}

#[test]
fn a_native_bundle_lists_days_and_a_python_era_one_months_reading_only_names() {
    let root = tempfile::tempdir().unwrap();
    with_native(root.path());
    with_python(root.path());
    let bundles = read_old_bundles(root.path());
    assert_eq!(bundles.len(), 2);
    let python = bundles
        .iter()
        .find(|bundle| bundle.id == "20260304_080000")
        .unwrap();
    assert_eq!(python.source, OldSource::AppleHealth);
    assert_eq!(
        python.span,
        OldSpan::Months(BTreeSet::from(["2026-03".to_owned()]))
    );
    let native = bundles.iter().find(|bundle| bundle.id == NATIVE).unwrap();
    assert_eq!(native.source, OldSource::AppleHealth);
    let OldSpan::Days(days) = &native.span else {
        panic!("a native bundle says days: {native:?}");
    };
    assert!(days.contains("20260101") && days.contains("20260102"));
    assert_eq!(days.len(), 2);

    // A month file's rows are never opened: garbage there changes nothing.
    fs::write(
        root.path()
            .join("imports/20260304_080000/normalized/2026-03.jsonl"),
        [0xff_u8, 0xfe],
    )
    .unwrap();
    assert_eq!(read_old_bundles(root.path()).len(), 2);
}

#[test]
fn an_oura_api_bundle_counts_as_oura_and_dexcom_clarity_is_listed_with_no_importer() {
    let root = tempfile::tempdir().unwrap();
    with_python_named(root.path(), "20251101_000000", "oura_api", &["2025-11"]);
    with_python_named(
        root.path(),
        "20251101_000001",
        "dexcom_clarity",
        &["2025-11"],
    );
    let bundles = read_old_bundles(root.path());
    assert_eq!(bundles[0].source, OldSource::Oura);
    assert_eq!(bundles[1].source, OldSource::DexcomClarity);
    assert_eq!(OldSource::DexcomClarity.stream(), None);
    // A Dexcom bundle has nothing to touch it, so its line never retires.
    let scoped = covers_at(root.path(), "20251110");
    assert_eq!(scoped.len(), 2);
    assert_eq!(scoped[1].1, CoverScope::Month("2025-11".to_owned()));
    window(root.path(), "20251110", "import.oura", "090000_3600");
    let scoped = covers_at(root.path(), "20251110");
    assert_eq!(
        scoped.iter().map(|(source, _)| *source).collect::<Vec<_>>(),
        vec![OldSource::DexcomClarity]
    );
}

#[test]
fn a_python_era_bundle_says_the_month_and_never_calls_a_day_covered_by_name() {
    let root = tempfile::tempdir().unwrap();
    with_python(root.path());
    let covers = covers_at(root.path(), "20260311");
    assert_eq!(
        covers,
        vec![(
            OldSource::AppleHealth,
            CoverScope::Month("2026-03".to_owned())
        )]
    );
    assert_eq!(month_words("2026-03"), Some("March 2026".to_owned()));
    assert!(covers_at(root.path(), "20260401").is_empty());
}

#[test]
fn a_day_reimported_loses_its_line_and_keeps_it_lost_after_a_delete() {
    let root = tempfile::tempdir().unwrap();
    with_native(root.path());
    assert_eq!(covers_at(root.path(), "20260101").len(), 1);
    let window_dir = window(
        root.path(),
        "20260101",
        "import.apple_health",
        "090000_3600",
    );
    assert!(covers_at(root.path(), "20260101").is_empty());
    // Deleted by the owner: a tombstone stays where the window was.
    fs::remove_dir_all(&window_dir).unwrap();
    fs::create_dir_all(&window_dir).unwrap();
    fs::write(window_dir.join("tombstone.json"), "{}").unwrap();
    assert!(covers_at(root.path(), "20260101").is_empty());
    // No tombstone either, but the day is in the import's own listing.
    fs::remove_dir_all(window_dir.parent().unwrap()).unwrap();
    assert_eq!(covers_at(root.path(), "20260101").len(), 1);
    new_import(
        root.path(),
        "20260201_000000",
        "apple_health",
        None,
        &["20260101"],
    );
    assert!(covers_at(root.path(), "20260101").is_empty());
}

#[test]
fn an_import_of_another_source_doesnt_retire_a_sources_line() {
    let root = tempfile::tempdir().unwrap();
    with_native(root.path());
    new_import(root.path(), "20260201_000000", "oura", None, &["20260101"]);
    window(root.path(), "20260101", "import.oura", "090000_3600");
    let covers = covers_at(root.path(), "20260101");
    assert_eq!(covers.len(), 1);
    assert_eq!(covers[0].0, OldSource::AppleHealth);
}

#[test]
fn a_month_reimported_whole_retires_every_day_even_ones_with_no_rows() {
    let root = tempfile::tempdir().unwrap();
    with_python_named(root.path(), "20260101_080000", "apple_health", &["2026-02"]);
    new_import(
        root.path(),
        "20260301_000000",
        "apple_health",
        Some(json!([["20260201", "20260228"]])),
        &[],
    );
    for day in ["20260201", "20260215", "20260228"] {
        assert!(covers_at(root.path(), day).is_empty(), "{day}");
    }
}

#[test]
fn an_oura_range_starting_mid_month_leaves_the_earlier_days() {
    let root = tempfile::tempdir().unwrap();
    with_python_named(root.path(), "20251201_080000", "oura_api", &["2025-11"]);
    new_import(
        root.path(),
        "20260101_000000",
        "oura",
        Some(json!([["20251115", "20251130"]])),
        &[],
    );
    assert_eq!(
        covers_at(root.path(), "20251114"),
        vec![(OldSource::Oura, CoverScope::Month("2025-11".to_owned()))]
    );
    assert!(covers_at(root.path(), "20251115").is_empty());
    assert!(covers_at(root.path(), "20251130").is_empty());
}

#[test]
fn an_oura_range_with_a_failed_middle_chunk_keeps_the_line_on_those_days() {
    let root = tempfile::tempdir().unwrap();
    with_python_named(root.path(), "20251201_080000", "oura_api", &["2025-11"]);
    new_import(
        root.path(),
        "20260101_000000",
        "oura",
        Some(json!([["20251101", "20251110"], ["20251121", "20251130"]])),
        &[],
    );
    assert!(covers_at(root.path(), "20251105").is_empty());
    assert_eq!(covers_at(root.path(), "20251115").len(), 1);
    assert!(covers_at(root.path(), "20251121").is_empty());
}

#[test]
fn a_span_on_a_new_import_of_another_source_doesnt_count() {
    let root = tempfile::tempdir().unwrap();
    with_python_named(root.path(), "20251201_080000", "oura_api", &["2025-11"]);
    new_import(
        root.path(),
        "20260101_000000",
        "apple_health",
        Some(json!([["20251101", "20251130"]])),
        &[],
    );
    assert_eq!(covers_at(root.path(), "20251105").len(), 1);
}

#[test]
fn a_new_import_reads_its_source_from_its_manifest_or_its_record_and_a_stopped_one_counts() {
    let root = tempfile::tempdir().unwrap();
    with_native(root.path());
    let stopped = root.path().join("imports/20260201_000000");
    fs::create_dir_all(&stopped).unwrap();
    fs::write(stopped.join(PROGRESS_FILE), "{}").unwrap();
    fs::write(
        stopped.join(IMPORT_RECORD),
        json!({"source_hint": "apple_health"}).to_string(),
    )
    .unwrap();
    fs::write(
        stopped.join(TOUCHED_FILE),
        json!({"day": "20260102", "windows": []}).to_string(),
    )
    .unwrap();
    assert_eq!(read_new_imports(root.path()).len(), 1);
    assert!(covers_at(root.path(), "20260102").is_empty());
    assert_eq!(covers_at(root.path(), "20260101").len(), 1);
}

#[test]
fn only_narrows_to_one_source() {
    let root = tempfile::tempdir().unwrap();
    with_native(root.path());
    with_python_named(root.path(), "20251201_080000", "oura_api", &["2026-01"]);
    assert_eq!(covers_at(root.path(), "20260101").len(), 2);
    let oura = covers_on_day(root.path(), "20260101", Some(OldSource::Oura));
    assert_eq!(oura.len(), 1);
    assert_eq!(oura[0].source, OldSource::Oura);
}
