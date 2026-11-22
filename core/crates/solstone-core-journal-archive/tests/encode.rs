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
                .expect("zip member name")
                .into_owned()
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
            "config/",
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
fn portable_zip_keeps_owner_bytes_and_credentials() {
    use solstone_core_journal_io::operational_log::{
        OplogFormat, OplogNameClassification, classify_oplog_name,
    };
    use std::ffi::OsStr;

    const LOG_LEAF: &str = "oplog--cortex~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--8f03cabead7e441d83f6c92b2d89a021--daily-think~7df259e6285645a5f9ea769caa484e07.log";
    const JSONL_LEAF: &str = "oplog--cortex~1ee11af4ed5d63caf142a30a96ba124b--20260901T164233.381904Z--8f03cabead7e441d83f6c92b2d89a021--daily-think~7df259e6285645a5f9ea769caa484e07.jsonl";
    const MALFORMED_LEAF: &str = "oplog--not-a-name.log";

    assert!(matches!(
        classify_oplog_name(OsStr::new(LOG_LEAF)),
        OplogNameClassification::Candidate(Ok(name)) if name.format() == OplogFormat::Log
    ));
    assert!(matches!(
        classify_oplog_name(OsStr::new(JSONL_LEAF)),
        OplogNameClassification::Candidate(Ok(name)) if name.format() == OplogFormat::Jsonl
    ));
    assert!(matches!(
        classify_oplog_name(OsStr::new(MALFORMED_LEAF)),
        OplogNameClassification::Candidate(Err(_))
    ));

    let temporary = TempDir::new("encode-owner-bytes");
    let root = journal(&temporary);

    let jsonl_leaf_path = format!("chronicle/20260101/health/{JSONL_LEAF}");
    let malformed_leaf_path = format!("chronicle/20260101/health/{MALFORMED_LEAF}");
    let nested_log_leaf_path = format!("chronicle/20260101/health/nested/{LOG_LEAF}");
    let seven_digit_log_path = format!("chronicle/2026010/health/{LOG_LEAF}");
    let directory_child_log_path = format!("chronicle/20260102/health/{LOG_LEAF}/child.txt");

    let kept_paths: Vec<&str> = vec![
        "config/journal.json",
        "config/push-vapid.json",
        "config/push-registry.json",
        "backup/hosted/binding.json",
        "link/client.json",
        "link/service-ca.key",
        "link/authorized_clients.json",
        "mcp-endpoint/pop.ed25519.pk8",
        "peers/remote/client.key",
        "config/chatgpt-sign-in.json.unreadable-1",
        "config/chatgpt-sign-in.json",
        "config/plaud-sign-in.json",
        "config/plaud-sign-in.json.renewal",
        "config/plaud-sign-in.json.unreadable-1",
        "mcp-endpoint/oauth.json",
        "mcp-endpoint/byo/accounts/owner/key.json",
        "peers/remote/client.crt",
        "chronicle/20260101/a.txt",
        "entities/owner/entity.json",
        "facets/work/facet.json",
        "facets/work/logs/actions.jsonl",
        "imports/item/source.bin",
        "imports/item/peers/original.txt",
        "imports/item/config/journal.json",
        "imports/item/stats.json",
        "imports/item/owner.tmp",
        "imports/item/owner.lock",
        "imports/item/owner.key",
        "streams/main.json",
        "speakers/keep.txt",
        "reflections/weekly/note.md",
        "owner-root/note.txt",
        "tokens/20260101.jsonl",
        "link/client-descriptions.json",
        "solstone/note.txt",
        "apps/observer/x.json",
        "apps/support/keep.txt",
        "awareness/current.json",
        "awareness/owner_candidate.npz",
        "awareness/speaker_candidates.json",
        "awareness/discovery_clusters.resolved.json",
        "awareness/20260101.jsonl",
        "config/schedules.json",
        "config/actions/20260101.jsonl",
        "health/week-left-out.json",
        "health/work-intent.json",
        "health/entity-merge-recovery/item.json",
        "health/offload/20260101.jsonl",
        "health/pruning-runs/x.jsonl",
        "health/retention.log",
        "logs/facet-heals.jsonl",
        "talents/example/20260101T000000.jsonl",
        ".agents/readme.txt",
        ".claude/readme.txt",
        ".agents/skills/notes.txt",
        ".claude/skills/notes.txt",
        "chronicle/not-a-day/task_log.txt",
        "chronicle/20260101/nested/task_log.txt",
        "chronicle/20260101/nested/stats.json",
        "chronicle/notaday/stats.json",
        "supervisor.log.1",
        "supervisor.1a",
        "supervisor.txt",
        "chronicle/20260101/supervisor.log",
        "chronicle/20260101/health/heartbeat.log",
        &jsonl_leaf_path,
        &malformed_leaf_path,
        &nested_log_leaf_path,
        &seven_digit_log_path,
        "stats.json/child.txt",
        "supervisor.log/child.txt",
        &directory_child_log_path,
        "backup/hosted/note.txt",
    ];

    let mut kept_sentinels: Vec<(&str, Vec<u8>)> = Vec::new();
    for (i, path) in kept_paths.iter().enumerate() {
        let sentinel = format!("keep-sentinel-{:02x}-{}\n", i, path.replace('/', "_")).into_bytes();
        write(&root, path, &sentinel);
        kept_sentinels.push((*path, sentinel));
    }

    let omitted_log_leaf_path = format!("chronicle/20260101/health/{LOG_LEAF}");
    let omitted_regular_paths: Vec<&str> = vec![
        "mcp-endpoint/byo/ingress.sock",
        "mcp-endpoint/byo/cutover.sock",
        "mcp-endpoint/local-door-state.json",
        "mcp-endpoint/lan-door-state.json",
        "mcp-endpoint/byo-door-state.json",
        "mcp-endpoint/.create.lock",
        "mcp-endpoint/tls/.tmp_candidate",
        "mcp-endpoint/.removing_old/key",
        "config/chatgpt-sign-in.json.tmp",
        "config/chatgpt-sign-in.json.lock",
        "config/plaud-sign-in.json.tmp",
        "config/plaud-sign-in.json.lock",
        "awareness/discovery_clusters.json",
        "awareness/activity_state.json",
        "awareness/activity_state/child.txt",
        "awareness/activity_settle/child.txt",
        "health/heartbeat.log",
        "health/steward.log",
        "health/service.log",
        "supervisor.1",
        "supervisor.12",
        "task_log.txt",
        "chronicle/20260101/task_log.txt",
        "chronicle/20260101/stats.json",
        &omitted_log_leaf_path,
        "imports/archive-merge-work/extract/x",
        "chronicle/20260101/.removing_original",
    ];

    let mut omitted_sentinels: Vec<(&str, Vec<u8>)> = Vec::new();
    for (i, path) in omitted_regular_paths.iter().enumerate() {
        let sentinel = format!("omit-sentinel-{:02x}-{}\n", i, path.replace('/', "_")).into_bytes();
        write(&root, path, &sentinel);
        omitted_sentinels.push((*path, sentinel));
    }

    let outside_dir = temporary.path().join("outside");
    fs::create_dir_all(&outside_dir).expect("create outside dir");

    let symlink_paths: Vec<&str> = vec![
        "apps/support/portal",
        ".agents/skills/journal",
        ".agents/skills/solstone",
        ".claude/skills/journal",
        ".claude/skills/solstone",
    ];

    let mut symlink_targets: Vec<(&str, std::path::PathBuf, Vec<u8>)> = Vec::new();
    for (i, link_rel) in symlink_paths.iter().enumerate() {
        let target_file = outside_dir.join(format!("outside_target_{i}.txt"));
        let sentinel = format!("omit-symlink-{:02x}-target\n", i).into_bytes();
        fs::write(&target_file, &sentinel).expect("write outside target");
        let link_full = root.join(link_rel);
        fs::create_dir_all(link_full.parent().expect("link parent")).expect("create link parent");
        std::os::unix::fs::symlink(&target_file, &link_full).expect("create symlink");
        symlink_targets.push((*link_rel, target_file, sentinel));
    }

    let source = ArchiveSource::open(&root).expect("open source");
    let zip_path = temporary.path().join("archive.zip");
    let mut file = File::create(&zip_path).expect("create zip output");
    encode_archive(&request(&source), &mut file).expect("encode archive");

    let skipped_names: Vec<&str> = source
        .inventory()
        .skipped_root_names()
        .iter()
        .map(|s| s.as_str())
        .collect();
    assert!(skipped_names.is_empty());

    let mut archive =
        ZipArchive::new(File::open(&zip_path).expect("open zip output")).expect("parse zip output");

    let expected_dir_members = [
        "chronicle/",
        "entities/",
        "facets/",
        "imports/",
        "streams/",
        "speakers/",
        "reflections/",
        "owner-root/",
        "tokens/",
        "link/",
        "mcp-endpoint/",
        "peers/",
        "solstone/",
        "apps/",
        "awareness/",
        "config/",
        "health/",
        "logs/",
        "talents/",
        ".agents/",
        ".claude/",
        "backup/",
        "stats.json/",
        "supervisor.log/",
    ];
    let mut expected_dirs: Vec<String> = expected_dir_members
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    expected_dirs.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));

    let mut expected_files: Vec<String> = kept_paths.iter().map(|s| (*s).to_owned()).collect();
    expected_files.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));

    let mut expected_sequence = expected_dirs;
    expected_sequence.extend(expected_files);
    expected_sequence.push("_export.json".to_owned());

    let actual_names: Vec<String> = (0..archive.len())
        .map(|i| {
            archive
                .by_index(i)
                .expect("zip member")
                .name()
                .expect("zip member name")
                .into_owned()
        })
        .collect();

    for (name, expected_bytes) in &kept_sentinels {
        let mut member = archive.by_name(name).expect("find kept member in zip");
        let mut body = Vec::new();
        member
            .read_to_end(&mut body)
            .expect("read kept member body");
        assert_eq!(&body, expected_bytes, "zip content mismatch for {name}");
        let on_disk = fs::read(root.join(name)).expect("read source file on disk");
        assert_eq!(
            &on_disk, expected_bytes,
            "source file on disk changed for {name}"
        );
    }

    for (name, expected_bytes) in &omitted_sentinels {
        assert!(
            archive.by_name(name).is_err(),
            "omitted member {name} found in zip"
        );
        let on_disk = fs::read(root.join(name)).expect("read omitted source on disk");
        assert_eq!(
            &on_disk, expected_bytes,
            "omitted source on disk changed for {name}"
        );
    }

    for (link_rel, target_path, expected_bytes) in &symlink_targets {
        assert!(
            archive.by_name(link_rel).is_err(),
            "symlink {link_rel} found in zip"
        );
        assert_eq!(
            fs::read_link(root.join(link_rel)).expect("read symlink"),
            *target_path
        );
        let on_disk = fs::read(target_path).expect("read outside target on disk");
        assert_eq!(&on_disk, expected_bytes);
    }

    let mut all_bodies = Vec::new();
    for i in 0..archive.len() {
        let mut member = archive.by_index(i).expect("member");
        let mut body = Vec::new();
        member.read_to_end(&mut body).expect("read member");
        all_bodies.push(body);
    }

    for (name, sentinel) in &omitted_sentinels {
        for (i, body) in all_bodies.iter().enumerate() {
            assert!(
                !body
                    .windows(sentinel.len())
                    .any(|w| w == sentinel.as_slice()),
                "omitted sentinel for {name} leaked into zip member index {i}"
            );
        }
    }
    for (name, _, sentinel) in &symlink_targets {
        for (i, body) in all_bodies.iter().enumerate() {
            assert!(
                !body
                    .windows(sentinel.len())
                    .any(|w| w == sentinel.as_slice()),
                "symlink target sentinel for {name} leaked into zip member index {i}"
            );
        }
    }

    assert!(actual_names.contains(&"imports/".to_owned()));
    assert!(actual_names.contains(&"config/".to_owned()));
    assert!(actual_names.contains(&"link/client-descriptions.json".to_owned()));
    assert!(actual_names.contains(&"mcp-endpoint/pop.ed25519.pk8".to_owned()));
    assert!(actual_names.contains(&"peers/remote/client.key".to_owned()));
    assert!(actual_names.contains(&"config/journal.json".to_owned()));
    assert_eq!(actual_names, expected_sequence);
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
        .map(|index| {
            archive
                .by_index(index)
                .expect("member")
                .name()
                .expect("zip member name")
                .into_owned()
        })
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
        empty_archive
            .by_index(0)
            .expect("only member")
            .name()
            .expect("zip member name")
            .as_ref(),
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

