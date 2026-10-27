// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! JSON manifest ingestion for audio archives.

use std::fs;
use std::path::{Component, Path};
use std::sync::Arc;

use serde::Deserialize;
use sha2::{Digest, Sha256};
use solstone_core_import::audio_archive::{
    AudioArchiveError, AudioArchiveMemberInput, DurationInput, MemberStartEvidence,
    MemberTimeAuthority, SourceRejection,
};

use super::folder::parse_limitless_civil_time;
use super::slicer::parse_ogg_opus_bytes;

#[derive(Debug, Deserialize)]
pub struct ManifestMemberEntry {
    pub path: Option<String>,
    pub original_relative_name: Option<String>,
    pub instant: Option<String>,
    pub local_civil: Option<String>,
    pub source_zone: Option<String>,
    pub run_note: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ManifestPayload {
    members: Vec<ManifestMemberEntry>,
}

pub fn read_manifest_members(
    manifest_path: &Path,
    source_root: &Path,
    fallback_source_zone: Option<&str>,
) -> Result<Vec<AudioArchiveMemberInput>, AudioArchiveError> {
    let content = fs::read_to_string(manifest_path).map_err(AudioArchiveError::Io)?;
    let payload: ManifestPayload = serde_json::from_str(&content)
        .map_err(|error| AudioArchiveError::Other(format!("invalid manifest JSON: {error}")))?;
    let source = Arc::new(
        solstone_core_journal_io::JournalRoot::open(source_root)
            .map_err(|error| AudioArchiveError::Other(error.to_string()))?,
    );

    let mut members = Vec::with_capacity(payload.members.len());
    for entry in payload.members {
        let file_subpath = entry
            .path
            .clone()
            .or_else(|| entry.original_relative_name.clone())
            .unwrap_or_default();

        let relative_name = entry
            .original_relative_name
            .clone()
            .unwrap_or_else(|| file_subpath.clone());
        if file_subpath.is_empty()
            || Path::new(&file_subpath)
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
        {
            return Err(AudioArchiveError::Other(
                "manifest members must name files inside the selected folder".to_owned(),
            ));
        }
        if Path::new(&relative_name) != Path::new(&file_subpath) {
            return Err(AudioArchiveError::Other(
                "manifest path and original_relative_name must name the same path".to_owned(),
            ));
        }

        let resolved_file = source_root.join(&file_subpath);

        if !solstone_core_import::work_continues() {
            return Err(AudioArchiveError::Other("import stopped".to_owned()));
        }
        let (byte_len, sha256, duration) =
            match read_manifest_member(&source, Path::new(&file_subpath))? {
                Some(bytes) => probe_member_bytes(&resolved_file, &bytes),
                None => (
                    0,
                    String::new(),
                    DurationInput::Rejected(SourceRejection::Unreadable),
                ),
            };

        let file_name = Path::new(&file_subpath)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        let limitless_civil = parse_limitless_civil_time(file_name);

        let explicit = entry.instant.is_some() || entry.local_civil.is_some();
        let (local_civil, authority) = if explicit {
            (entry.local_civil, MemberTimeAuthority::Manifest)
        } else {
            (limitless_civil, MemberTimeAuthority::Filename)
        };
        // A default zone supplies naive civil evidence, not an extra field on an instant.
        let fallback = if entry.instant.is_some() && local_civil.is_none() {
            None
        } else {
            fallback_source_zone.map(str::to_owned)
        };

        let start = MemberStartEvidence {
            instant: entry.instant,
            local_civil,
            source_zone: entry.source_zone.or(fallback),
            authority,
        };

        members.push(AudioArchiveMemberInput {
            original_relative_name: relative_name,
            byte_len,
            sha256,
            duration,
            start,
            run_note: entry.run_note,
        });
    }

    Ok(members)
}

#[cfg(unix)]
fn read_manifest_member(
    root: &Arc<solstone_core_journal_io::JournalRoot>,
    relative: &Path,
) -> Result<Option<Vec<u8>>, AudioArchiveError> {
    use solstone_core_journal_io::FlatDirectory;
    use std::io::Read;
    let error = |error: solstone_core_journal_io::FlatDirectoryError| {
        AudioArchiveError::Other(error.to_string())
    };
    let parent =
        FlatDirectory::open_normal_relative(root, relative.parent().unwrap_or(Path::new("")))
            .map_err(error)?;
    let Some(entry) = parent
        .observe(relative.file_name().expect("validated member path"))
        .map_err(error)?
    else {
        return Ok(None);
    };
    let mut file = parent.open_regular_file(&entry).map_err(error)?;
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 65536];
    loop {
        if !solstone_core_import::work_continues() {
            return Err(AudioArchiveError::Other("import stopped".into()));
        }
        root.revalidate_canonical_binding()
            .map_err(|error| AudioArchiveError::Other(error.to_string()))?;
        parent.verify_regular_file(&file, &entry).map_err(error)?;
        let length = file.read(&mut buffer).map_err(AudioArchiveError::Io)?;
        if length == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..length]);
    }
    parent.verify_regular_file(&file, &entry).map_err(error)?;
    root.revalidate_canonical_binding()
        .map_err(|error| AudioArchiveError::Other(error.to_string()))?;
    Ok(Some(bytes))
}

