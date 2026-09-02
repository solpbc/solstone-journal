// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(all(test, feature = "full-tests"))]
#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use chrono::NaiveDate;

use crate::store_tests::TempDir;
use crate::{
    FACET_CANDIDATE_MIN_SEGMENTS, FACET_CANDIDATE_WINDOW_DAYS, SpeculativeFacetSample,
    aggregate_speculative_facets,
};

fn day(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

fn segment_dir(root: &Path, day: &str, stream: Option<&str>, segment: &str) -> PathBuf {
    let mut path = root.join("chronicle").join(day);
    if let Some(stream) = stream {
        path.push(stream);
    }
    path.join(segment)
}

fn write_sense(root: &Path, day: &str, stream: Option<&str>, segment: &str, contents: &[u8]) {
    let talents = segment_dir(root, day, stream, segment).join("talents");
    fs::create_dir_all(&talents).unwrap();
    fs::write(talents.join("sense.json"), contents).unwrap();
}

fn snapshot_tree(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    fn visit(root: &Path, current: &Path, snapshot: &mut Vec<(PathBuf, Vec<u8>)>) {
        let mut entries: Vec<_> = fs::read_dir(current).unwrap().map(Result::unwrap).collect();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            if path.is_dir() {
                visit(root, &path, snapshot);
            } else {
                snapshot.push((
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    fs::read(path).unwrap(),
                ));
            }
        }
    }

    let mut snapshot = Vec::new();
    visit(root, root, &mut snapshot);
    snapshot
}

#[test]
fn aggregation_is_side_effect_free_over_multiple_days() {
    let temporary = TempDir::new();
    write_sense(
        temporary.path(),
        "20260808",
        Some("archon"),
        "090000_300",
        br#"{"speculative_facet":"Home Reno"}"#,
    );
    write_sense(
        temporary.path(),
        "20260809",
        None,
        "100000_300",
        br#"{"speculative_facet":"Home Reno"}"#,
    );
    let before = snapshot_tree(temporary.path());

    let candidates = aggregate_speculative_facets(temporary.path(), day(2026, 8, 10), 1).unwrap();

    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].count, 2);
    assert_eq!(snapshot_tree(temporary.path()), before);
}

#[test]
fn aggregation_uses_the_injected_local_day_window() {
    let temporary = TempDir::new();
    write_sense(
        temporary.path(),
        "20260727",
        Some("archon"),
        "090000_300",
        br#"{"speculative_facet":"Boundary"}"#,
    );
    write_sense(
        temporary.path(),
        "20260726",
        Some("archon"),
        "100000_300",
        br#"{"speculative_facet":"Too Old"}"#,
    );

    let candidates = aggregate_speculative_facets(temporary.path(), day(2026, 8, 10), 1).unwrap();

    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].name, "Boundary");
    assert_eq!(candidates[0].window_days, FACET_CANDIDATE_WINDOW_DAYS);
}

#[test]
fn aggregation_surfaces_only_candidates_at_the_minimum_segment_count() {
    let temporary = TempDir::new();
    for segment in ["090000_300", "093000_300", "100000_300"] {
        write_sense(
            temporary.path(),
            "20260810",
            Some("archon"),
            segment,
            br#"{"speculative_facet":"Recurring"}"#,
        );
    }
    for segment in ["103000_300", "110000_300"] {
        write_sense(
            temporary.path(),
            "20260810",
            Some("archon"),
            segment,
            br#"{"speculative_facet":"One Off"}"#,
        );
    }

    let candidates = aggregate_speculative_facets(
        temporary.path(),
        day(2026, 8, 10),
        FACET_CANDIDATE_MIN_SEGMENTS,
    )
    .unwrap();

    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].name, "Recurring");
    assert_eq!(candidates[0].count, FACET_CANDIDATE_MIN_SEGMENTS);
}

#[test]
fn aggregation_keeps_the_first_three_samples_in_segment_order() {
    let temporary = TempDir::new();
    for segment in [
        "090000_300",
        "093000_300",
        "100000_300",
        "103000_300",
        "110000_300",
    ] {
        write_sense(
            temporary.path(),
            "20260810",
            Some("archon"),
            segment,
            br#"{"speculative_facet":"Home Reno"}"#,
        );
    }

    let candidates = aggregate_speculative_facets(temporary.path(), day(2026, 8, 10), 1).unwrap();

    assert_eq!(candidates[0].count, 5);
    assert_eq!(
        candidates[0].samples,
        vec![
            SpeculativeFacetSample {
                day: "20260810".to_owned(),
                stream: "archon".to_owned(),
                segment: "090000_300".to_owned(),
                unrepresentable: false,
            },
            SpeculativeFacetSample {
                day: "20260810".to_owned(),
                stream: "archon".to_owned(),
                segment: "093000_300".to_owned(),
                unrepresentable: false,
            },
            SpeculativeFacetSample {
                day: "20260810".to_owned(),
                stream: "archon".to_owned(),
                segment: "100000_300".to_owned(),
                unrepresentable: false,
            },
        ]
    );
}

