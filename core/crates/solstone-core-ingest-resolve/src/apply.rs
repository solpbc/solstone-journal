// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::error::Error;
use std::fmt;
use std::io;
use std::path::PathBuf;

use solstone_core_journal_io::{AtomicWriteError, LockError, PathError};
use solstone_core_segment::{
    ContentName, ContentWriteOutcome, SegmentDir, SegmentError, write_content,
};

use crate::held::is_currently_held;
use crate::manifest::write_ingest_manifest;
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
        // Independent write-path parity fix: Python advances only after minting
        // a segment directory, never merely because a candidate plan is non-duplicate.
        should_advance: plan.created_segment,
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
        assert!(!plan.segment.path().join("ingest.json").exists());
    }

    #[test]
    fn manifest_write_failure_returns_already_written_files() {
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
            ApplyError::Lock(_) | ApplyError::Atomic(_)
        ));
        assert_eq!(failure.applied.len(), 1);
        assert_eq!(failure.applied[0].disposition, AppliedDisposition::Written);
        assert!(plan.segment.path().join("audio.flac").is_file());
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