#[cfg(windows)]
fn read_manifest_member(
    root: &Arc<solstone_core_journal_io::JournalRoot>,
    relative: &Path,
) -> Result<Option<Vec<u8>>, AudioArchiveError> {
    let parent = solstone_core_journal_io::media::MediaDirectory::open(
        root.clone(),
        relative.parent().unwrap_or(Path::new("")),
    )
    .map_err(AudioArchiveError::Io)?;
    let bytes = parent
        .read_file_bounded(
            relative.file_name().expect("validated member path"),
            usize::MAX,
        )
        .map_err(AudioArchiveError::Io)?;
    if !solstone_core_import::work_continues() {
        return Err(AudioArchiveError::Other("import stopped".into()));
    }
    Ok(bytes.map(|read| read.bytes))
}

#[cfg(test)]
thread_local! {
    static MEMBER_PROBES: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

pub fn probe_member_file(path: &Path) -> (u64, String, DurationInput) {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(_) => {
            return (
                fs::metadata(path)
                    .map(|metadata| metadata.len())
                    .unwrap_or(0),
                String::new(),
                DurationInput::Rejected(SourceRejection::Unreadable),
            );
        }
    };
    probe_member_bytes(path, &bytes)
}

fn probe_member_bytes(path: &Path, bytes: &[u8]) -> (u64, String, DurationInput) {
    #[cfg(test)]
    MEMBER_PROBES.with(|count| count.set(count.get() + 1));
    let byte_len = bytes.len() as u64;
    let sha256 = format!("{:x}", Sha256::digest(bytes));
    if bytes.is_empty() {
        return (byte_len, sha256, DurationInput::Empty);
    }
    let supported = path
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("ogg") || extension.eq_ignore_ascii_case("opus")
        });
    let duration = if !supported {
        DurationInput::Rejected(SourceRejection::Unsupported)
    } else {
        match parse_ogg_opus_bytes(bytes) {
            Ok(parsed) if parsed.playable_duration_ms > 0 => {
                DurationInput::Milliseconds(parsed.playable_duration_ms)
            }
            Ok(_) => DurationInput::Empty,
            Err(AudioArchiveError::SourceRejected(reason)) => DurationInput::Rejected(reason),
            Err(_) => DurationInput::Rejected(SourceRejection::Unsupported),
        }
    };
    (byte_len, sha256, duration)
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    use solstone_core_import::{WorkStop, WorkStopGuard};

    use super::MEMBER_PROBES;

    fn admit_one_probe() -> WorkStopGuard {
        MEMBER_PROBES.with(|count| count.set(0));
        let stop =
            WorkStop::linked_with_checkpoint(Arc::new(AtomicBool::new(false)), None, move || {
                MEMBER_PROBES.with(|count| count.get() > 0)
            });
        WorkStopGuard::install(&stop)
    }

    fn probes() -> u32 {
        MEMBER_PROBES.with(|count| count.get())
    }

    #[test]
    fn manifest_rejects_escaping_member_paths_before_probing() {
        let root = tempfile::tempdir().unwrap();
        let selected = root.path().join("selected");
        std::fs::create_dir(&selected).unwrap();
        std::fs::write(root.path().join("outside.opus"), b"outside selected folder").unwrap();
        let manifest = selected.join("members.json");
        for member in [
            "../outside.opus".to_owned(),
            root.path()
                .join("outside.opus")
                .to_str()
                .unwrap()
                .to_owned(),
            String::new(),
        ] {
            MEMBER_PROBES.with(|count| count.set(0));
            std::fs::write(
                &manifest,
                serde_json::to_vec(&serde_json::json!({"members":[{"path":member}]})).unwrap(),
            )
            .unwrap();
            let result = super::read_manifest_members(&manifest, &selected, Some("UTC"));
            assert_eq!(probes(), 0, "untrusted member was probed: {member}");
            assert!(result.is_err(), "unsafe member was accepted: {member}");
        }
        let nested = selected.join("audio");
        std::fs::create_dir(&nested).unwrap();
        std::fs::write(nested.join("inside.opus"), b"inside selected folder").unwrap();
        std::fs::write(&manifest, r#"{"members":[{"path":"audio/inside.opus"}]}"#).unwrap();
        let members = super::read_manifest_members(&manifest, &selected, Some("UTC")).unwrap();
        assert_eq!(
            probes(),
            1,
            "valid nested members still reach the same probe"
        );
        assert_eq!(members[0].byte_len, b"inside selected folder".len() as u64);
    }

    #[test]
    #[cfg(unix)]
    fn manifest_refuses_leaf_and_parent_symlinks_before_probing() {
        let root = tempfile::tempdir().unwrap();
        let selected = root.path().join("selected");
        let outside = root.path().join("outside");
        std::fs::create_dir(&selected).unwrap();
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("clip.opus"), b"outside selected folder").unwrap();
        std::os::unix::fs::symlink(outside.join("clip.opus"), selected.join("link.opus")).unwrap();
        std::os::unix::fs::symlink(&outside, selected.join("link")).unwrap();
        let manifest = selected.join("members.json");
        for member in ["link.opus", "link/clip.opus"] {
            MEMBER_PROBES.with(|count| count.set(0));
            std::fs::write(
                &manifest,
                serde_json::to_vec(&serde_json::json!({"members":[{"path":member}]})).unwrap(),
            )
            .unwrap();
            let result = super::read_manifest_members(&manifest, &selected, Some("UTC"));
            assert_eq!(
                probes(),
                0,
                "a symlink member reached the byte probe: {member}"
            );
            assert!(result.is_err(), "a symlink member was accepted: {member}");
        }
        assert_eq!(
            std::fs::read(outside.join("clip.opus")).unwrap(),
            b"outside selected folder"
        );
    }

    #[test]
    fn stop_after_the_first_folder_probe_does_not_read_the_next_member() {
        let folder = tempfile::tempdir().unwrap();
        std::fs::write(folder.path().join("a.wav"), b"first").unwrap();
        std::fs::write(folder.path().join("b.wav"), b"second").unwrap();
        let _guard = admit_one_probe();
        let error = super::super::scan_folder_members(folder.path(), "UTC").unwrap_err();
        assert_eq!(error.to_string(), "import stopped");
        assert_eq!(probes(), 1);
    }

    #[test]
    fn stop_after_the_first_manifest_probe_does_not_read_the_next_member() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("a.wav"), b"first").unwrap();
        std::fs::write(root.path().join("b.wav"), b"second").unwrap();
        let manifest = root.path().join("members.json");
        std::fs::write(
            &manifest,
            r#"{"members":[
                {"path":"a.wav","original_relative_name":"a.wav"},
                {"path":"b.wav","original_relative_name":"b.wav"}
            ]}"#,
        )
        .unwrap();
        let _guard = admit_one_probe();
        let error = super::read_manifest_members(&manifest, root.path(), Some("UTC")).unwrap_err();
        assert_eq!(error.to_string(), "import stopped");
        assert_eq!(probes(), 1);
    }

    #[test]
    fn stopped_folder_scan_fails_the_archive_before_a_plan() {
        let journal = tempfile::tempdir().unwrap();
        let folder = tempfile::tempdir().unwrap();
        std::fs::write(folder.path().join("a.wav"), b"first").unwrap();
        std::fs::write(folder.path().join("b.wav"), b"second").unwrap();
        let _guard = admit_one_probe();
        let run = super::super::run_audio_archive_with(
            super::super::AudioArchiveDispatchOptions {
                path: folder.path().to_path_buf(),
                source_zone: Some("UTC".to_owned()),
                resume: None,
                expected_identity: None,
                expected_owner_zone: None,
                dry_run: true,
                json: false,
            },
            journal.path(),
            None,
            None,
        );
        assert_eq!(run.exit_code, 1);
        assert_eq!(run.stderr, "audio archive failed: import stopped\n");
        assert_eq!(probes(), 1);
        assert!(!journal.path().join("chronicle").exists());
        assert!(!journal.path().join("imports").exists());
    }
}
