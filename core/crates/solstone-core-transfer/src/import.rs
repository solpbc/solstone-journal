// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Single-pass validation and staged import for transfer archives.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

#[cfg(test)]
use std::sync::{LazyLock, Mutex};

use sha2::{Digest, Sha256};
use solstone_core_journal_io::{
    StagedDirOptions, contained_path, day_path, find_available_segment_with_occupied, path_lexists,
    publish_staged_dir,
};
use solstone_core_transfer_manifest::{
    ExpectedMember, MANIFEST_NAME, SegmentRoute, TransferManifest, expected_members,
    parse_manifest, validate_expected_members,
};
use tar::{Archive, EntryType};
use tempfile::TempDir;

use crate::manifest::{map_manifest_error, reject_symlink_day_directory};
use crate::rescan::{RescanOutcome, send_indexer_rescan};
use crate::{ImportError, ImportReport, ImportRequest, SegmentOutcome, TransferError};

#[cfg(test)]
static TEST_RACE_DESTINATION: LazyLock<Mutex<Option<PathBuf>>> = LazyLock::new(|| Mutex::new(None));

#[cfg(test)]
fn set_test_race_destination(destination: PathBuf) {
    *TEST_RACE_DESTINATION.lock().expect("race hook lock") = Some(destination);
}

#[cfg(test)]
fn inject_test_race(destination: &Path) {
    let mut hook = TEST_RACE_DESTINATION.lock().expect("race hook lock");
    if hook.as_deref() == Some(destination) {
        fs::create_dir_all(destination).expect("race destination");
        fs::write(destination.join("racer.json"), b"racer").expect("race marker");
        *hook = None;
    }
}

#[derive(Debug)]
struct BufferedFile {
    path: PathBuf,
    mtime: u64,
}

#[derive(Debug)]
struct PlannedSegment {
    source: SegmentRoute,
    target: SegmentRoute,
    files: Vec<(String, BufferedFile)>,
    deconflicted: bool,
    already_synced: bool,
}

/// Validate and optionally publish a v1 transfer archive.
pub fn import(journal: &Path, request: ImportRequest) -> Result<ImportReport, ImportError> {
    let archive_file = File::open(&request.archive).map_err(TransferError::from)?;
    let mut archive = Archive::new(flate2::read::GzDecoder::new(archive_file));
    let mut entries = archive.entries().map_err(TransferError::from)?;
    let first = entries
        .next()
        .ok_or_else(|| TransferError::ArchiveMember("archive has no manifest.json".to_owned()))?
        .map_err(TransferError::from)?;
    let manifest = read_manifest(first)?;
    let day_directory =
        day_path(journal, Some(&manifest.day), false).map_err(TransferError::from)?;
    // `contained_path` trusts its supplied root; refuse a symlinked day root so
    // untrusted archive paths cannot redirect publication outside the journal.
    reject_symlink_day_directory(&day_directory)?;
    let expected = expected_members(&manifest).map_err(map_manifest_error)?;
    for member in expected.values() {
        let segment_directory = contained_path(&day_directory, &member.route.archive_key())
            .map_err(TransferError::from)?;
        contained_path(&segment_directory, &member.file.name).map_err(TransferError::from)?;
    }
    validate_expected_members(&expected).map_err(map_manifest_error)?;
    let scratch = TempDir::new().map_err(TransferError::from)?;
    let buffered = buffer_members(entries, &expected, scratch.path())?;
    let plans = plan_segments(&manifest, &day_directory, &expected, &buffered)?;

    let mut report = ImportReport {
        day: manifest.day,
        dry_run: request.dry_run,
        outcomes: Vec::new(),
        rescan: RescanOutcome::NotNeeded,
    };
    if request.dry_run {
        for plan in plans {
            report.outcomes.push(outcome_for_plan(&plan));
        }
        return Ok(report);
    }

    let mut plans = plans.into_iter();
    while let Some(plan) = plans.next() {
        if plan.already_synced {
            report.outcomes.push(outcome_for_plan(&plan));
            continue;
        }
        let target_directory = day_directory.join(plan.target.archive_key());
        #[cfg(test)]
        inject_test_race(&target_directory);
        // Deliberate divergence from Python's per-file overwrite: each segment
        // is published as one create-only directory set, so a race fails closed.
        match publish_segment(&target_directory, &plan) {
            Ok(()) => report.outcomes.push(outcome_for_plan(&plan)),
            Err(error) => {
                report.outcomes.push(SegmentOutcome::Failed {
                    source: plan.source.archive_key(),
                    reason: error.to_string(),
                });
                report
                    .outcomes
                    .extend(plans.map(|remaining| SegmentOutcome::NotAttempted {
                        source: remaining.source.archive_key(),
                    }));
                if report.landed() > 0 {
                    report.rescan = send_indexer_rescan(journal);
                }
                return Err(ImportError::Partial {
                    report,
                    reason: error.to_string(),
                });
            }
        }
    }
    if report.landed() > 0 {
        report.rescan = send_indexer_rescan(journal);
    }
    Ok(report)
}

