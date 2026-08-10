// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, UNIX_EPOCH};

use flate2::Compression;
use flate2::write::GzEncoder;
use serde_json::json;
use solstone_core_transfer::{
    ExportRequest, ImportError, ImportRequest, RescanOutcome, SegmentOutcome, TransferError,
    export, import, send_indexer_rescan,
};
use tar::{Builder, EntryType, Header};

type FixtureFile<'a> = (&'a str, &'a [u8]);
type FixtureSegment<'a> = (&'a str, &'a [FixtureFile<'a>]);

fn write_file(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().expect("parent")).expect("parent directories");
    fs::write(path, bytes).expect("file");
}

fn sha256(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

fn manifest(day: &str, segment: &str, files: &[(&str, &[u8])]) -> serde_json::Value {
    json!({
        "version": 1,
        "day": day,
        "created_at": 1,
        "host": "test",
        "segments": {
            segment: {
                "files": files.iter().map(|(name, bytes)| json!({
                    "name": name,
                    "sha256": sha256(bytes),
                    "size": bytes.len(),
                })).collect::<Vec<_>>(),
            }
        }
    })
}

fn manifest_many(day: &str, segments: &[FixtureSegment<'_>]) -> serde_json::Value {
    let mut values = serde_json::Map::new();
    for (segment, files) in segments {
        values.insert(
            (*segment).to_owned(),
            json!({"files": files.iter().map(|(name, bytes)| json!({
                "name": name,
                "sha256": sha256(bytes),
                "size": bytes.len(),
            })).collect::<Vec<_>>() }),
        );
    }
    json!({"version": 1, "day": day, "segments": values})
}

fn tree(root: &Path) -> Vec<(PathBuf, bool, Vec<u8>, u64)> {
    fn visit(root: &Path, path: &Path, found: &mut Vec<(PathBuf, bool, Vec<u8>, u64)>) {
        let mut entries: Vec<_> = fs::read_dir(path)
            .expect("read tree")
            .map(|entry| entry.expect("tree entry"))
            .collect();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let entry_path = entry.path();
            let metadata = fs::symlink_metadata(&entry_path).expect("metadata");
            if metadata.is_dir() {
                let modified = metadata
                    .modified()
                    .expect("directory mtime")
                    .duration_since(UNIX_EPOCH)
                    .expect("post epoch")
                    .as_secs();
                found.push((
                    entry_path
                        .strip_prefix(root)
                        .expect("relative")
                        .to_path_buf(),
                    true,
                    Vec::new(),
                    modified,
                ));
                visit(root, &entry_path, found);
            } else {
                let modified = metadata
                    .modified()
                    .expect("mtime")
                    .duration_since(UNIX_EPOCH)
                    .expect("post epoch")
                    .as_secs();
                found.push((
                    entry_path
                        .strip_prefix(root)
                        .expect("relative")
                        .to_path_buf(),
                    false,
                    fs::read(&entry_path).expect("file bytes"),
                    modified,
                ));
            }
        }
    }
    let mut found = Vec::new();
    visit(root, root, &mut found);
    found
}

fn assert_unchanged(root: &Path, before: &[(PathBuf, bool, Vec<u8>, u64)]) {
    assert_eq!(tree(root), before);
}

fn set_mtime(path: &Path, seconds: u64) {
    File::options()
        .write(true)
        .open(path)
        .expect("open timestamp target")
        .set_times(fs::FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(seconds)))
        .expect("set timestamp");
}

fn write_archive(path: &Path, manifest: &serde_json::Value, files: &[(&str, &[u8])]) {
    let output = File::create(path).expect("archive");
    let encoder = GzEncoder::new(output, Compression::default());
    let mut archive = Builder::new(encoder);
    append(
        &mut archive,
        "manifest.json",
        &serde_json::to_vec(manifest).expect("manifest"),
    );
    for (name, bytes) in files {
        append(&mut archive, name, bytes);
    }
    archive.finish().expect("finish archive");
}

