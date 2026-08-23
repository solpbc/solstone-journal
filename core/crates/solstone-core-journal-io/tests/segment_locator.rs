// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use std::fs;

use solstone_core_journal_io::{
    DEFAULT_STREAM, PathOrDay, SegmentLayout, iter_segments, resolve_segment_locator_exact,
};

const DAY: &str = "20240103";

#[test]
fn public_surface_resolves_layouts_and_exposes_locator_identity_fields() {
    let temporary = tempfile::TempDir::new().unwrap();
    let journal = temporary.path().join("journal");
    let direct = journal.join("chronicle").join(DAY).join("080000_300");
    let named_default = journal
        .join("chronicle")
        .join(DAY)
        .join(DEFAULT_STREAM)
        .join("090000_300");
    let named_main = journal
        .join("chronicle")
        .join(DAY)
        .join("main")
        .join("093000_300_summary");
    fs::create_dir_all(&direct).unwrap();
    fs::create_dir_all(&named_default).unwrap();
    fs::create_dir_all(&named_main).unwrap();

    assert_eq!(
        resolve_segment_locator_exact(
            &journal,
            DAY,
            DEFAULT_STREAM,
            "080000_300",
            SegmentLayout::Direct,
        )
        .unwrap()
        .as_deref(),
        Some(direct.as_path())
    );
    assert_eq!(
        resolve_segment_locator_exact(
            &journal,
            DAY,
            DEFAULT_STREAM,
            "090000_300",
            SegmentLayout::Named,
        )
        .unwrap()
        .as_deref(),
        Some(named_default.as_path())
    );
    assert_eq!(
        resolve_segment_locator_exact(
            &journal,
            DAY,
            "main",
            "093000_300_summary",
            SegmentLayout::Named,
        )
        .unwrap()
        .as_deref(),
        Some(named_main.as_path())
    );

    let segments = iter_segments(&journal, PathOrDay::Day(DAY)).unwrap();
    assert_eq!(segments.len(), 3);
    for segment in &segments {
        let identity = segment.locator_identity().unwrap();
        if segment.stream().is_direct() {
            assert_eq!(identity.layout, SegmentLayout::Direct);
            assert_eq!(identity.stream, DEFAULT_STREAM);
            assert_eq!(identity.key, "080000_300");
            assert_eq!(identity.name, "080000_300");
        } else if segment.stream().directory() == Some(std::ffi::OsStr::new(DEFAULT_STREAM)) {
            assert_eq!(identity.layout, SegmentLayout::Named);
            assert_eq!(identity.stream, DEFAULT_STREAM);
            assert_eq!(identity.key, "090000_300");
            assert_eq!(identity.name, "090000_300");
        } else {
            assert_eq!(identity.layout, SegmentLayout::Named);
            assert_eq!(identity.stream, "main");
            assert_eq!(identity.key, "093000_300");
            assert_eq!(identity.name, "093000_300_summary");
        }
    }
}