fn read_manifest<R: Read>(mut entry: tar::Entry<'_, R>) -> Result<TransferManifest, TransferError> {
    if entry.header().entry_type() != EntryType::Regular {
        return Err(TransferError::ArchiveMember(
            "first archive member must be regular manifest.json".to_owned(),
        ));
    }
    let path = entry.path()?.to_string_lossy().into_owned();
    if path != MANIFEST_NAME {
        return Err(TransferError::ArchiveMember(
            "first archive member must be manifest.json".to_owned(),
        ));
    }
    let mut bytes = Vec::new();
    entry.read_to_end(&mut bytes)?;
    parse_manifest(&bytes).map_err(map_manifest_error)
}

fn buffer_members<R: Read>(
    entries: tar::Entries<'_, R>,
    expected: &BTreeMap<String, ExpectedMember>,
    scratch: &Path,
) -> Result<BTreeMap<String, BufferedFile>, TransferError> {
    let mut buffered = BTreeMap::new();
    for entry in entries {
        let mut entry = entry?;
        if entry.header().entry_type() != EntryType::Regular {
            return Err(TransferError::ArchiveMember(
                "archive contains a non-regular member".to_owned(),
            ));
        }
        let name = entry.path()?.to_string_lossy().into_owned();
        let Some(expected_file) = expected.get(&name) else {
            return Err(TransferError::ArchiveMember(format!(
                "{name} is not listed in manifest"
            )));
        };
        if buffered.contains_key(&name) {
            return Err(TransferError::ArchiveMember(format!(
                "duplicate archive member {name}"
            )));
        }
        let path = scratch
            .join(expected_file.route.archive_key())
            .join(&expected_file.file.name);
        let parent = path
            .parent()
            .ok_or_else(|| TransferError::ArchiveMember(format!("{name} has no scratch parent")))?;
        fs::create_dir_all(parent)?;
        let mtime = entry.header().mtime().unwrap_or(0);
        let (sha256, size) = copy_hashing(&mut entry, &path)?;
        if sha256 != expected_file.file.sha256 || size != expected_file.file.size {
            return Err(TransferError::ContentMismatch(name));
        }
        buffered.insert(name, BufferedFile { path, mtime });
    }
    let missing: Vec<_> = expected
        .keys()
        .filter(|name| !buffered.contains_key(*name))
        .cloned()
        .collect();
    if !missing.is_empty() {
        return Err(TransferError::ArchiveMember(format!(
            "manifest members missing from archive: {}",
            missing.join(", ")
        )));
    }
    Ok(buffered)
}

fn copy_hashing(
    reader: &mut impl Read,
    destination: &Path,
) -> Result<(String, u64), TransferError> {
    let mut destination = File::create(destination)?;
    let mut digest = Sha256::new();
    let mut size = 0_u64;
    let mut buffer = [0_u8; 65_536];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        destination.write_all(&buffer[..read])?;
        digest.update(&buffer[..read]);
        size += read as u64;
    }
    destination.sync_all()?;
    Ok((format!("{:x}", digest.finalize()), size))
}