fn append(archive: &mut Builder<GzEncoder<File>>, name: &str, bytes: &[u8]) {
    let mut header = Header::new_gnu();
    header.set_entry_type(EntryType::Regular);
    header.set_size(bytes.len() as u64);
    header.set_mode(0o644);
    header.set_mtime(1_700_000_000);
    header.set_cksum();
    archive
        .append_data(&mut header, name, bytes)
        .expect("tar member");
}

#[test]
fn export_and_import_preserve_regular_members_and_drop_subdirectories() {
    let source = tempfile::tempdir().expect("source");
    let day = "20260203";
    let segment = source
        .path()
        .join("chronicle")
        .join(day)
        .join("audio")
        .join("120000_30");
    write_file(&segment.join("stream.json"), b"stream");
    write_file(&segment.join("ingest.json"), b"ingest");
    write_file(&segment.join("device.json"), b"device");
    write_file(&segment.join("nested").join("ignored.json"), b"ignored");
    let output = source.path().join("archive.tgz");

    let export_report = export(
        source.path(),
        ExportRequest {
            day: day.to_owned(),
            output: output.clone(),
        },
    )
    .expect("export");
    assert_eq!(export_report.segments, 1);
    assert_eq!(export_report.files, 3);

    let destination = tempfile::tempdir().expect("destination");
    let report = import(
        destination.path(),
        ImportRequest {
            archive: output,
            dry_run: false,
        },
    )
    .expect("import");
    assert_eq!(report.landed(), 1);
    assert!(matches!(report.rescan, RescanOutcome::Unavailable));
    let imported = destination
        .path()
        .join("chronicle")
        .join(day)
        .join("audio")
        .join("120000_30");
    assert_eq!(
        fs::read(imported.join("stream.json")).expect("stream"),
        b"stream"
    );
    assert_eq!(
        fs::read(imported.join("ingest.json")).expect("ingest"),
        b"ingest"
    );
    assert_eq!(
        fs::read(imported.join("device.json")).expect("device"),
        b"device"
    );
    assert!(!imported.join("nested").exists());
}

#[cfg(unix)]
#[test]
fn export_refuses_a_symlinked_day_directory() {
    use std::os::unix::fs::symlink;

    let journal = tempfile::tempdir().expect("journal");
    let external = tempfile::tempdir().expect("external");
    write_file(
        &external.path().join("audio/120000_30/device.json"),
        b"must not archive",
    );
    let day_directory = journal.path().join("chronicle/20260203");
    fs::create_dir_all(day_directory.parent().expect("chronicle")).expect("chronicle");
    symlink(external.path(), &day_directory).expect("day symlink");
    let before = tree(external.path());
    let output = journal.path().join("outside.tgz");

    let error = export(
        journal.path(),
        ExportRequest {
            day: "20260203".to_owned(),
            output: output.clone(),
        },
    )
    .expect_err("symlinked day must be refused");

    assert!(matches!(error, TransferError::PoisonedDayDirectory(_)));
    assert!(!output.exists());
    assert_unchanged(external.path(), &before);
}

#[cfg(unix)]
#[test]
fn import_refuses_a_symlinked_day_directory_without_writing_its_target() {
    use std::os::unix::fs::symlink;

    let temporary = tempfile::tempdir().expect("temporary");
    let archive = temporary.path().join("input.tgz");
    let member = "audio/120000_30/device.json";
    write_archive(
        &archive,
        &manifest(
            "20260203",
            "audio/120000_30",
            &[("device.json", b"archive")],
        ),
        &[(member, b"archive")],
    );
    let journal = tempfile::tempdir().expect("journal");
    let external = tempfile::tempdir().expect("external");
    write_file(&external.path().join("sentinel"), b"unchanged");
    let day_directory = journal.path().join("chronicle/20260203");
    fs::create_dir_all(day_directory.parent().expect("chronicle")).expect("chronicle");
    symlink(external.path(), &day_directory).expect("day symlink");
    let before = tree(external.path());

    let error = import(
        journal.path(),
        ImportRequest {
            archive,
            dry_run: false,
        },
    )
    .expect_err("symlinked day must be refused");

    assert!(matches!(
        error,
        ImportError::Fatal(TransferError::PoisonedDayDirectory(_))
    ));
    assert_unchanged(external.path(), &before);
}

