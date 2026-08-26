// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]
#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

mod common;

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom};

use solstone_core_journal_archive::{
    ArchiveError, ArchiveSource, DayWindow, EncodeArchiveError, EncodeArchiveRequest,
    EncodingPhase, encode_archive,
};
use zip::{CompressionMethod, ZipArchive};

use common::{TempDir, directory, entry, journal, valid_four_root_journal, write};

const VERSION: &str = "0.9.0";
const EXPORTED_AT: &str = "2026-08-07T21:22:23Z";

fn request<'a>(source: &'a ArchiveSource) -> EncodeArchiveRequest<'a> {
    EncodeArchiveRequest {
        source,
        solstone_version: VERSION,
        exported_at: EXPORTED_AT,
        day_window: None,
    }
}

fn assert_archive_shape(path: &std::path::Path, expected_names: &[&str], expected_manifest: &[u8]) {
    let mut archive = ZipArchive::new(File::open(path).expect("open archive output"))
        .expect("parse archive output");
    let names: Vec<String> = (0..archive.len())
        .map(|index| {
            archive
                .by_index(index)
                .expect("archive member")
                .name()
                .to_owned()
        })
        .collect();
    assert_eq!(
        names,
        expected_names
            .iter()
            .map(|name| (*name).to_owned())
            .collect::<Vec<_>>()
    );

    let directory_count = expected_names
        .iter()
        .take_while(|name| name.ends_with('/'))
        .count();
    for index in 0..directory_count {
        let member = archive.by_index(index).expect("root member");
        assert!(member.is_dir());
        assert_eq!(member.compression(), CompressionMethod::Stored);
        assert_eq!(member.unix_mode(), Some(0o40700));
        let timestamp = member.last_modified().expect("root timestamp");
        assert_eq!(
            (
                timestamp.year(),
                timestamp.month(),
                timestamp.day(),
                timestamp.hour(),
                timestamp.minute(),
                timestamp.second(),
            ),
            (2026, 8, 7, 21, 22, 22)
        );
    }

    for index in directory_count..archive.len() {
        let member = archive.by_index(index).expect("file member");
        assert!(!member.is_dir());
        assert_eq!(member.compression(), CompressionMethod::Deflated);
        assert_eq!(member.unix_mode(), Some(0o100600));
        let timestamp = member.last_modified().expect("file timestamp");
        assert_eq!(
            (
                timestamp.year(),
                timestamp.month(),
                timestamp.day(),
                timestamp.hour(),
                timestamp.minute(),
                timestamp.second(),
            ),
            (2026, 8, 7, 21, 22, 22)
        );
    }

    let mut manifest = archive.by_name("_export.json").expect("manifest member");
    let mut contents = Vec::new();
    manifest
        .read_to_end(&mut contents)
        .expect("read manifest member");
    assert_eq!(contents, expected_manifest);
}

fn assert_member_contents(path: &std::path::Path, expected: &[(&str, &[u8])]) {
    let mut archive = ZipArchive::new(File::open(path).expect("open archive output"))
        .expect("parse archive output");
    for (name, expected_contents) in expected {
        let mut member = archive.by_name(name).expect("archive source member");
        let mut contents = Vec::new();
        member
            .read_to_end(&mut contents)
            .expect("read archive source member");
        assert_eq!(&contents, expected_contents, "wrong contents for {name}");
    }
}

fn assert_invalid_writer(
    source: &ArchiveSource,
    file: &mut File,
    path: &std::path::Path,
    reason: &'static str,
) {
    let before_length = file.metadata().expect("output metadata").len();
    let before_position = file.stream_position().expect("output position");
    assert!(matches!(
        encode_archive(&request(source), file),
        Err(EncodeArchiveError::InvalidWriter { reason: actual }) if actual == reason
    ));
    assert_eq!(
        file.metadata().expect("output metadata").len(),
        before_length
    );
    assert_eq!(
        file.stream_position().expect("output position"),
        before_position
    );
    assert_eq!(
        fs::metadata(path).expect("path metadata").len(),
        before_length
    );
}

