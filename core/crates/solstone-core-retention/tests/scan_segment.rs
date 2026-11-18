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

#[test]
fn checked_scan_distinguishes_empty_from_incomplete_observations() {
    use solstone_core_retention::scan::scan_segment_checked;
    let root = tempfile::tempdir().unwrap();
    assert!(
        scan_segment_checked(root.path(), &ClosedHandlerSet, &JournalMedia)
            .unwrap()
            .files()
            .is_empty()
    );
    assert!(
        scan_segment_checked(
            &root.path().join("missing"),
            &ClosedHandlerSet,
            &JournalMedia
        )
        .is_err()
    );
    fs::write(root.path().join("not_directory"), b"bytes").unwrap();
    assert!(
        scan_segment_checked(
            &root.path().join("not_directory"),
            &ClosedHandlerSet,
            &JournalMedia
        )
        .is_err()
    );
    fs::write(root.path().join("audio.flac"), b"bytes").unwrap();
    assert_eq!(
        scan_segment_checked(root.path(), &ClosedHandlerSet, &JournalMedia)
            .unwrap()
            .files()
            .len(),
        1
    );
    fs::create_dir(root.path().join("audio.jsonl")).unwrap();
    assert!(scan_segment_checked(root.path(), &ClosedHandlerSet, &JournalMedia).is_err());
    fs::remove_dir(root.path().join("audio.jsonl")).unwrap();
    fs::write(root.path().join("audio.jsonl"), [0xff, b'\n']).unwrap();
    assert!(scan_segment_checked(root.path(), &ClosedHandlerSet, &JournalMedia).is_err());
}

#[cfg(unix)]
#[test]
fn checked_scan_refuses_media_symlinks_instead_of_omitting_siblings() {
    use solstone_core_retention::scan::scan_segment_checked;
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("target"), b"bytes").unwrap();
    std::os::unix::fs::symlink("target", root.path().join("audio.flac")).unwrap();
    assert!(scan_segment_checked(root.path(), &ClosedHandlerSet, &JournalMedia).is_err());
}

mod bound {
    use super::*;
    use std::ffi::OsStr;
    use std::path::Path;
    use std::sync::Arc;

    use solstone_core_journal_io::JournalRoot;
    use solstone_core_journal_io::media::{MediaDirectory, MediaPrimitive, run_with_media_hook};
    use solstone_core_retention::scan::{scan_segment_bound, scan_segment_checked};

    struct Bed {
        temp: tempfile::TempDir,
        directory: Arc<MediaDirectory>,
    }

    impl Bed {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            fs::create_dir(temp.path().join("segment")).unwrap();
            let root =
                Arc::new(JournalRoot::open(&fs::canonicalize(temp.path()).unwrap()).unwrap());
            let directory = MediaDirectory::open(root, Path::new("segment")).unwrap();
            Self { temp, directory }
        }

        fn path(&self) -> std::path::PathBuf {
            self.temp.path().join("segment")
        }

        fn write(&self, name: &str, bytes: impl AsRef<[u8]>) {
            fs::write(self.path().join(name), bytes).unwrap();
        }