#[cfg(target_os = "linux")]
#[test]
fn non_utf8_sample_is_kept_and_marked_unrepresentable() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let temporary = TempDir::new();
    let talents = temporary
        .path()
        .join("chronicle/20260810")
        .join(OsStr::from_bytes(b"s\xff"))
        .join("090000_300")
        .join("talents");
    fs::create_dir_all(&talents).unwrap();
    fs::write(
        talents.join("sense.json"),
        br#"{"speculative_facet":"Home Reno"}"#,
    )
    .unwrap();

    let candidates = aggregate_speculative_facets(temporary.path(), day(2026, 8, 10), 1).unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].count, 1);
    assert_eq!(
        candidates[0].samples,
        vec![SpeculativeFacetSample {
            day: "20260810".to_owned(),
            stream: String::new(),
            segment: "090000_300".to_owned(),
            unrepresentable: true,
        }]
    );
}

#[cfg(target_os = "linux")]
#[test]
fn unrepresentable_sample_is_visible_after_representable_cap() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let temporary = TempDir::new();
    for segment in ["090000_300", "093000_300", "100000_300"] {
        write_sense(
            temporary.path(),
            "20260810",
            Some("archon"),
            segment,
            br#"{"speculative_facet":"Home Reno"}"#,
        );
    }
    let talents = temporary
        .path()
        .join("chronicle/20260810")
        .join(OsStr::from_bytes(b"s\xff"))
        .join("110000_300")
        .join("talents");
    fs::create_dir_all(&talents).unwrap();
    fs::write(
        talents.join("sense.json"),
        br#"{"speculative_facet":"Home Reno"}"#,
    )
    .unwrap();

    let candidates = aggregate_speculative_facets(temporary.path(), day(2026, 8, 10), 1).unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].count, 4);
    assert!(
        candidates[0]
            .samples
            .iter()
            .any(|sample| sample.unrepresentable),
        "unrepresentable evidence vanished behind the sample cap: {:?}",
        candidates[0].samples
    );
}

#[test]
fn aggregation_groups_whitespace_and_unicode_casefolded_names() {
    let temporary = TempDir::new();
    for (segment, name) in [
        ("090000_300", "Side  Project"),
        ("093000_300", " side\\u001cproject "),
        ("100000_300", "SIDE PROJECT"),
        ("103000_300", "Straße"),
        ("110000_300", "STRASSE"),
    ] {
        write_sense(
            temporary.path(),
            "20260810",
            Some("archon"),
            segment,
            format!(r#"{{"speculative_facet":"{name}"}}"#).as_bytes(),
        );
    }

    let candidates = aggregate_speculative_facets(temporary.path(), day(2026, 8, 10), 2).unwrap();

    assert_eq!(candidates.len(), 2);
    assert_eq!(candidates[0].name, "Side Project");
    assert_eq!(candidates[0].count, 3);
    let street = candidates
        .iter()
        .find(|candidate| candidate.name == "Straße")
        .unwrap();
    assert_eq!(street.count, 2);
    assert_eq!(street.name_key, caseless::default_case_fold_str("Straße"));
}

#[test]
fn aggregation_skips_unreadable_and_invalid_segment_outputs_without_aborting() {
    let temporary = TempDir::new();
    let root = temporary.path();
    fs::create_dir_all(segment_dir(root, "20260810", Some("archon"), "090000_300").join("talents"))
        .unwrap();
    write_sense(root, "20260810", Some("archon"), "093000_300", b"not json");
    write_sense(
        root,
        "20260810",
        Some("archon"),
        "100000_300",
        br#"[1,2,3]"#,
    );
    write_sense(
        root,
        "20260810",
        Some("archon"),
        "103000_300",
        br#"{"speculative_facet":42}"#,
    );
    write_sense(
        root,
        "20260810",
        Some("archon"),
        "110000_300",
        br#"{"speculative_facet":null}"#,
    );
    let unreadable = segment_dir(root, "20260810", Some("archon"), "113000_300").join("talents");
    fs::create_dir_all(unreadable.join("sense.json")).unwrap();
    let permission_denied =
        segment_dir(root, "20260810", Some("archon"), "120000_300").join("talents");
    fs::create_dir_all(&permission_denied).unwrap();
    let permission_file = permission_denied.join("sense.json");
    fs::write(&permission_file, br#"{"speculative_facet":42}"#).unwrap();
    #[cfg(unix)]
    fs::set_permissions(&permission_file, fs::Permissions::from_mode(0o000)).unwrap();
    write_sense(
        root,
        "20260810",
        Some("archon"),
        "123000_300",
        br#"{"speculative_facet":"Survives"}"#,
    );

    let candidates = aggregate_speculative_facets(root, day(2026, 8, 10), 1).unwrap();

    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].name, "Survives");
    assert_eq!(candidates[0].count, 1);
}
