// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Reading a real segment directory into the facts the predicate decides on.
//!
//! ⚠ These live in `tests/` rather than beside the code because
//! `tests/architecture.rs` forbids any `src/` module from naming a removal or rename
//! primitive, and a filesystem bed has to tear itself down. That guard is worth more
//! than the convenience of co-location: it is what keeps the removal surface one
//! named file.

#![allow(
    clippy::disallowed_methods,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    reason = "bed setup and teardown; the crate-wide bans exist to constrain the \
              production verbs, and they fired here, which is them working"
)]

use std::fs;

use solstone_core_processing_record::vocab;

use serde_json::Value;
use solstone_core_retention::content::{ClosedHandlerSet, ContentName, JournalMedia};
use solstone_core_retention::scan::{READ_BOUND, read_sidecar, scan_segment, sidecar_name};

fn bed(name: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!(
        "retention-scan-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("a clock")
            .as_nanos()
    ));
    fs::create_dir_all(&root).expect("a bed");
    root
}

fn content(name: &str) -> ContentName {
    ContentName::new(name).expect("a name")
}

#[test]
fn a_sidecar_name_replaces_the_final_extension() {
    assert_eq!(
        sidecar_name(&content("audio.flac")).as_deref(),
        Some("audio.jsonl")
    );
    assert_eq!(
        sidecar_name(&content("chunk_audio.flac")).as_deref(),
        Some("chunk_audio.jsonl")
    );
    assert_eq!(
        sidecar_name(&content("a.b.mp4")).as_deref(),
        Some("a.b.jsonl")
    );
    assert_eq!(sidecar_name(&content("noextension")), None);
    assert_eq!(sidecar_name(&content(".flac")), None);
}

/// 🔴 The defect the strict rule exists to prevent.
#[test]
fn a_marker_key_in_the_header_does_not_prove_analysis_rows() {
    let root = bed("header-marker");
    let path = root.join("audio.jsonl");
    // Segment-wide metadata merged a `start` key into the header, and there is
    // no second row at all.
    fs::write(&path, b"{\"segment\": \"x\", \"start\": 0.0}\n").expect("write");

    let facts = read_sidecar(&path, Some(vocab::AUDIO_TRANSCRIPT_ROW_KEY));
    assert!(
        !facts.has_analysis_row,
        "a header-only file must never look chunk-bearing"
    );

    // The positive control: the same key on a real second row does prove it.
    fs::write(
        &path,
        b"{\"segment\": \"x\"}\n{\"start\": 0.0, \"text\": \"hi\"}\n",
    )
    .expect("write");
    assert!(read_sidecar(&path, Some(vocab::AUDIO_TRANSCRIPT_ROW_KEY)).has_analysis_row);

    fs::remove_dir_all(&root).expect("teardown");
}

#[test]
fn blank_lines_do_not_count_as_the_second_row() {
    let root = bed("blank-lines");
    let path = root.join("audio.jsonl");
    fs::write(&path, b"{\"segment\": \"x\"}\n\n   \n{\"start\": 0.0}\n").expect("write");
    assert!(
        read_sidecar(&path, Some(vocab::AUDIO_TRANSCRIPT_ROW_KEY)).has_analysis_row,
        "blank lines are skipped, so the real row is still the second one"
    );
    fs::remove_dir_all(&root).expect("teardown");
}

#[test]
fn a_record_is_read_from_the_header_and_must_be_an_object() {
    let root = bed("record");
    let path = root.join("audio.jsonl");

    fs::write(
        &path,
        format!(
            "{{\"_solstone_processing\": {{\"schema\": \"{}\", \"state\": \"analyzed\"}}}}\n",
            vocab::SCHEMA
        ),
    )
    .expect("write");
    let facts = read_sidecar(&path, None);
    assert_eq!(
        facts
            .record
            .as_ref()
            .and_then(|r| r.get("state"))
            .and_then(Value::as_str),
        Some("analyzed")
    );

    // A non-object record is no record.
    fs::write(&path, b"{\"_solstone_processing\": \"analyzed\"}\n").expect("write");
    assert!(read_sidecar(&path, None).record.is_none());

    // A non-object row is no header.
    fs::write(&path, b"[1, 2, 3]\n").expect("write");
    assert!(read_sidecar(&path, None).record.is_none());

    fs::remove_dir_all(&root).expect("teardown");
}

