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
use crate::{ApplyPlan, FileDisposition, HeldEvidence, IngestFile, PlanStatus};

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
    pub evidence: Option<HeldEvidence>,
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

/// Apply one previously resolved plan without retrying stale state internally.
pub fn apply_plan(plan: &ApplyPlan, files: &[IngestFile<'_>]) -> Result<ApplyResult, ApplyError> {
    if !matches_plan(plan, files) {
        return Err(ApplyError::PlanInputMismatch);
    }
    let mut applied = Vec::with_capacity(plan.files.len());
    let mut bytes_written = 0;
    for (planned, file) in plan.files.iter().zip(files) {
        match planned.disposition {
            FileDisposition::NeedsWrite { .. } => {
                match write_content(&plan.segment, file.name.clone(), file.bytes)
                    .map_err(ApplyError::Segment)?
                {
                    ContentWriteOutcome::Written(content) => {
                        bytes_written += content.size;
                        applied.push(applied_file(
                            content.name,
                            content.sha256,
                            content.size,
                            AppliedDisposition::Written,
                            None,
                        ));
                    }
                    ContentWriteOutcome::AlreadyHeld(content) => {
                        applied.push(applied_file(
                            content.name,
                            content.sha256,
                            content.size,
                            AppliedDisposition::AlreadyHeld,
                            Some(HeldEvidence::OnDisk),
                        ));
                    }
                    ContentWriteOutcome::Conflict { .. } => return Err(ApplyError::Stale),
                }
            }
            FileDisposition::Held { evidence } => {
                applied.push(applied_file(
                    file.name.clone(),
                    planned.sha256.clone(),
                    planned.size,
                    AppliedDisposition::AlreadyHeld,
                    Some(evidence),
                ));
            }
            FileDisposition::Unwritten { .. } => {
                applied.push(applied_file(
                    file.name.clone(),
                    planned.sha256.clone(),
                    planned.size,
                    AppliedDisposition::Unwritten,
                    None,
                ));
            }
        }
    }
    for ((planned, file), applied_file) in plan.files.iter().zip(files).zip(applied.iter_mut()) {
        if matches!(planned.disposition, FileDisposition::Held { .. }) {
            let evidence = is_currently_held(&plan.segment, file)
                .map_err(|source| ApplyError::Io {
                    path: plan.segment.path().join(file.name.as_str()),
                    source,
                })?
                .ok_or(ApplyError::Stale)?;
            applied_file.evidence = Some(evidence);
        }
    }
    write_ingest_manifest(&plan.segment, &plan.requested_segment, &applied)?;
    Ok(ApplyResult {
        status: plan.status,
        landed_segment: plan.landed_segment.clone(),
        segment: plan.segment.clone(),
        files: applied,
        bytes_written,
        should_advance: plan.status != PlanStatus::Duplicate,
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
    evidence: Option<HeldEvidence>,
) -> AppliedFile {
    AppliedFile {
        name,
        sha256,
        size,
        disposition,
        evidence,
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use serde_json::{Value, json};
    use solstone_core_segment::ContentName;

    use crate::{IngestFile, Resolution, resolve_ingest};

    use super::*;

    fn root() -> PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("solstone-core-ingest-apply-{suffix}"));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn file<'a>(bytes: &'a [u8]) -> IngestFile<'a> {
        IngestFile {
            name: ContentName::new("audio.flac").unwrap(),
            bytes,
        }
    }

    #[test]
    fn stale_evidence_recheck_is_rejected_before_manifest_commit() {
        let root = root();
        let segment = root.join("chronicle/20260804/device/120000_1");
        fs::create_dir_all(&segment).unwrap();
        fs::write(segment.join("audio.flac"), b"same").unwrap();
        let files = [file(b"same")];
        let Resolution::Apply(plan) =
            resolve_ingest(&root, "20260804", "device", "120000_1", &files).unwrap()
        else {
            panic!("expected apply plan");
        };

        fs::write(segment.join("audio.flac"), b"drift").unwrap();
        assert!(matches!(apply_plan(&plan, &files), Err(ApplyError::Stale)));
        assert!(!segment.join("ingest.json").exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn needs_write_race_records_on_disk_evidence() {
        let root = root();
        let files = [file(b"sound")];
        let Resolution::Apply(plan) =
            resolve_ingest(&root, "20260804", "device", "120000_1", &files).unwrap()
        else {
            panic!("expected apply plan");
        };
        fs::create_dir_all(plan.segment.path()).unwrap();
        fs::write(plan.segment.path().join("audio.flac"), b"sound").unwrap();

        let result = apply_plan(&plan, &files).unwrap();

        assert_eq!(result.files[0].disposition, AppliedDisposition::AlreadyHeld);
        assert_eq!(result.files[0].evidence, Some(HeldEvidence::OnDisk));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn held_recheck_replaces_on_disk_evidence_with_terminal_proof() {
        let root = root();
        let segment = root.join("chronicle/20260804/device/120000_1");
        fs::create_dir_all(&segment).unwrap();
        fs::write(segment.join("audio.flac"), b"sound").unwrap();
        let files = [file(b"sound")];
        let Resolution::Apply(plan) =
            resolve_ingest(&root, "20260804", "device", "120000_1", &files).unwrap()
        else {
            panic!("expected apply plan");
        };
        assert!(matches!(
            plan.files[0].disposition,
            FileDisposition::Held {
                evidence: HeldEvidence::OnDisk
            }
        ));
        fs::write(
            plan.segment
                .path()
                .join("audio.flac")
                .with_extension("jsonl"),
            json!({
                "_solstone_processing": {
                    "schema": "solstone.processing.v1",
                    "state": "analyzed",
                    "handler": "transcribe",
                    "input_size": 5,
                }
            })
            .to_string()
                + "\n",
        )
        .unwrap();
        fs::remove_file(plan.segment.path().join("audio.flac")).unwrap();

        let result = apply_plan(&plan, &files).unwrap();

        assert_eq!(result.files[0].disposition, AppliedDisposition::AlreadyHeld);
        assert_eq!(result.files[0].evidence, Some(HeldEvidence::TerminalProof));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn manifest_records_written_and_held_files_with_original_request() {
        let root = root();
        let files = [file(b"sound")];
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
        let _ = fs::remove_dir_all(root);
    }
}
