// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::error::Error;
use std::fmt;
use std::io;
use std::path::PathBuf;

use solstone_core_journal_io::{AtomicWriteError, LockError, PathError, ReadError};
use solstone_core_segment::{
    ContentName, ContentWriteOutcome, SegmentDir, SegmentError, write_content,
};

use crate::held::is_currently_held;
use crate::manifest::{
    prepare_ingest_notifications, prepare_stream_advance, write_ingest_manifest,
};
use crate::{ApplyPlan, FileDisposition, IngestFile, PlanStatus};

/// Final file disposition published in the ingest event and response.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AppliedDisposition {
    Written,
    AlreadyHeld,
    Unwritten,
}

/// Applied facts for every submitted file, including intentionally unwritten files.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppliedFile {
    pub name: ContentName,
    pub sha256: String,
    pub size: u64,
    pub disposition: AppliedDisposition,
}

/// One successful, manifest-committed application of an accepted resolution plan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApplyResult {
    pub status: PlanStatus,
    pub landed_segment: String,
    pub segment: SegmentDir,
    pub files: Vec<AppliedFile>,
    pub bytes_written: u64,
    pub should_advance: bool,
}

/// Apply failures. `Stale` asks the caller to resolve once more from fresh state.
#[derive(Debug)]
pub enum ApplyError {
    Segment(SegmentError),
    Atomic(AtomicWriteError),
    Read(ReadError),
    Lock(LockError),
    Path(PathError),
    Io { path: PathBuf, source: io::Error },
    PlanInputMismatch,
    Stale,
}

impl fmt::Display for ApplyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Segment(error) => error.fmt(formatter),
            Self::Atomic(error) => error.fmt(formatter),
            Self::Read(error) => error.fmt(formatter),
            Self::Lock(error) => error.fmt(formatter),
            Self::Path(error) => error.fmt(formatter),
            Self::Io { path, source } => write!(formatter, "{}: {source}", path.display()),
            Self::PlanInputMismatch => {
                formatter.write_str("apply files do not match resolution plan")
            }
            Self::Stale => formatter.write_str("resolution plan is stale"),
        }
    }
}

impl Error for ApplyError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Segment(error) => Some(error),
            Self::Atomic(error) => Some(error),
            Self::Read(error) => Some(error),
            Self::Lock(error) => Some(error),
            Self::Path(error) => Some(error),
            Self::Io { source, .. } => Some(source),
            Self::PlanInputMismatch | Self::Stale => None,
        }
    }
}

/// An apply failure that still reports every file that already landed.
#[derive(Debug)]
pub struct ApplyFailure {
    pub error: ApplyError,
    pub applied: Vec<AppliedFile>,
}

impl fmt::Display for ApplyFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.error.fmt(formatter)
    }
}

impl Error for ApplyFailure {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.error)
    }
}