#[test]
fn hash_mismatch_leaves_no_journal_content() {
    let temporary = tempfile::tempdir().expect("temporary");
    let archive = temporary.path().join("bad.tgz");
    let bytes = b"expected";
    let manifest = manifest("20260203", "audio/120000_30", &[("device.json", bytes)]);
    write_archive(
        &archive,
        &manifest,
        &[("audio/120000_30/device.json", b"actual")],
    );

    let destination = tempfile::tempdir().expect("destination");
    assert!(
        import(
            destination.path(),
            ImportRequest {
                archive,
                dry_run: false
            }
        )
        .is_err()
    );
    assert!(!destination.path().join("chronicle").exists());
}

#[test]
fn dry_run_does_not_create_the_journal_tree() {
    let temporary = tempfile::tempdir().expect("temporary");
    let archive = temporary.path().join("archive.tgz");
    let bytes = b"device";
    let manifest = manifest("20260203", "audio/120000_30", &[("device.json", bytes)]);
    write_archive(
        &archive,
        &manifest,
        &[("audio/120000_30/device.json", bytes)],
    );
    let destination = tempfile::tempdir().expect("destination");

    let report = import(
        destination.path(),
        ImportRequest {
            archive,
            dry_run: true,
        },
    )
    .expect("dry-run import");
    assert_eq!(report.landed(), 1);
    assert!(!destination.path().join("chronicle").exists());
}

#[test]
fn already_synced_segments_are_skipped_and_conflicts_deconflict() {
    let temporary = tempfile::tempdir().expect("temporary");
    let archive = temporary.path().join("archive.tgz");
    let bytes = b"device";
    let manifest = manifest("20260203", "audio/120000_30", &[("device.json", bytes)]);
    write_archive(
        &archive,
        &manifest,
        &[("audio/120000_30/device.json", bytes)],
    );
    let destination = tempfile::tempdir().expect("destination");
    let existing = destination
        .path()
        .join("chronicle/20260203/audio/120000_30/device.json");
    write_file(&existing, bytes);
    let report = import(
        destination.path(),
        ImportRequest {
            archive: archive.clone(),
            dry_run: false,
        },
    )
    .expect("synced import");
    assert!(matches!(
        report.outcomes.as_slice(),
        [SegmentOutcome::SkippedAlreadySynced { .. }]
    ));

    write_file(&existing, b"different");
    let report = import(
        destination.path(),
        ImportRequest {
            archive,
            dry_run: false,
        },
    )
    .expect("deconflicted import");
    assert!(matches!(
        report.outcomes.as_slice(),
        [SegmentOutcome::LandedDeconflicted { .. }]
    ));
}

#[test]
fn non_regular_tar_member_is_rejected_before_journal_publication() {
    let temporary = tempfile::tempdir().expect("temporary");
    let archive_path = temporary.path().join("link.tgz");
    let bytes = b"device";
    let manifest = manifest("20260203", "audio/120000_30", &[("device.json", bytes)]);
    let output = File::create(&archive_path).expect("archive");
    let encoder = GzEncoder::new(output, Compression::default());
    let mut archive = Builder::new(encoder);
    append(
        &mut archive,
        "manifest.json",
        &serde_json::to_vec(&manifest).expect("manifest"),
    );
    let mut header = Header::new_gnu();
    header.set_entry_type(EntryType::Symlink);
    header.set_size(0);
    header.set_cksum();
    archive
        .append_link(&mut header, "audio/120000_30/device.json", "outside")
        .expect("link");
    archive.finish().expect("finish");
    let destination = tempfile::tempdir().expect("destination");
    assert!(
        import(
            destination.path(),
            ImportRequest {
                archive: archive_path,
                dry_run: false
            }
        )
        .is_err()
    );
    assert!(!destination.path().join("chronicle").exists());
}