#[test]
fn portable_zip_preserves_legacy_peer_credentials() {
    let temporary = TempDir::new("encode-peers");
    let root = journal(&temporary);
    write(&root, "peers/remote/client.key", b"peer-key-sentinel-7f3a");
    write(&root, "peers/remote/client.crt", b"peer-crt-sentinel-9c2e");
    write(
        &root,
        "imports/item/peers/original.txt",
        b"nested-peers-owner-4b81",
    );
    write(&root, "chronicle/20260101/note.txt", b"ordinary-note-1d60");

    let pre_key = fs::read(root.join("peers/remote/client.key")).expect("read client.key");
    let pre_crt = fs::read(root.join("peers/remote/client.crt")).expect("read client.crt");

    let source = ArchiveSource::open(&root).expect("open source");
    let path = temporary.path().join("archive.zip");
    let mut file = File::create(&path).expect("create output");
    encode_archive(&request(&source), &mut file).expect("encode");

    let mut archive = ZipArchive::new(File::open(&path).expect("open archive output"))
        .expect("parse archive output");
    let mut members = Vec::new();
    for index in 0..archive.len() {
        let mut member = archive.by_index(index).expect("member");
        let name = member.name().expect("zip member name").into_owned();
        let mut body = Vec::new();
        member.read_to_end(&mut body).expect("read member body");
        members.push((name, body));
    }

    for (name, expected) in [
        ("peers/remote/client.key", pre_key.as_slice()),
        ("peers/remote/client.crt", pre_crt.as_slice()),
    ] {
        let member = members.iter().find(|(member, _)| member == name);
        assert_eq!(
            member.map(|(_, body)| body.as_slice()),
            Some(expected),
            "{name}"
        );
    }

    let note = members
        .iter()
        .find(|(name, _)| name == "chronicle/20260101/note.txt");
    assert_eq!(
        note.map(|(_, body)| body.as_slice()),
        Some(&b"ordinary-note-1d60"[..]),
        "missing or wrong chronicle note"
    );
    let owner = members
        .iter()
        .find(|(name, _)| name == "imports/item/peers/original.txt");
    assert_eq!(
        owner.map(|(_, body)| body.as_slice()),
        Some(&b"nested-peers-owner-4b81"[..]),
        "missing or wrong owner text"
    );

    assert_eq!(
        fs::read(root.join("peers/remote/client.key")).expect("source client.key"),
        pre_key
    );
    assert_eq!(
        fs::read(root.join("peers/remote/client.crt")).expect("source client.crt"),
        pre_crt
    );
}