        fn scan(
            &self,
            maximum: usize,
        ) -> std::io::Result<solstone_core_retention::scan::CheckedSegment> {
            scan_segment_bound(&self.directory, &ClosedHandlerSet, &JournalMedia, maximum)
        }
    }

    #[test]
    fn bound_empty_exact_cap_and_all_entry_overflow_are_distinct() {
        let bed = Bed::new();
        assert!(bed.scan(0).unwrap().files().is_empty());
        bed.write("audio.flac", b"1234567");
        bed.write("photo.png", b"12345");
        bed.write("screen.mp4", b"123");
        bed.write("notes.txt", b"ignored");
        fs::create_dir(bed.path().join("subdir")).unwrap();
        assert!(bed.scan(0).is_err());
        assert!(bed.scan(3).is_err());
        assert!(bed.scan(4).is_err());
        let observed = bed.scan(5).unwrap();
        assert_eq!(
            observed
                .files()
                .iter()
                .map(|file| (file.name.as_str(), file.size))
                .collect::<Vec<_>>(),
            vec![("audio.flac", 7), ("photo.png", 5), ("screen.mp4", 3)]
        );
        for file in observed.files() {
            let entry = observed.bound_entry(&file.name).unwrap();
            assert_eq!(entry.name, OsStr::new(file.name.as_str()));
            assert_eq!(entry.size, file.size);
            assert_eq!(
                *entry,
                bed.directory
                    .read_prefix(&entry.name, 0)
                    .unwrap()
                    .unwrap()
                    .entry
            );
        }
        assert!(observed.bound_entry(&content("notes.txt")).is_none());
    }

    #[test]
    fn bound_and_path_facts_match_with_independent_header_and_marker_parsing() {
        let bed = Bed::new();
        bed.write("audio.flac", b"1234567");
        let header = format!(
            "{{\"_solstone_processing\": {{\"schema\": \"{}\", \"state\": \"analyzed\"}}}}",
            vocab::SCHEMA
        );
        for (sidecar, record, rows) in [
            (format!("{header}\n\n  \n{{\"start\": 0}}\n"), true, true),
            (format!("{header}\n{{\"timestamp\": 0}}\n"), true, false),
            ("{\"start\": 0}\n".into(), false, false),
            ("invalid header\n{\"start\": 0}\n".into(), false, true),
            (format!("{header}\ninvalid row\n"), true, false),
        ] {
            bed.write("audio.jsonl", sidecar);
            let bound = bed.scan(2).unwrap();
            let path = scan_segment_checked(&bed.path(), &ClosedHandlerSet, &JournalMedia).unwrap();
            let b = &bound.files()[0];
            let p = &path.files()[0];
            assert_eq!(b.name, p.name);
            assert_eq!(b.size, p.size);
            assert_eq!(b.sidecar.record, p.sidecar.record);
            assert_eq!(b.sidecar.has_analysis_row, p.sidecar.has_analysis_row);
            assert_eq!(b.sidecar.record.is_some(), record);
            assert_eq!(b.sidecar.has_analysis_row, rows);
            assert!(bound.bound_entry(&b.name).is_some());
            assert!(path.bound_entry(&p.name).is_none());
        }
        fs::remove_file(bed.path().join("audio.jsonl")).unwrap();
        let missing = bed.scan(1).unwrap();
        assert!(missing.files()[0].sidecar.record.is_none());
        assert!(!missing.files()[0].sidecar.has_analysis_row);
    }

    #[test]
    fn bound_prefix_accepts_long_transcripts_and_does_not_manufacture_oversize_headers() {
        let bed = Bed::new();
        bed.write("audio.flac", b"1234567");
        let long = format!(
            "{{}}\n{{\"start\": 0}}\n{}",
            "x".repeat(READ_BOUND as usize + 1024)
        );
        bed.write("audio.jsonl", long);
        let prefix = bed
            .directory
            .read_prefix(OsStr::new("audio.jsonl"), READ_BOUND as usize)
            .unwrap()
            .unwrap();
        assert!(!prefix.complete);
        assert_eq!(prefix.bytes.len(), READ_BOUND as usize);
        assert!(bed.scan(2).unwrap().files()[0].sidecar.has_analysis_row);
        bed.write(
            "audio.jsonl",
            format!(
                "{{\"pad\": \"{}\", \"_solstone_processing\": {{}}}}\n{{\"start\": 0}}\n",
                "x".repeat(READ_BOUND as usize + 1024)
            ),
        );
        let observed = bed.scan(2).unwrap();
        assert!(observed.files()[0].sidecar.record.is_none());
        assert!(!observed.files()[0].sidecar.has_analysis_row);
    }

    /// A long transcript whose read bound falls inside a multi-byte character.
    #[test]
    fn a_character_cut_by_the_read_bound_neither_refuses_nor_hides_rows() {
        let bed = Bed::new();
        bed.write("audio.flac", b"1234567");
        let bound = READ_BOUND as usize;
        let mut sidecar = format!(
            "{{\"_solstone_processing\": {{\"schema\": \"{}\", \"state\": \"analyzed\"}}}}\n{{\"start\": 0, \"text\": \"hi\"}}\n",
            vocab::SCHEMA
        );
        let filler = format!("{{\"start\": 1, \"text\": \"{}\"}}\n", "b".repeat(1000));
        while sidecar.len() + filler.len() < bound - 2000 {
            sidecar.push_str(&filler);
        }
        sidecar.push_str("{\"start\": 2, \"text\": \"");
        sidecar.push_str(&"c".repeat(bound - 2 - sidecar.len()));
        sidecar.push_str("\u{1F600}\"}\n");
        assert!(!sidecar.is_char_boundary(bound));
        bed.write("audio.jsonl", &sidecar);

        let bound_scan = bed.scan(2).unwrap();
        let path_scan =
            scan_segment_checked(&bed.path(), &ClosedHandlerSet, &JournalMedia).unwrap();
        let soft = read_sidecar(
            &bed.path().join("audio.jsonl"),
            Some(vocab::AUDIO_TRANSCRIPT_ROW_KEY),
        );
        for facts in [
            &bound_scan.files()[0].sidecar,
            &path_scan.files()[0].sidecar,
            &soft,
        ] {
            assert!(facts.record.is_some());
            assert!(facts.has_analysis_row);
        }
    }

    /// A header longer than the bound, cut mid-character, holds the raw without
    /// refusing the segment.
    #[test]
    fn a_header_cut_mid_character_by_the_bound_holds_without_refusing() {
        let bed = Bed::new();
        bed.write("audio.flac", b"1234567");
        let sidecar = format!(
            "{{\"pad\": \"{}\", \"_solstone_processing\": {{}}}}\n{{\"start\": 0}}\n",
            "\u{e9}".repeat(READ_BOUND as usize)
        );
        assert!(!sidecar.is_char_boundary(READ_BOUND as usize));
        bed.write("audio.jsonl", &sidecar);
        let bound_scan = bed.scan(2).unwrap();
        let path_scan =
            scan_segment_checked(&bed.path(), &ClosedHandlerSet, &JournalMedia).unwrap();
        for facts in [
            &bound_scan.files()[0].sidecar,
            &path_scan.files()[0].sidecar,
        ] {
            assert!(facts.record.is_none());
            assert!(!facts.has_analysis_row);
        }
    }

    /// Only the rows the decision reads are held to UTF-8.
    #[test]
    fn invalid_utf8_refuses_only_within_the_rows_read() {
        let bed = Bed::new();
        bed.write("audio.flac", b"1234567");
        bed.write("audio.jsonl", b"{}\n{\"start\": \"\xff\"}\n");
        assert!(bed.scan(2).is_err());
        assert!(scan_segment_checked(&bed.path(), &ClosedHandlerSet, &JournalMedia).is_err());

        bed.write("audio.jsonl", b"{}\n{\"start\": 0}\n\xff\n");
        assert!(bed.scan(2).unwrap().files()[0].sidecar.has_analysis_row);
        assert!(
            scan_segment_checked(&bed.path(), &ClosedHandlerSet, &JournalMedia)
                .unwrap()
                .files()[0]
                .sidecar
                .has_analysis_row
        );
    }

    #[test]
    fn bound_nonplain_raw_or_sidecar_and_invalid_utf8_refuse() {
        let bed = Bed::new();
        fs::create_dir(bed.path().join("audio.flac")).unwrap();
        assert!(bed.scan(1).is_err());
        fs::remove_dir(bed.path().join("audio.flac")).unwrap();
        bed.write("audio.flac", b"1234567");
        fs::create_dir(bed.path().join("audio.jsonl")).unwrap();
        assert!(bed.scan(2).is_err());
        fs::remove_dir(bed.path().join("audio.jsonl")).unwrap();
        bed.write("audio.jsonl", [0xff, b'\n']);
        assert!(bed.scan(2).is_err());
        bed.write("audio.jsonl", b"{}\n{\"start\": 0}\n");
        assert!(bed.scan(2).unwrap().files()[0].sidecar.has_analysis_row);
    }

    #[cfg(unix)]
    #[test]
    fn bound_symlink_raw_sidecar_and_non_utf8_name_refuse() {
        use std::os::unix::ffi::OsStringExt;
        let bed = Bed::new();
        bed.write("target", b"1234567");
        std::os::unix::fs::symlink("target", bed.path().join("audio.flac")).unwrap();
        assert!(bed.scan(2).is_err());
        fs::remove_file(bed.path().join("audio.flac")).unwrap();
        bed.write("audio.flac", b"1234567");
        std::os::unix::fs::symlink("target", bed.path().join("audio.jsonl")).unwrap();
        assert!(bed.scan(3).is_err());
        fs::remove_file(bed.path().join("audio.jsonl")).unwrap();
        let odd = bed.path().join(std::ffi::OsString::from_vec(vec![0xff]));
        match fs::write(&odd, b"ignored but unbound name") {
            Ok(()) => {
                assert!(bed.scan(3).is_err());
                fs::remove_file(odd).unwrap();
            }
            // macOS can reject this filename before scanner admission (EILSEQ).
            #[cfg(target_os = "macos")]
            Err(error) if error.raw_os_error() == Some(92) => {}
            Err(error) => panic!("invalid-filename fixture creation failed: {error}"),
        }
        assert_eq!(bed.scan(2).unwrap().files().len(), 1);
    }

    #[test]
    fn bound_final_coverage_refuses_sidecar_appearing_after_initial_listing() {
        use std::cell::Cell;
        use std::rc::Rc;
        let bed = Bed::new();
        bed.write("audio.flac", b"1234567");
        let (unchanged, fired) =
            run_with_media_hook(MediaPrimitive::AfterPrefixRead, || Ok(()), || bed.scan(2));
        assert!(fired && unchanged.is_ok());
        let path = bed.path().join("audio.jsonl");
        let mutated = Rc::new(Cell::new(false));
        let completed = Rc::clone(&mutated);
        let (changed, fired) = run_with_media_hook(
            MediaPrimitive::AfterPrefixRead,
            move || {
                fs::write(path, b"{}\n{\"start\": 0}\n")?;
                completed.set(true);
                Ok(())
            },
            || bed.scan(2),
        );
        assert!(fired && mutated.get());
        assert!(changed.is_err());
        assert!(bed.scan(2).unwrap().files()[0].sidecar.has_analysis_row);
    }

    #[cfg(unix)]
    #[test]
    fn bound_raw_same_length_mutation_and_replacement_during_read_refuse() {
        use std::cell::Cell;
        use std::rc::Rc;
        for replace in [false, true] {
            let bed = Bed::new();
            bed.write("audio.flac", b"1234567");
            let path = bed.path().join("audio.flac");
            let mutated = Rc::new(Cell::new(false));
            let completed = Rc::clone(&mutated);
            let (result, fired) = run_with_media_hook(
                MediaPrimitive::BeforePrefixRead,
                move || {
                    if replace {
                        fs::rename(&path, path.with_extension("moved"))?;
                    }
                    fs::write(path, b"7654321")?;
                    completed.set(true);
                    Ok(())
                },
                || bed.scan(2),
            );
            assert!(fired && mutated.get());
            assert!(result.is_err());
            assert_eq!(bed.scan(2).unwrap().files()[0].size, 7);
        }
    }

    #[cfg(unix)]
    #[test]
    fn bound_sidecar_list_read_mismatch_and_late_previous_sidecar_change_refuse() {
        use std::cell::Cell;
        use std::rc::Rc;
        let bed = Bed::new();
        bed.write("a.flac", b"1234567");
        bed.write("a.jsonl", b"{}\n{\"start\": 0}\n");
        let path = bed.path().join("a.jsonl");
        let mutated = Rc::new(Cell::new(false));
        let completed = Rc::clone(&mutated);
        let (result, fired) = run_with_media_hook(
            MediaPrimitive::AfterPrefixRead,
            move || {
                fs::write(path, b"{}\n{\"start\": 1}\n")?;
                completed.set(true);
                Ok(())
            },
            || bed.scan(2),
        );
        assert!(fired && mutated.get());
        assert!(result.is_err());
        assert!(bed.scan(2).is_ok());

        bed.write("z.flac", b"1234567");
        let changed = Cell::new(false);
        let classifier = |name: &ContentName| {
            if name.as_str() == "z.flac" {
                fs::write(bed.path().join("a.jsonl"), b"{}\n{\"start\": 0}\n").unwrap();
                changed.set(true);
            }
            solstone_core_retention::content::MediaClassifier::is_owner_media(&JournalMedia, name)
        };
        assert!(scan_segment_bound(&bed.directory, &ClosedHandlerSet, &classifier, 3).is_err());
        assert!(changed.get());
        assert_eq!(bed.scan(3).unwrap().files().len(), 2);
    }

    #[cfg(unix)]
    #[test]
    fn bound_directory_and_root_replacement_refuse_even_identical_contents() {
        let bed = Bed::new();
        bed.write("audio.flac", b"1234567");
        fs::rename(bed.path(), bed.temp.path().join("old-segment")).unwrap();
        fs::create_dir(bed.path()).unwrap();
        bed.write("audio.flac", b"1234567");
        assert!(bed.scan(1).is_err());
        let current = Bed::new();
        current.write("audio.flac", b"1234567");
        let parent = current.temp.path().parent().unwrap();
        let moved = parent.join(format!(
            "root-held-{}",
            current.temp.path().file_name().unwrap().to_string_lossy()
        ));
        fs::rename(current.temp.path(), &moved).unwrap();
        fs::create_dir(current.temp.path()).unwrap();
        fs::create_dir(current.path()).unwrap();
        current.write("audio.flac", b"1234567");
        assert!(current.scan(1).is_err());
        fs::remove_dir_all(moved).unwrap();
    }

    #[test]
    fn bound_all_siblings_feed_existing_independent_group_decisions() {
        use jiff::Timestamp;
        use serde_json::json;
        use solstone_core_journal_io::paths::SegmentLayout;
        use solstone_core_retention::grouped::{DecisionPurpose, GroupVerdict, inspect_grouped};
        use solstone_core_retention::policy::{Days, policy_from_retention};
        use solstone_core_retention::receipt::Target;
        let bed = Bed::new();
        for (name, state, row) in [
            ("a", "analyzed", true),
            ("blocked", "pending", false),
            ("empty", "empty", false),
        ] {
            bed.write(&format!("{name}.flac"), b"1234567");
            let mut record = json!({"schema": vocab::SCHEMA, "handler": "transcribe", "state": state,
                "input_size": 7, "attempted_at": "2026-10-10T00:00:00Z"});
            if state == "empty" {
                record["reason_code"] = json!("no_speech");
            }
            bed.write(
                &format!("{name}.jsonl"),
                format!(
                    "{}\n{}",
                    json!({"_solstone_processing": record}),
                    if row { "{\"start\":0}\n" } else { "" }
                ),
            );
        }
        let observed = bed.scan(6).unwrap();
        let mut policy = policy_from_retention(json!({}).as_object().unwrap());
        policy.minimum_age = Days(90);
        let target = Target {
            day: "20261010".into(),
            stream: "_default".into(),
            dir: "prefix_120000_60_suffix".into(),
        };
        let now = "2026-10-10T01:00:00Z".parse::<Timestamp>().unwrap();
        let result = inspect_grouped(
            &observed,
            &target,
            SegmentLayout::Named,
            &policy,
            jiff::tz::Offset::UTC.to_datetime(now).date(),
            now,
            DecisionPurpose::OffloadProposal,
        );
        assert_eq!(result.layout(), SegmentLayout::Named);
        assert_eq!(result.ordinary().members().len(), 2);
        assert!(matches!(
            result.ordinary().verdict(),
            GroupVerdict::Unproven(_)
        ));
        assert_eq!(
            result
                .eligible()
                .map(|file| file.name())
                .collect::<Vec<_>>(),
            vec!["empty.flac"]
        );
    }
}