#[test]
fn encodes_empty_and_mixed_sources_with_the_fixed_archive_shape() {
    let empty_temporary = TempDir::new("encode-empty");
    let empty_root = journal(&empty_temporary);
    let empty_source = ArchiveSource::open(&empty_root).expect("open empty source");
    let empty_path = empty_temporary.path().join("empty.zip");
    let mut empty_file = File::create(&empty_path).expect("create empty output");

    encode_archive(&request(&empty_source), &mut empty_file).expect("encode empty source");
    let empty_manifest = format!(
        "{{\n  \"solstone_version\": \"{VERSION}\",\n  \"exported_at\": \"{EXPORTED_AT}\",\n  \"source_journal\": \"{}\",\n  \"day_count\": 0,\n  \"entity_count\": 0,\n  \"facet_count\": 0\n}}",
        empty_source
            .canonical_source()
            .to_str()
            .expect("UTF-8 source")
    );
    assert_archive_shape(&empty_path, &["_export.json"], empty_manifest.as_bytes());

    let mixed_temporary = TempDir::new("encode-mixed");
    let mixed_root = valid_four_root_journal(&mixed_temporary);
    directory(&mixed_root, "imports/empty");
    let mixed_source = ArchiveSource::open(&mixed_root).expect("open mixed source");
    let mixed_path = mixed_temporary.path().join("mixed.zip");
    let mut mixed_file = File::create(&mixed_path).expect("create mixed output");

    encode_archive(&request(&mixed_source), &mut mixed_file).expect("encode mixed source");
    let mixed_manifest = format!(
        "{{\n  \"solstone_version\": \"{VERSION}\",\n  \"exported_at\": \"{EXPORTED_AT}\",\n  \"source_journal\": \"{}\",\n  \"day_count\": 1,\n  \"entity_count\": 1,\n  \"facet_count\": 1\n}}",
        mixed_source
            .canonical_source()
            .to_str()
            .expect("UTF-8 source")
    );
    assert_archive_shape(
        &mixed_path,
        &[
            "chronicle/",
            "entities/",
            "facets/",
            "imports/",
            "chronicle/20260101/a.txt",
            "chronicle/20260101/nested/b.txt",
            "entities/alice/entity.json",
            "facets/work/facet.json",
            "imports/import-1/source.bin",
            "_export.json",
        ],
        mixed_manifest.as_bytes(),
    );
    let bytes = fs::read(&mixed_path).expect("read mixed archive");
    assert!(bytes.windows(4).any(|field| field == [1, 0, 16, 0]));
    assert_member_contents(
        &mixed_path,
        &[
            ("chronicle/20260101/a.txt", b"a"),
            ("chronicle/20260101/nested/b.txt", b"bb"),
            ("entities/alice/entity.json", b"{}"),
            ("facets/work/facet.json", b"{}"),
            ("imports/import-1/source.bin", b"source"),
        ],
    );
}

#[test]
fn deny_list_zip_includes_new_roots_and_omits_pruned_trees() {
    let temporary = TempDir::new("encode-deny");
    let root = journal(&temporary);
    write(&root, "chronicle/20260101/a.txt", b"a");
    write(&root, "identity/partner.md", b"hello");
    write(&root, "config/journal.json", b"{}");
    write(&root, "apps/observer/x.json", b"{}");
    write(&root, "chronicle/20260101/foo.sqlite", b"db");
    let source = ArchiveSource::open(&root).expect("open source");
    let path = temporary.path().join("archive.zip");
    let mut file = File::create(&path).expect("create output");
    encode_archive(&request(&source), &mut file).expect("encode");
    let mut archive = ZipArchive::new(File::open(&path).expect("open archive")).expect("parse");
    let names: Vec<String> = (0..archive.len())
        .map(|index| archive.by_index(index).expect("member").name().to_owned())
        .collect();
    assert!(names.contains(&"chronicle/".to_owned()));
    assert!(names.contains(&"identity/".to_owned()));
    assert!(names.contains(&"chronicle/20260101/a.txt".to_owned()));
    assert!(names.contains(&"identity/partner.md".to_owned()));
    assert!(!names.iter().any(|name| name.starts_with("config")));
    assert!(!names.iter().any(|name| name.starts_with("apps")));
    assert!(!names.iter().any(|name| name.ends_with("foo.sqlite")));
}