#[test]
fn an_absent_or_unreadable_sidecar_yields_no_evidence() {
    let root = bed("absent");
    let facts = read_sidecar(&root.join("nosuch.jsonl"), Some("start"));
    assert!(facts.record.is_none());
    assert!(!facts.has_analysis_row);

    // Invalid UTF-8 is unreadable, not empty-but-fine.
    let path = root.join("bad.jsonl");
    fs::write(&path, [0xff, 0xfe, b'\n']).expect("write");
    assert!(read_sidecar(&path, Some("start")).record.is_none());

    fs::remove_dir_all(&root).expect("teardown");
}

/// A header past the byte bound loses its record; it does not gain one.
#[test]
fn a_header_beyond_the_read_bound_yields_no_record() {
    let root = bed("huge");
    let path = root.join("audio.jsonl");
    let padding = "x".repeat(READ_BOUND as usize + 1024);
    fs::write(
        &path,
        format!("{{\"pad\": \"{padding}\", \"_solstone_processing\": {{}}}}\n"),
    )
    .expect("write");
    assert!(
        read_sidecar(&path, None).record.is_none(),
        "a truncated header cannot parse, so it cannot manufacture a record"
    );
    fs::remove_dir_all(&root).expect("teardown");
}

#[test]
fn a_scan_finds_media_and_ignores_everything_else() {
    let root = bed("scan");
    for name in [
        "audio.flac",
        "screen.mp4",
        "photo.png",
        "audio.jsonl",
        "screen.jsonl",
        "tombstone.json",
        "notes.txt",
    ] {
        fs::write(root.join(name), b"bytes").expect("write");
    }
    fs::create_dir(root.join("subdir")).expect("dir");

    let found = scan_segment(&root, &ClosedHandlerSet, &JournalMedia);
    let mut names: Vec<&str> = found.iter().map(|item| item.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        vec!["audio.flac", "photo.png", "screen.mp4"],
        "sidecars, metadata, text and directories are not owner media"
    );
    for item in &found {
        assert_eq!(item.size, 5, "the size is the size on disk");
    }
    fs::remove_dir_all(&root).expect("teardown");
}

/// The scanner routes each modality's own marker key.
#[test]
fn each_modality_gets_its_own_marker_key() {
    let root = bed("markers");
    fs::write(root.join("audio.flac"), b"bytes").expect("write");
    fs::write(root.join("screen.mp4"), b"bytes").expect("write");
    // Each sidecar carries the OTHER modality's key on its second row.
    fs::write(
        root.join("audio.jsonl"),
        b"{\"segment\": \"x\"}\n{\"timestamp\": 1}\n",
    )
    .expect("write");
    fs::write(
        root.join("screen.jsonl"),
        b"{\"segment\": \"x\"}\n{\"start\": 0.0}\n",
    )
    .expect("write");

    let found = scan_segment(&root, &ClosedHandlerSet, &JournalMedia);
    for item in &found {
        assert!(
            !item.sidecar.has_analysis_row,
            "{} accepted the other modality's marker key",
            item.name.as_str()
        );
    }
    fs::remove_dir_all(&root).expect("teardown");
}

/// An empty directory yields nothing rather than failing.
#[test]
fn an_empty_or_absent_segment_yields_no_content() {
    let root = bed("empty");
    assert!(scan_segment(&root, &ClosedHandlerSet, &JournalMedia).is_empty());
    assert!(scan_segment(&root.join("nosuch"), &ClosedHandlerSet, &JournalMedia).is_empty());
    fs::remove_dir_all(&root).expect("teardown");
}