/// Apply one previously resolved plan without retrying stale state internally.
pub fn apply_plan(plan: &ApplyPlan, files: &[IngestFile<'_>]) -> Result<ApplyResult, ApplyFailure> {
    if !matches_plan(plan, files) {
        return Err(ApplyFailure {
            error: ApplyError::PlanInputMismatch,
            applied: Vec::new(),
        });
    }
    let should_advance = prepare_stream_advance(
        &plan.segment,
        &plan.requested_segment,
        plan.created_segment,
        plan.files
            .iter()
            .any(|file| matches!(file.disposition, FileDisposition::NeedsWrite { .. })),
    )
    .map_err(|error| ApplyFailure {
        error,
        applied: Vec::new(),
    })?;
    prepare_ingest_notifications(plan).map_err(|error| ApplyFailure {
        error,
        applied: Vec::new(),
    })?;
    let mut applied = Vec::with_capacity(plan.files.len());
    let mut bytes_written = 0;
    for (planned, file) in plan.files.iter().zip(files) {
        match planned.disposition {
            FileDisposition::NeedsWrite { .. } => {
                match write_content(&plan.segment, file.name.clone(), file.bytes) {
                    Ok(ContentWriteOutcome::Written(content)) => {
                        bytes_written += content.size;
                        applied.push(applied_file(
                            content.name,
                            content.sha256,
                            content.size,
                            AppliedDisposition::Written,
                        ));
                    }
                    Ok(ContentWriteOutcome::AlreadyHeld(content)) => {
                        applied.push(applied_file(
                            content.name,
                            content.sha256,
                            content.size,
                            AppliedDisposition::AlreadyHeld,
                        ));
                    }
                    Ok(ContentWriteOutcome::Conflict { .. }) => {
                        return Err(ApplyFailure {
                            error: ApplyError::Stale,
                            applied,
                        });
                    }
                    Err(error) => {
                        return Err(ApplyFailure {
                            error: ApplyError::Segment(error),
                            applied,
                        });
                    }
                }
            }
            FileDisposition::Held { .. } => {
                applied.push(applied_file(
                    file.name.clone(),
                    planned.sha256.clone(),
                    planned.size,
                    AppliedDisposition::AlreadyHeld,
                ));
            }
            FileDisposition::Unwritten { .. } => {
                applied.push(applied_file(
                    file.name.clone(),
                    planned.sha256.clone(),
                    planned.size,
                    AppliedDisposition::Unwritten,
                ));
            }
        }
    }
    for (planned, file) in plan.files.iter().zip(files) {
        if matches!(planned.disposition, FileDisposition::Held { .. }) {
            match is_currently_held(&plan.segment, file) {
                Ok(true) => {}
                Ok(false) => {
                    return Err(ApplyFailure {
                        error: ApplyError::Stale,
                        applied,
                    });
                }
                Err(source) => {
                    return Err(ApplyFailure {
                        error: ApplyError::Io {
                            path: plan.segment.path().join(file.name.as_str()),
                            source,
                        },
                        applied,
                    });
                }
            }
        }
    }
    if let Err(error) = write_ingest_manifest(&plan.segment, &plan.requested_segment, &applied) {
        return Err(ApplyFailure { error, applied });
    }
    Ok(ApplyResult {
        status: plan.status,
        landed_segment: plan.landed_segment.clone(),
        segment: plan.segment.clone(),
        files: applied,
        bytes_written,
        // Pending first admission survives partial bytes and post-write failures.
        should_advance,
    })
}

fn matches_plan(plan: &ApplyPlan, files: &[IngestFile<'_>]) -> bool {
    plan.files.len() == files.len()
        && plan.files.iter().zip(files).all(|(planned, file)| {
            planned.name == file.name
                && planned.size == file.bytes.len() as u64
                && planned.sha256 == crate::held::sha256(file.bytes)
        })
}

fn applied_file(
    name: ContentName,
    sha256: String,
    size: u64,
    disposition: AppliedDisposition,
) -> AppliedFile {
    AppliedFile {
        name,
        sha256,
        size,
        disposition,
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use std::fs;

    use serde_json::Value;
    use solstone_core_segment::ContentName;

    use crate::{IngestFile, Resolution, resolve_ingest};

    use super::*;

    fn root() -> tempfile::TempDir {
        tempfile::TempDir::new().unwrap()
    }

    fn file<'a>(name: &str, bytes: &'a [u8]) -> IngestFile<'a> {
        IngestFile {
            name: ContentName::new(name).unwrap(),
            bytes,
        }
    }

    #[test]
    fn interrupted_raw_restore_cannot_reuse_the_previous_notification_receipt() {
        let dir = root();
        let files = [file("audio.flac", b"same")];
        let resolve = || {
            let Resolution::Apply(plan) =
                resolve_ingest(dir.path(), "20260804", "device", "120000_1", &files).unwrap()
            else {
                panic!("expected apply plan");
            };
            plan
        };
        let applied = apply_plan(&resolve(), &files).unwrap();
        crate::record_ingest_notification(
            &applied.segment,
            &applied.files,
            &["audio.flac".to_owned()],
        )
        .unwrap();
        assert!(
            crate::pending_ingest_notifications(&applied.segment, &applied.files)
                .unwrap()
                .is_empty()
        );
        let raw = applied.segment.path().join("audio.flac");
        fs::remove_file(&raw).unwrap();
        let restore = resolve();
        assert!(matches!(
            restore.files[0].disposition,
            FileDisposition::NeedsWrite { .. }
        ));
        prepare_ingest_notifications(&restore).unwrap();
        // Stop after publishing the raw bytes but before the normal manifest
        // update. The retry sees held bytes, not this invocation's Written flag.
        fs::write(&raw, b"same").unwrap();
        let retry = apply_plan(&resolve(), &files).unwrap();
        assert_eq!(retry.files[0].disposition, AppliedDisposition::AlreadyHeld);
        assert_eq!(
            crate::pending_ingest_notifications(&retry.segment, &retry.files).unwrap(),
            ["audio.flac"]
        );
        crate::record_ingest_notification(&retry.segment, &retry.files, &["audio.flac".to_owned()])
            .unwrap();
        assert!(
            crate::pending_ingest_notifications(&retry.segment, &retry.files)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn held_drift_is_stale_and_never_commits_a_manifest() {
        let dir = root();
        let root = dir.path().to_path_buf();
        let segment = root.join("chronicle/20260804/device/120000_1");
        fs::create_dir_all(&segment).unwrap();
        fs::write(segment.join("audio.flac"), b"same").unwrap();
        let files = [file("audio.flac", b"same")];
        let Resolution::Apply(plan) =
            resolve_ingest(&root, "20260804", "device", "120000_1", &files).unwrap()
        else {
            panic!("expected apply plan");
        };

        fs::write(segment.join("audio.flac"), b"drift").unwrap();
        let failure = apply_plan(&plan, &files).unwrap_err();
        assert!(matches!(failure.error, ApplyError::Stale));
        assert!(!segment.join("ingest.json").exists());
    }

    #[test]
    fn mid_loop_write_failure_returns_already_written_files() {
        let dir = root();
        let root = dir.path().to_path_buf();
        let files = [file("audio.flac", b"sound"), file("notes.json", b"notes")];
        let Resolution::Apply(plan) =
            resolve_ingest(&root, "20260804", "device", "120000_1", &files).unwrap()
        else {
            panic!("expected apply plan");
        };
        fs::create_dir_all(plan.segment.path().join("notes.json")).unwrap();
        let failure = apply_plan(&plan, &files).unwrap_err();
        assert!(matches!(failure.error, ApplyError::Segment(_)));
        assert_eq!(failure.applied.len(), 1);
        assert_eq!(failure.applied[0].name.as_str(), "audio.flac");
        assert_eq!(failure.applied[0].disposition, AppliedDisposition::Written);
        assert!(plan.segment.path().join("audio.flac").is_file());
        let manifest: Value =
            serde_json::from_slice(&fs::read(plan.segment.path().join("ingest.json")).unwrap())
                .unwrap();
        assert_eq!(manifest["stream_advance_pending"], true);
        assert_eq!(manifest["files"], serde_json::json!({}));
    }

    #[test]
    fn pending_manifest_failure_prevents_raw_writes_and_can_retry() {
        let dir = root();
        let root = dir.path().to_path_buf();
        let files = [file("audio.flac", b"sound")];
        let Resolution::Apply(plan) =
            resolve_ingest(&root, "20260804", "device", "120000_1", &files).unwrap()
        else {
            panic!("expected apply plan");
        };
        fs::create_dir_all(plan.segment.path().join("ingest.json")).unwrap();
        let failure = apply_plan(&plan, &files).unwrap_err();
        assert!(matches!(
            failure.error,
            ApplyError::Lock(_) | ApplyError::Atomic(_) | ApplyError::Read(_)
        ));
        assert!(failure.applied.is_empty());
        assert!(!plan.segment.path().join("audio.flac").exists());
        fs::remove_dir(plan.segment.path().join("ingest.json")).unwrap();
        let Resolution::Apply(retry) =
            resolve_ingest(&root, "20260804", "device", "120000_1", &files).unwrap()
        else {
            panic!("retry apply");
        };
        assert!(!retry.created_segment);
        assert!(apply_plan(&retry, &files).unwrap().should_advance);
    }

    #[test]
    fn pre_proof_orphan_refuses_before_raw_and_preserves_ambiguous_bytes() {
        let dir = root();
        let root = dir.path();
        let segment = root.join("chronicle/20260804/device/120000_1");
        fs::create_dir_all(&segment).unwrap();
        // A crash before proof rename may leave a stage. Its name is not
        // authority: clients may also use temp-looking content names.
        let orphan = segment.join(".tmp_ingest-stage.tmp");
        fs::write(&orphan, b"unfinished proof or legacy content").unwrap();
        let files = [file("audio.flac", b"sound")];
        let Resolution::Apply(plan) =
            resolve_ingest(root, "20260804", "device", "120000_1", &files).unwrap()
        else {
            panic!("apply plan");
        };
        let failure = apply_plan(&plan, &files).unwrap_err();
        assert!(failure.applied.is_empty());
        assert!(!segment.join("audio.flac").exists());
        assert!(!segment.join("ingest.json").exists());
        assert_eq!(
            fs::read(&orphan).unwrap(),
            b"unfinished proof or legacy content"
        );
    }

    #[cfg(unix)]
    #[test]
    fn uncertain_pending_publication_never_admits_raw_bytes() {
        use solstone_core_journal_io::{
            BoundPublicationPrimitive, run_with_bound_publication_fault,
        };
        let dir = root();
        let root = dir.path();
        let files = [file("audio.flac", b"sound")];
        let resolve =
            || match resolve_ingest(root, "20260804", "device", "120000_1", &files).unwrap() {
                Resolution::Apply(plan) => plan,
                _ => panic!("apply plan"),
            };
        for _ in 0..2 {
            let plan = resolve();
            let (result, injected) = run_with_bound_publication_fault(
                BoundPublicationPrimitive::ParentSync,
                1,
                5,
                || apply_plan(&plan, &files),
            );
            assert!(injected);
            let failure = result.unwrap_err();
            assert!(failure.applied.is_empty());
            assert!(!plan.segment.path().join("audio.flac").exists());
        }
        let applied = apply_plan(&resolve(), &files).unwrap();
        assert!(applied.should_advance);
        assert_eq!(
            fs::read(applied.segment.path().join("audio.flac")).unwrap(),
            b"sound"
        );
    }

    #[test]
    fn pending_clear_failure_retries_the_published_marker_without_readvancing() {
        use crate::complete_stream_advance;
        use solstone_core_segment::{StreamHints, advance_bound_stream, bind_stream};
        let dir = root();
        let root = dir.path();
        let cid = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let bound = bind_stream(
            root,
            "20260804",
            "120000_1",
            "device",
            cid,
            "",
            &StreamHints::default(),
        )
        .unwrap();
        let files = [file("audio.flac", b"sound")];
        let resolve =
            || match resolve_ingest(root, "20260804", &bound.stream, "120000_1", &files).unwrap() {
                Resolution::Apply(plan) => plan,
                _ => panic!("apply plan"),
            };
        let applied = apply_plan(&resolve(), &files).unwrap();
        assert!(applied.should_advance);
        let advance = || {
            advance_bound_stream(
                &bound.stream,
                "20260804",
                "120000_1",
                &bound.segment,
                StreamHints::default(),
                cid,
                "",
            )
            .unwrap()
        };
        assert_eq!(advance().seq, 1);
        let path = applied.segment.path().join("ingest.json");
        let bytes = fs::read(&path).unwrap();
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        assert!(complete_stream_advance(&applied.segment).is_err());
        fs::remove_dir(&path).unwrap();
        fs::write(&path, bytes).unwrap();
        assert!(apply_plan(&resolve(), &files).unwrap().should_advance);
        assert_eq!(advance().seq, 1);
        complete_stream_advance(&applied.segment).unwrap();
        assert!(!apply_plan(&resolve(), &files).unwrap().should_advance);
        assert_eq!(
            fs::read(applied.segment.path().join("audio.flac")).unwrap(),
            b"sound"
        );
    }

    #[test]
    fn existing_raw_or_legacy_manifest_never_acquires_admission_intent() {
        for legacy_manifest in [false, true] {
            let dir = root();
            let root = dir.path();
            let segment = root.join("chronicle/20260804/device/120000_1");
            fs::create_dir_all(&segment).unwrap();
            fs::write(segment.join("audio.flac"), b"sound").unwrap();
            if legacy_manifest {
                fs::write(
                    segment.join("ingest.json"),
                    br#"{"schema_version":1,"files":{}}"#,
                )
                .unwrap();
            }
            let files = [file("audio.flac", b"sound")];
            let Resolution::Apply(plan) =
                resolve_ingest(root, "20260804", "device", "120000_1", &files).unwrap()
            else {
                panic!("apply plan");
            };
            assert!(!apply_plan(&plan, &files).unwrap().should_advance);
            let manifest: Value =
                serde_json::from_slice(&fs::read(segment.join("ingest.json")).unwrap()).unwrap();
            assert!(manifest.get("stream_advance_pending").is_none());
        }
    }

    #[test]
    fn manifest_records_written_and_held_files_with_original_request() {
        let dir = root();
        let root = dir.path().to_path_buf();
        let files = [file("audio.flac", b"sound")];
        let Resolution::Apply(plan) =
            resolve_ingest(&root, "20260804", "device", "120000_1", &files).unwrap()
        else {
            panic!("expected apply plan");
        };
        let result = apply_plan(&plan, &files).unwrap();
        assert_eq!(result.files[0].disposition, AppliedDisposition::Written);
        let manifest: Value =
            serde_json::from_slice(&fs::read(result.segment.path().join("ingest.json")).unwrap())
                .unwrap();
        assert_eq!(manifest["schema_version"], 1);
        assert_eq!(manifest["requested_segment"], "120000_1");
        assert_eq!(manifest["files"]["audio.flac"]["size"], 5);
    }
}