#[test]
fn date_window_keeps_matching_chronicle_and_drops_every_other_root() {
    let temporary = TempDir::new("encode-window");
    let root = journal(&temporary);
    write(&root, "chronicle/20260101/a.txt", b"a");
    write(&root, "chronicle/20260102/b.txt", b"b");
    write(&root, "entities/alice/entity.json", b"{}");
    write(&root, "facets/work/facet.json", b"{}");
    write(&root, "imports/import-1/source.bin", b"source");
    write(&root, "identity/partner.md", b"hello");
    let source = ArchiveSource::open(&root).expect("open source");
    let path = temporary.path().join("sliced.zip");
    let mut file = File::create(&path).expect("create output");
    let request = EncodeArchiveRequest {
        source: &source,
        solstone_version: VERSION,
        exported_at: EXPORTED_AT,
        day_window: Some(DayWindow {
            from: Some("20260101".to_owned()),
            to: Some("20260101".to_owned()),
        }),
    };
    encode_archive(&request, &mut file).expect("encode sliced");
    let mut archive = ZipArchive::new(File::open(&path).expect("open archive")).expect("parse");
    let names: Vec<String> = (0..archive.len())
        .map(|index| archive.by_index(index).expect("member").name().to_owned())
        .collect();
    assert_eq!(
        names,
        vec![
            "chronicle/".to_owned(),
            "chronicle/20260101/a.txt".to_owned(),
            "_export.json".to_owned(),
        ]
    );
    let mut manifest = archive.by_name("_export.json").expect("manifest");
    let mut contents = Vec::new();
    manifest.read_to_end(&mut contents).expect("read manifest");
    assert!(
        contents
            .windows(b"\"day_count\": 1".len())
            .any(|w| w == b"\"day_count\": 1")
    );
    assert!(
        contents
            .windows(b"\"entity_count\": 0".len())
            .any(|w| w == b"\"entity_count\": 0")
    );
    assert!(
        contents
            .windows(b"\"facet_count\": 0".len())
            .any(|w| w == b"\"facet_count\": 0")
    );

    let empty_path = temporary.path().join("empty-window.zip");
    let mut empty_file = File::create(&empty_path).expect("create empty-window output");
    let empty_request = EncodeArchiveRequest {
        source: &source,
        solstone_version: VERSION,
        exported_at: EXPORTED_AT,
        day_window: Some(DayWindow {
            from: Some("20261231".to_owned()),
            to: Some("20261231".to_owned()),
        }),
    };
    encode_archive(&empty_request, &mut empty_file).expect("encode empty window");
    let mut empty_archive =
        ZipArchive::new(File::open(&empty_path).expect("open empty-window")).expect("parse");
    assert_eq!(empty_archive.len(), 1);
    assert_eq!(
        empty_archive.by_index(0).expect("only member").name(),
        "_export.json"
    );
}