#[cfg(unix)]
#[test]
fn rescan_sender_uses_the_existing_supervisor_request_shape() {
    use std::io::Read;
    use std::os::unix::net::UnixListener;

    let temporary = tempfile::tempdir().expect("temporary");
    let health = temporary.path().join("health");
    fs::create_dir(&health).expect("health");
    let socket = health.join("callosum.sock");
    let listener = UnixListener::bind(&socket).expect("listener");
    let receiver = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("connection");
        let mut line = String::new();
        stream.read_to_string(&mut line).expect("line");
        line
    });
    assert_eq!(send_indexer_rescan(temporary.path()), RescanOutcome::Queued);
    let line = receiver.join().expect("receiver");
    let value: serde_json::Value = serde_json::from_str(&line).expect("json");
    assert_eq!(value["tract"], "supervisor");
    assert_eq!(value["event"], "request");
    assert_eq!(value["cmd"], json!(["journal", "indexer", "--rescan"]));
}

#[test]
fn export_requires_an_existing_output_parent_and_overwrites_existing_output() {
    let source = tempfile::tempdir().expect("source");
    write_file(
        &source
            .path()
            .join("chronicle/20260203/120000_30/device.json"),
        b"device",
    );
    let missing = source.path().join("missing/archive.tgz");
    assert!(
        export(
            source.path(),
            ExportRequest {
                day: "20260203".to_owned(),
                output: missing
            }
        )
        .is_err()
    );
    let output = source.path().join("archive.tgz");
    fs::write(&output, b"old").expect("old archive");
    export(
        source.path(),
        ExportRequest {
            day: "20260203".to_owned(),
            output: output.clone(),
        },
    )
    .expect("overwrite export");
    assert_ne!(fs::read(output).expect("archive"), b"old");
}

#[test]
fn manifest_disagreement_refuses_without_changing_the_destination_tree() {
    let temporary = tempfile::tempdir().expect("temporary");
    let destination = tempfile::tempdir().expect("destination");
    write_file(
        &destination.path().join("chronicle/19990101/keep.json"),
        b"keep",
    );
    let before = tree(destination.path());
    let bytes = b"device";
    let valid = manifest("20260203", "audio/120000_30", &[("device.json", bytes)]);

    let unexpected = temporary.path().join("unexpected.tgz");
    write_archive(
        &unexpected,
        &valid,
        &[
            ("audio/120000_30/device.json", bytes),
            ("audio/120000_30/extra.json", b"extra"),
        ],
    );
    assert!(
        import(
            destination.path(),
            ImportRequest {
                archive: unexpected,
                dry_run: false
            }
        )
        .is_err()
    );
    assert_unchanged(destination.path(), &before);

    let missing = temporary.path().join("missing.tgz");
    write_archive(&missing, &valid, &[]);
    let error = import(
        destination.path(),
        ImportRequest {
            archive: missing,
            dry_run: false,
        },
    )
    .expect_err("missing member refused");
    assert!(error.to_string().contains("device.json"));
    assert_unchanged(destination.path(), &before);
}

#[test]
fn hostile_manifest_routes_are_refused_before_any_destination_write() {
    let temporary = tempfile::tempdir().expect("temporary");
    let cases = [
        manifest("20260203", "../escape", &[("device.json", b"device")]),
        manifest(
            "20260203",
            "audio/120000_30",
            &[("../../outside", b"device")],
        ),
        manifest("20260203", "audio/120000_30", &[("", b"")]),
    ];
    for (index, manifest) in cases.iter().enumerate() {
        let archive = temporary.path().join(format!("hostile-{index}.tgz"));
        write_archive(&archive, manifest, &[]);
        let destination = tempfile::tempdir().expect("destination");
        let before = tree(destination.path());
        assert!(
            import(
                destination.path(),
                ImportRequest {
                    archive,
                    dry_run: false
                }
            )
            .is_err()
        );
        assert_unchanged(destination.path(), &before);
    }
}