fn plan_segments(
    manifest: &TransferManifest,
    day_directory: &Path,
    expected: &BTreeMap<String, ExpectedMember>,
    buffered: &BTreeMap<String, BufferedFile>,
) -> Result<Vec<PlannedSegment>, TransferError> {
    let mut reservations: HashMap<String, HashSet<String>> = HashMap::new();
    let mut plans = Vec::new();
    for (route_value, segment) in &manifest.segments {
        if segment.files.is_empty() {
            continue;
        }
        let source = SegmentRoute::parse(route_value).map_err(map_manifest_error)?;
        let target_directory = day_directory.join(source.archive_key());
        let mut files = Vec::new();
        for file in &segment.files {
            let name = format!("{route_value}/{}", file.name);
            let buffered_file = buffered.get(&name).ok_or_else(|| {
                TransferError::ArchiveMember(format!("missing buffered member {name}"))
            })?;
            files.push((
                file.name.clone(),
                BufferedFile {
                    path: buffered_file.path.clone(),
                    mtime: buffered_file.mtime,
                },
            ));
        }
        if local_files_match(&target_directory, &files, expected, route_value)? {
            plans.push(PlannedSegment {
                source: source.clone(),
                target: source,
                files,
                deconflicted: false,
                already_synced: true,
            });
            continue;
        }
        let exists = path_lexists(&target_directory)?;
        let occupied = reservations.entry(source.stream.clone()).or_default();
        let (target, deconflicted) = if exists || occupied.contains(&source.key) {
            let parent = target_directory.parent().ok_or_else(|| {
                TransferError::ArchiveMember(format!("target {route_value} has no parent"))
            })?;
            let Some(key) =
                find_available_segment_with_occupied(parent, &source.key, 100, occupied)?
            else {
                return Err(TransferError::ArchiveMember(format!(
                    "no available segment key for {route_value}"
                )));
            };
            (
                SegmentRoute {
                    stream: source.stream.clone(),
                    key,
                },
                true,
            )
        } else {
            (source.clone(), false)
        };
        occupied.insert(target.key.clone());
        plans.push(PlannedSegment {
            source,
            target,
            files,
            deconflicted,
            already_synced: false,
        });
    }
    Ok(plans)
}

fn local_files_match(
    target_directory: &Path,
    files: &[(String, BufferedFile)],
    expected: &BTreeMap<String, ExpectedMember>,
    route: &str,
) -> Result<bool, TransferError> {
    if !path_lexists(target_directory)? {
        return Ok(false);
    }
    if !target_directory.is_dir() {
        return Ok(false);
    }
    for (name, _) in files {
        let path = contained_path(target_directory, name)?;
        if !path.is_file() {
            return Ok(false);
        }
        let member = expected.get(&format!("{route}/{name}")).ok_or_else(|| {
            TransferError::ArchiveMember(format!("missing expected member {route}/{name}"))
        })?;
        let (sha256, size) = crate::export::hash_file(&path)?;
        if sha256 != member.file.sha256 || size != member.file.size {
            return Ok(false);
        }
    }
    Ok(true)
}

fn publish_segment(destination: &Path, plan: &PlannedSegment) -> Result<(), TransferError> {
    publish_staged_dir(destination, StagedDirOptions::default(), |staging| {
        for (name, buffered) in &plan.files {
            let output = contained_path(staging, name)?;
            let parent = output.parent().ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "staged output has no parent",
                )
            })?;
            fs::create_dir_all(parent)?;
            let mut source = File::open(&buffered.path)?;
            let mut output_file = File::create(&output)?;
            std::io::copy(&mut source, &mut output_file)?;
            output_file.set_times(
                fs::FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(buffered.mtime)),
            )?;
            output_file.sync_all()?;
        }
        Ok::<(), TransferError>(())
    })?;
    Ok(())
}