#[test]
fn invalid_writer_forms_leave_real_files_unchanged() {
    let temporary = TempDir::new("encode-invalid-writer");
    let root = journal(&temporary);
    let source = ArchiveSource::open(&root).expect("open source");

    let read_only_path = temporary.path().join("read-only.zip");
    File::create(&read_only_path).expect("create read-only output");
    let mut read_only = OpenOptions::new()
        .read(true)
        .open(&read_only_path)
        .expect("open read-only output");
    assert_invalid_writer(
        &source,
        &mut read_only,
        &read_only_path,
        "must be opened for writing",
    );

    let append_path = temporary.path().join("append.zip");
    File::create(&append_path).expect("create append output");
    let mut append = OpenOptions::new()
        .append(true)
        .open(&append_path)
        .expect("open append output");
    assert_invalid_writer(
        &source,
        &mut append,
        &append_path,
        "must not be opened in append mode",
    );

    let nonempty_path = temporary.path().join("nonempty.zip");
    fs::write(&nonempty_path, b"old").expect("write nonempty output");
    let mut nonempty = OpenOptions::new()
        .write(true)
        .open(&nonempty_path)
        .expect("open nonempty output");
    assert_invalid_writer(&source, &mut nonempty, &nonempty_path, "must be empty");

    let offset_path = temporary.path().join("offset.zip");
    let mut offset = File::create(&offset_path).expect("create offset output");
    offset.seek(SeekFrom::Start(1)).expect("seek offset output");
    assert_invalid_writer(
        &source,
        &mut offset,
        &offset_path,
        "must be positioned at the start",
    );

    let directory_path = temporary.path().join("directory");
    fs::create_dir(&directory_path).expect("create output directory");
    let mut directory = File::open(&directory_path).expect("open output directory");
    assert!(matches!(
        encode_archive(&request(&source), &mut directory),
        Err(EncodeArchiveError::InvalidWriter {
            reason: "must be a regular file"
        })
    ));
}

#[test]
fn metadata_and_pre_inventory_source_failures_leave_output_empty() {
    let temporary = TempDir::new("encode-prewrite-errors");
    let root = journal(&temporary);
    write(&root, "imports/item/source.bin", b"source");
    let source = ArchiveSource::open(&root).expect("open source");

    for (name, solstone_version, exported_at, field) in [
        ("invalid-version", "", EXPORTED_AT, "solstone_version"),
        (
            "invalid-timestamp",
            VERSION,
            "not-a-timestamp",
            "exported_at",
        ),
    ] {
        let path = temporary.path().join(format!("{name}.zip"));
        let mut file = File::create(&path).expect("create metadata output");
        let request = EncodeArchiveRequest {
            source: &source,
            solstone_version,
            exported_at,
            day_window: None,
        };
        assert!(matches!(
            encode_archive(&request, &mut file),
            Err(EncodeArchiveError::InvalidMetadata { field: actual, .. }) if actual == field
        ));
        assert_eq!(file.metadata().expect("metadata output").len(), 0);
        assert_eq!(file.stream_position().expect("metadata position"), 0);
    }

    let member = "imports/item/source.bin";
    assert_eq!(entry(&source, member).member_name().as_str(), member);
    fs::write(root.join(member), b"changed").expect("mutate inventoried source");
    let path = temporary.path().join("changed.zip");
    let mut file = File::create(&path).expect("create changed output");
    assert!(matches!(
        encode_archive(&request(&source), &mut file),
        Err(EncodeArchiveError::Source(ArchiveError::SourceChanged {
            member: Some(actual)
        })) if actual.as_str() == member
    ));
    assert_eq!(file.metadata().expect("changed output metadata").len(), 0);
    assert_eq!(file.stream_position().expect("changed output position"), 0);
}

#[test]
fn exported_archives_never_report_a_fault_for_an_unchanged_source() {
    let temporary = TempDir::new("encode-unchanged");
    let root = valid_four_root_journal(&temporary);
    let source = ArchiveSource::open(&root).expect("open source");
    let path = temporary.path().join("archive.zip");
    let mut file = File::create(&path).expect("create output");

    let result = encode_archive(&request(&source), &mut file);
    assert!(
        !matches!(
            result,
            Err(EncodeArchiveError::ArchiveFinish {
                phase: EncodingPhase::Body | EncodingPhase::Finalize,
                ..
            })
        ),
        "unchanged source must not produce an output fault"
    );
    assert!(result.is_ok());
}