#[test]
fn dry_run_uses_the_same_hostile_manifest_refusal_as_real_import() {
    let temporary = tempfile::tempdir().expect("temporary");
    let archive = temporary.path().join("hostile.tgz");
    let manifest = manifest("20260203", "../escape", &[("device.json", b"device")]);
    write_archive(&archive, &manifest, &[]);
    let destination = tempfile::tempdir().expect("destination");
    let before = tree(destination.path());
    let dry = import(
        destination.path(),
        ImportRequest {
            archive: archive.clone(),
            dry_run: true,
        },
    )
    .expect_err("dry-run refusal");
    let real = import(
        destination.path(),
        ImportRequest {
            archive,
            dry_run: false,
        },
    )
    .expect_err("real refusal");
    assert_eq!(dry.to_string(), real.to_string());
    assert!(
        dry.to_string()
            .contains("journal path contains invalid component")
    );
    assert_unchanged(destination.path(), &before);
}

#[test]
fn empty_manifest_segment_is_not_published_or_counted() {
    let temporary = tempfile::tempdir().expect("temporary");
    let archive = temporary.path().join("empty.tgz");
    let manifest = manifest_many("20260203", &[("audio/120000_30", &[])]);
    write_archive(&archive, &manifest, &[]);
    let destination = tempfile::tempdir().expect("destination");
    let report = import(
        destination.path(),
        ImportRequest {
            archive,
            dry_run: false,
        },
    )
    .expect("import");
    assert_eq!(report.landed(), 0);
    assert!(report.outcomes.is_empty());
    assert!(
        !destination
            .path()
            .join("chronicle/20260203/audio/120000_30")
            .exists()
    );
}

#[test]
fn invalid_day_is_refused_before_any_destination_write() {
    let temporary = tempfile::tempdir().expect("temporary");
    let archive = temporary.path().join("bad-day.tgz");
    let manifest = manifest(
        "2026-02-03",
        "audio/120000_30",
        &[("device.json", b"device")],
    );
    write_archive(&archive, &manifest, &[]);
    let destination = tempfile::tempdir().expect("destination");
    let before = tree(destination.path());
    let error = import(
        destination.path(),
        ImportRequest {
            archive,
            dry_run: false,
        },
    )
    .expect_err("bad day");
    assert!(error.to_string().contains("day must be YYYYMMDD"));
    assert_unchanged(destination.path(), &before);
}

#[test]
fn non_regular_members_are_refused_with_or_without_manifest_entries() {
    for (entry_type, listed) in [
        (EntryType::Symlink, false),
        (EntryType::Symlink, true),
        (EntryType::Link, false),
        (EntryType::Link, true),
    ] {
        let temporary = tempfile::tempdir().expect("temporary");
        let archive_path = temporary.path().join("link.tgz");
        let bytes = b"";
        let manifest = if listed {
            manifest("20260203", "audio/120000_30", &[("device.json", bytes)])
        } else {
            manifest_many("20260203", &[("audio/120000_30", &[])])
        };
        let output = File::create(&archive_path).expect("archive");
        let encoder = GzEncoder::new(output, Compression::default());
        let mut archive = Builder::new(encoder);
        append(
            &mut archive,
            "manifest.json",
            &serde_json::to_vec(&manifest).expect("manifest"),
        );
        let mut header = Header::new_gnu();
        header.set_entry_type(entry_type);
        header.set_size(0);
        header.set_cksum();
        archive
            .append_link(&mut header, "audio/120000_30/device.json", "outside")
            .expect("link");
        archive.finish().expect("finish");
        let destination = tempfile::tempdir().expect("destination");
        assert!(
            import(
                destination.path(),
                ImportRequest {
                    archive: archive_path,
                    dry_run: false
                }
            )
            .is_err()
        );
        assert!(
            !destination
                .path()
                .join("chronicle/20260203/audio/120000_30/device.json")
                .exists()
        );
    }
}