fn outcome_for_plan(plan: &PlannedSegment) -> SegmentOutcome {
    if plan.already_synced {
        SegmentOutcome::SkippedAlreadySynced {
            source: plan.source.archive_key(),
        }
    } else if plan.deconflicted {
        SegmentOutcome::LandedDeconflicted {
            source: plan.source.archive_key(),
            target: plan.target.archive_key(),
        }
    } else {
        SegmentOutcome::Landed {
            source: plan.source.archive_key(),
            target: plan.target.archive_key(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs::File;

    use flate2::Compression;
    use flate2::write::GzEncoder;
    use serde_json::json;
    use sha2::{Digest, Sha256};
    use solstone_core_journal_io::{StagedDirOptions, StagedWriteError, publish_staged_dir};
    use tar::{Builder, EntryType, Header};

    use super::{
        ImportError, ImportRequest, RescanOutcome, SegmentOutcome, import,
        set_test_race_destination,
    };

    fn digest(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
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
            .expect("member");
    }

    #[test]
    fn create_only_publication_failure_becomes_a_partial_import_report() {
        let temporary = tempfile::tempdir().expect("temporary");
        let archive_path = temporary.path().join("partial.tgz");
        let files = [
            (
                "audio/120000_30",
                "audio/120000_30/device.json",
                b"first".as_slice(),
            ),
            (
                "audio/120001_30",
                "audio/120001_30/device.json",
                b"second".as_slice(),
            ),
            (
                "audio/120002_30",
                "audio/120002_30/device.json",
                b"third".as_slice(),
            ),
        ];
        let mut segments = serde_json::Map::new();
        for (route, _, bytes) in &files {
            segments.insert(
                (*route).to_owned(),
                json!({"files": [{"name": "device.json", "sha256": digest(bytes), "size": bytes.len()}]}),
            );
        }
        let manifest = json!({"version": 1, "day": "20260203", "segments": segments});
        let output = File::create(&archive_path).expect("archive");
        let encoder = GzEncoder::new(output, Compression::default());
        let mut archive = Builder::new(encoder);
        append(
            &mut archive,
            "manifest.json",
            &serde_json::to_vec(&manifest).expect("manifest"),
        );
        for (_, name, bytes) in &files {
            append(&mut archive, name, bytes);
        }
        archive
            .into_inner()
            .expect("tar finish")
            .finish()
            .expect("gzip finish");

        let destination = tempfile::tempdir().expect("destination");
        set_test_race_destination(
            destination
                .path()
                .join("chronicle/20260203/audio/120001_30"),
        );
        let error = import(
            destination.path(),
            ImportRequest {
                archive: archive_path,
                dry_run: false,
            },
        )
        .expect_err("racing publication fails");
        let ImportError::Partial { report, .. } = error else {
            panic!("expected partial report, got {error:?}")
        };
        assert!(matches!(report.outcomes[0], SegmentOutcome::Landed { .. }));
        assert!(matches!(report.outcomes[1], SegmentOutcome::Failed { .. }));
        assert!(matches!(
            report.outcomes[2],
            SegmentOutcome::NotAttempted { .. }
        ));
        assert_eq!(
            std::fs::read(
                destination
                    .path()
                    .join("chronicle/20260203/audio/120000_30/device.json")
            )
            .expect("first"),
            b"first"
        );
        assert!(
            !destination
                .path()
                .join("chronicle/20260203/audio/120002_30")
                .exists()
        );
        assert_ne!(report.rescan, RescanOutcome::NotNeeded);
    }

    #[test]
    fn staged_publisher_reports_already_exists_without_overwriting() {
        let temporary = tempfile::tempdir().expect("temporary");
        let destination = temporary.path().join("existing");
        std::fs::create_dir(&destination).expect("destination");
        let error = publish_staged_dir(&destination, StagedDirOptions::default(), |_| {
            Ok::<(), std::io::Error>(())
        })
        .expect_err("create-only failure");
        assert!(
            matches!(error, StagedWriteError::Io { source, .. } if source.kind() == std::io::ErrorKind::AlreadyExists)
        );
        assert!(destination.is_dir());
    }
}