#[test]
fn deconfliction_skips_five_immediate_occupied_neighbors_without_touching_them() {
    let temporary = tempfile::tempdir().expect("temporary");
    let archive = temporary.path().join("collision.tgz");
    let bytes = b"incoming";
    let manifest = manifest("20260203", "audio/120000_30", &[("device.json", bytes)]);
    write_archive(
        &archive,
        &manifest,
        &[("audio/120000_30/device.json", bytes)],
    );
    let destination = tempfile::tempdir().expect("destination");
    let keys = [
        "120000_30",
        "115959_30",
        "120001_30",
        "120000_29",
        "120000_31",
    ];
    for (index, key) in keys.iter().enumerate() {
        write_file(
            &destination
                .path()
                .join(format!("chronicle/20260203/audio/{key}/device.json")),
            format!("occupied-{index}").as_bytes(),
        );
    }
    let before: Vec<_> = keys
        .iter()
        .map(|key| {
            tree(
                &destination
                    .path()
                    .join(format!("chronicle/20260203/audio/{key}")),
            )
        })
        .collect();
    let report = import(
        destination.path(),
        ImportRequest {
            archive,
            dry_run: false,
        },
    )
    .expect("import");
    let SegmentOutcome::LandedDeconflicted { target, .. } = &report.outcomes[0] else {
        panic!("deconflicted outcome")
    };
    let key = target.rsplit('/').next().expect("target key");
    assert!(!keys.contains(&key));
    let (time, length) = key.split_once('_').expect("segment key");
    assert_eq!(time.len(), 6);
    assert!(time.bytes().all(|byte| byte.is_ascii_digit()));
    assert!(length.parse::<u64>().expect("length") > 0);
    for (key, snapshot) in keys.iter().zip(before) {
        assert_unchanged(
            &destination
                .path()
                .join(format!("chronicle/20260203/audio/{key}")),
            &snapshot,
        );
    }
}

#[test]
fn simultaneous_collisions_in_one_stream_choose_distinct_reserved_targets() {
    let temporary = tempfile::tempdir().expect("temporary");
    let archive = temporary.path().join("batch.tgz");
    let manifest = manifest_many(
        "20260203",
        &[
            ("audio/235958_30", &[("device.json", b"first")]),
            ("audio/235959_30", &[("device.json", b"second")]),
        ],
    );
    write_archive(
        &archive,
        &manifest,
        &[
            ("audio/235958_30/device.json", b"first"),
            ("audio/235959_30/device.json", b"second"),
        ],
    );
    // A's +1-second candidate is B's otherwise-free source key. The other
    // immediate A candidates are occupied, so retries overwhelmingly observe
    // the batch-reservation path without adding an RNG test seam.
    for _ in 0..64 {
        let destination = tempfile::tempdir().expect("destination");
        for (key, contents) in [
            ("235958_30", b"old-first".as_slice()),
            ("235957_30", b"old-time-minus".as_slice()),
            ("235958_29", b"old-length-minus".as_slice()),
            ("235958_31", b"old-length-plus".as_slice()),
        ] {
            write_file(
                &destination
                    .path()
                    .join(format!("chronicle/20260203/audio/{key}/device.json")),
                contents,
            );
        }
        let before = tree(&destination.path().join("chronicle/20260203/audio"));
        let report = import(
            destination.path(),
            ImportRequest {
                archive: archive.clone(),
                dry_run: false,
            },
        )
        .expect("import");
        let targets: Vec<_> = report
            .outcomes
            .iter()
            .filter_map(|outcome| match outcome {
                SegmentOutcome::LandedDeconflicted { source, target }
                    if source == "audio/235958_30" || source == "audio/235959_30" =>
                {
                    Some((source, target))
                }
                _ => None,
            })
            .collect();
        if !targets
            .iter()
            .any(|(source, target)| *source == "audio/235958_30" && *target == "audio/235959_30")
        {
            continue;
        }
        assert_eq!(targets.len(), 2);
        assert_ne!(targets[0].1, targets[1].1);
        for (source, target) in targets {
            let expected = if source == "audio/235958_30" {
                b"first".as_slice()
            } else {
                b"second".as_slice()
            };
            assert_eq!(
                fs::read(
                    destination
                        .path()
                        .join(format!("chronicle/20260203/{target}/device.json"))
                )
                .expect("landed payload"),
                expected
            );
        }
        for (key, contents) in [
            ("235958_30", b"old-first".as_slice()),
            ("235957_30", b"old-time-minus".as_slice()),
            ("235958_29", b"old-length-minus".as_slice()),
            ("235958_31", b"old-length-plus".as_slice()),
        ] {
            assert_eq!(
                fs::read(
                    destination
                        .path()
                        .join(format!("chronicle/20260203/audio/{key}/device.json"))
                )
                .expect("original payload"),
                contents
            );
        }
        let after = tree(&destination.path().join("chronicle/20260203/audio"));
        assert!(after.len() > before.len());
        return;
    }
    panic!("did not observe A claiming B's free source key after 64 retries");
}

#[test]
fn already_synced_ignores_extra_files_and_mtime() {
    let temporary = tempfile::tempdir().expect("temporary");
    let archive = temporary.path().join("synced.tgz");
    let bytes = b"device";
    let manifest = manifest("20260203", "audio/120000_30", &[("device.json", bytes)]);
    write_archive(
        &archive,
        &manifest,
        &[("audio/120000_30/device.json", bytes)],
    );
    let destination = tempfile::tempdir().expect("destination");
    let device = destination
        .path()
        .join("chronicle/20260203/audio/120000_30/device.json");
    write_file(&device, bytes);
    write_file(&device.with_file_name("extra.json"), b"extra");
    set_mtime(&device, 1_600_000_000);
    let report = import(
        destination.path(),
        ImportRequest {
            archive,
            dry_run: false,
        },
    )
    .expect("import");
    assert!(matches!(
        report.outcomes.as_slice(),
        [SegmentOutcome::SkippedAlreadySynced { .. }]
    ));
}

#[test]
fn dry_run_preserves_existing_tree_and_matches_real_plan_counts() {
    let temporary = tempfile::tempdir().expect("temporary");
    let archive = temporary.path().join("dry-run.tgz");
    let bytes = b"device";
    let manifest = manifest("20260203", "audio/120000_30", &[("device.json", bytes)]);
    write_archive(
        &archive,
        &manifest,
        &[("audio/120000_30/device.json", bytes)],
    );
    let dry_destination = tempfile::tempdir().expect("dry destination");
    let real_destination = tempfile::tempdir().expect("real destination");
    for destination in [&dry_destination, &real_destination] {
        write_file(
            &destination
                .path()
                .join("chronicle/19990101/other/010000_1/device.json"),
            b"unrelated",
        );
    }
    let before = tree(dry_destination.path());
    let dry = import(
        dry_destination.path(),
        ImportRequest {
            archive: archive.clone(),
            dry_run: true,
        },
    )
    .expect("dry run");
    assert_unchanged(dry_destination.path(), &before);
    let real = import(
        real_destination.path(),
        ImportRequest {
            archive,
            dry_run: false,
        },
    )
    .expect("real run");
    assert_eq!(
        (dry.landed(), dry.skipped(), dry.deconflicted()),
        (real.landed(), real.skipped(), real.deconflicted())
    );
}

#[test]
fn import_restores_tar_mtime_to_integer_seconds() {
    let temporary = tempfile::tempdir().expect("temporary");
    let archive = temporary.path().join("mtime.tgz");
    let bytes = b"device";
    let manifest = manifest("20260203", "audio/120000_30", &[("device.json", bytes)]);
    write_archive(
        &archive,
        &manifest,
        &[("audio/120000_30/device.json", bytes)],
    );
    let destination = tempfile::tempdir().expect("destination");
    import(
        destination.path(),
        ImportRequest {
            archive,
            dry_run: false,
        },
    )
    .expect("import");
    let modified = fs::metadata(
        destination
            .path()
            .join("chronicle/20260203/audio/120000_30/device.json"),
    )
    .expect("metadata")
    .modified()
    .expect("mtime")
    .duration_since(UNIX_EPOCH)
    .expect("epoch")
    .as_secs();
    assert_eq!(modified, 1_700_000_000);
}
