// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use solstone_core_journal_io::{DEFAULT_STREAM, PathError, day_path};
use solstone_core_segment::{ContentName, SegmentDir, SegmentError};

use crate::held::{absent_target_held, manifest_entry_matches, read_lenient_manifest, sha256};
use crate::terminal_proof::is_media_name;

/// Python-compatible number of requested-key allocation candidates.
pub const MAX_INGEST_SEGMENT_ATTEMPTS: u64 = 100;

/// One validated incoming file whose bytes remain owned by the wire layer.
pub struct IngestFile<'a> {
    pub name: ContentName,
    pub bytes: &'a [u8],
}

/// Evidence that a planned file is already held by its candidate segment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HeldEvidence {
    OnDisk,
    TerminalProof,
}

/// Why a candidate file needs a new create-exclusive write in the apply phase.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MissingWriteReason {
    MissingContent,
    NewFile,
}

/// Per-file result retained by an accepted candidate plan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FileDisposition {
    Held { evidence: HeldEvidence },
    NeedsWrite { reason: MissingWriteReason },
    Unwritten { reason: UnwrittenReason },
}

/// Why an accepted plan deliberately leaves one incoming file untouched.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnwrittenReason {
    SidecarConflict,
}

/// Immutable incoming facts and disposition for one accepted candidate file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlannedFile {
    pub name: ContentName,
    pub sha256: String,
    pub size: u64,
    pub disposition: FileDisposition,
}

/// Response status for an accepted candidate plan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlanStatus {
    Ok,
    Collision,
    Duplicate,
}

/// Candidate selected for the later apply phase.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApplyPlan {
    pub status: PlanStatus,
    pub requested_segment: String,
    pub landed_segment: String,
    pub segment: SegmentDir,
    pub files: Vec<PlannedFile>,
    /// Whether candidate preparation created the target segment directory.
    pub created_segment: bool,
}

/// A terminal sidecar conflict in an otherwise content-identical candidate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConflictPlan {
    pub requested_segment: String,
    pub existing_segment: String,
    pub conflicting_files: Vec<ContentName>,
}

/// A fully evaluated request for which no bounded fresh key was available.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FailedPlan {
    pub requested_segment: String,
}

/// Pure resolution result. Conflict and failed outcomes are expected outcomes,
/// while unreadable or unsafe journal state is returned as [`ResolveError`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Resolution {
    Apply(ApplyPlan),
    Conflict(ConflictPlan),
    Failed(FailedPlan),
}

/// Read/path failures that prevent a trustworthy resolution outcome.
#[derive(Debug)]
pub enum ResolveError {
    Io { path: PathBuf, source: io::Error },
    Path(PathError),
    Segment(SegmentError),
    SegmentInput(&'static str),
}

impl fmt::Display for ResolveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(formatter, "{}: {source}", path.display()),
            Self::Path(error) => error.fmt(formatter),
            Self::Segment(error) => error.fmt(formatter),
            Self::SegmentInput(message) => formatter.write_str(message),
        }
    }
}

impl Error for ResolveError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Path(error) => Some(error),
            Self::Segment(error) => Some(error),
            Self::SegmentInput(_) => None,
        }
    }
}

impl From<PathError> for ResolveError {
    fn from(error: PathError) -> Self {
        Self::Path(error)
    }
}

impl From<SegmentError> for ResolveError {
    fn from(error: SegmentError) -> Self {
        Self::Segment(error)
    }
}

#[derive(Clone, Debug)]
struct IncomingFact {
    name: ContentName,
    sha256: String,
    size: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CandidateDisposition {
    Held(HeldEvidence),
    NeedsWrite(MissingWriteReason),
    ContentConflict,
    SidecarConflict,
}

/// Resolve an ingest request without creating, replacing, appending, or locking
/// journal state.
pub fn resolve_ingest(
    journal_root: &Path,
    day: &str,
    stream: &str,
    requested_segment: &str,
    files: &[IngestFile<'_>],
) -> Result<Resolution, ResolveError> {
    if files.is_empty() {
        return Err(ResolveError::SegmentInput("ingest files must not be empty"));
    }
    let requested_handle = SegmentDir::resolve(journal_root, day, requested_segment, stream)?;
    let incoming = incoming_facts(files);
    let content_names = incoming_content_names(&incoming);
    let has_media_identity = incoming
        .iter()
        .any(|file| content_names.contains(&file.name) && is_media_name(file.name.as_str()));

    let stream_dir = stream_directory(journal_root, day, stream)?;
    for candidate in candidate_names(&stream_dir, requested_segment)? {
        let segment = if candidate == requested_segment {
            requested_handle.clone()
        } else {
            SegmentDir::resolve(journal_root, day, &candidate, stream)?
        };
        let dispositions = classify_candidate(&segment, &incoming, &content_names)?;
        if dispositions
            .iter()
            .any(|(_, disposition)| *disposition == CandidateDisposition::ContentConflict)
        {
            continue;
        }
        if dispositions.iter().any(|(_, disposition)| {
            *disposition == CandidateDisposition::NeedsWrite(MissingWriteReason::MissingContent)
        }) {
            return Ok(Resolution::Apply(plan_for_candidate(
                requested_segment,
                candidate,
                segment,
                &incoming,
                &dispositions,
            )));
        }
        let sidecars: Vec<ContentName> = dispositions
            .iter()
            .filter(|(_, disposition)| *disposition == CandidateDisposition::SidecarConflict)
            .map(|(file, _)| file.name.clone())
            .collect();
        if has_media_identity && !sidecars.is_empty() {
            return Ok(Resolution::Conflict(ConflictPlan {
                requested_segment: requested_segment.to_owned(),
                existing_segment: candidate,
                conflicting_files: sidecars,
            }));
        }
        if dispositions.iter().any(|(_, disposition)| {
            *disposition == CandidateDisposition::NeedsWrite(MissingWriteReason::NewFile)
        }) {
            return Ok(Resolution::Apply(plan_for_candidate(
                requested_segment,
                candidate,
                segment,
                &incoming,
                &dispositions,
            )));
        }
        return Ok(Resolution::Apply(plan_for_candidate(
            requested_segment,
            candidate,
            segment,
            &incoming,
            &dispositions,
        )));
    }

    let Some((start, duration)) = requested_segment.split_once('_') else {
        return Err(ResolveError::SegmentInput(
            "segment must contain a duration",
        ));
    };
    let duration = duration
        .parse::<u64>()
        .map_err(|_| ResolveError::SegmentInput("segment duration must be an integer"))?;
    for offset in 0..MAX_INGEST_SEGMENT_ATTEMPTS {
        let candidate = if offset == 0 {
            requested_segment.to_owned()
        } else {
            let Some(adjusted_duration) = duration.checked_add(offset) else {
                return Err(ResolveError::SegmentInput("segment allocation overflow"));
            };
            format!("{start}_{adjusted_duration}")
        };
        let segment = if candidate == requested_segment {
            requested_handle.clone()
        } else {
            SegmentDir::resolve(journal_root, day, &candidate, stream)?
        };
        if !segment_exists(segment.path())? {
            return Ok(Resolution::Apply(ApplyPlan {
                status: status_for(requested_segment, &candidate, false),
                requested_segment: requested_segment.to_owned(),
                landed_segment: candidate,
                segment,
                files: incoming
                    .iter()
                    .map(|file| PlannedFile {
                        name: file.name.clone(),
                        sha256: file.sha256.clone(),
                        size: file.size,
                        disposition: FileDisposition::NeedsWrite {
                            reason: MissingWriteReason::NewFile,
                        },
                    })
                    .collect(),
                created_segment: true,
            }));
        }
    }
    Ok(Resolution::Failed(FailedPlan {
        requested_segment: requested_segment.to_owned(),
    }))
}

fn incoming_facts(files: &[IngestFile<'_>]) -> Vec<IncomingFact> {
    files
        .iter()
        .map(|file| IncomingFact {
            name: file.name.clone(),
            sha256: sha256(file.bytes),
            size: file.bytes.len() as u64,
        })
        .collect()
}

fn incoming_content_names(files: &[IncomingFact]) -> BTreeSet<ContentName> {
    let media: BTreeSet<ContentName> = files
        .iter()
        .filter(|file| is_media_name(file.name.as_str()))
        .map(|file| file.name.clone())
        .collect();
    if media.is_empty() {
        files.iter().map(|file| file.name.clone()).collect()
    } else {
        media
    }
}

fn stream_directory(journal_root: &Path, day: &str, stream: &str) -> Result<PathBuf, ResolveError> {
    let day_dir = day_path(journal_root, Some(day), false)?;
    if stream == DEFAULT_STREAM {
        Ok(day_dir)
    } else {
        Ok(day_dir.join(stream))
    }
}

fn candidate_names(
    stream_dir: &Path,
    requested_segment: &str,
) -> Result<Vec<String>, ResolveError> {
    let entries = match fs::read_dir(stream_dir) {
        Ok(entries) => entries,
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            return Ok(Vec::new());
        }
        Err(source) => {
            return Err(ResolveError::Io {
                path: stream_dir.to_path_buf(),
                source,
            });
        }
    };
    let start = requested_segment
        .split_once('_')
        .map_or(requested_segment, |(start, _)| start);
    let mut candidates = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| ResolveError::Io {
            path: stream_dir.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        let is_directory = match fs::metadata(&path) {
            Ok(metadata) => metadata.is_dir(),
            Err(error) if error.kind() == io::ErrorKind::NotFound => false,
            Err(source) => return Err(ResolveError::Io { path, source }),
        };
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if is_directory && name.starts_with(&format!("{start}_")) {
            candidates.push(name);
        }
    }
    candidates.sort();
    if let Some(position) = candidates.iter().position(|name| name == requested_segment) {
        let requested = candidates.remove(position);
        candidates.insert(0, requested);
    }
    Ok(candidates)
}

/// Classify incoming files using the native ingest content and processing rules.
///
/// An existing target is held only when its digest matches; otherwise it is a
/// content conflict for an incoming identity name and a sidecar conflict for
/// every other name. For an absent target, a terminal-processing proof is
/// consulted only when there is no manifest entry or its entry matches the
/// incoming digest and size. A mismatched manifest entry bypasses that proof:
/// it is a content conflict for identity content and a new file otherwise.
/// With no holding proof, identity content becomes missing content and every
/// other file becomes a new file. Candidate outcome selection intentionally
/// gives missing content priority over a coexisting sidecar conflict; do not
/// simplify that ordering.
fn classify_candidate(
    segment: &SegmentDir,
    files: &[IncomingFact],
    content_names: &BTreeSet<ContentName>,
) -> Result<Vec<(IncomingFact, CandidateDisposition)>, ResolveError> {
    let manifest = read_lenient_manifest(segment.path());
    files
        .iter()
        .cloned()
        .map(|file| {
            let content_identity = content_names.contains(&file.name);
            let target = segment.path().join(file.name.as_str());
            let disposition = match fs::read(&target) {
                Ok(existing) if sha256(&existing) == file.sha256 => {
                    CandidateDisposition::Held(HeldEvidence::OnDisk)
                }
                Ok(_) => conflict_for(content_identity),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    let manifest_entry = manifest.get(file.name.as_str());
                    if absent_target_held(segment, &file.name, &file.sha256, file.size, &manifest) {
                        CandidateDisposition::Held(HeldEvidence::TerminalProof)
                    } else if manifest_entry.is_some_and(|entry| {
                        !manifest_entry_matches(entry, &file.sha256, file.size)
                    }) {
                        if content_identity {
                            CandidateDisposition::ContentConflict
                        } else {
                            CandidateDisposition::NeedsWrite(MissingWriteReason::NewFile)
                        }
                    } else if content_identity {
                        CandidateDisposition::NeedsWrite(MissingWriteReason::MissingContent)
                    } else {
                        CandidateDisposition::NeedsWrite(MissingWriteReason::NewFile)
                    }
                }
                Err(source) => {
                    return Err(ResolveError::Io {
                        path: target,
                        source,
                    });
                }
            };
            Ok((file, disposition))
        })
        .collect()
}

fn conflict_for(content_identity: bool) -> CandidateDisposition {
    // With no incoming media, content identity is every non-reserved incoming
    // file. Any conflict is therefore content, never sidecar: reject this
    // candidate and reallocate instead of silently dropping the file as `ok`.
    if content_identity {
        CandidateDisposition::ContentConflict
    } else {
        CandidateDisposition::SidecarConflict
    }
}

fn plan_for_candidate(
    requested_segment: &str,
    landed_segment: String,
    segment: SegmentDir,
    files: &[IncomingFact],
    dispositions: &[(IncomingFact, CandidateDisposition)],
) -> ApplyPlan {
    let has_missing_content = dispositions.iter().any(|(_, disposition)| {
        *disposition == CandidateDisposition::NeedsWrite(MissingWriteReason::MissingContent)
    });
    ApplyPlan {
        status: status_for(
            requested_segment,
            &landed_segment,
            !has_missing_content && all_held(dispositions),
        ),
        requested_segment: requested_segment.to_owned(),
        landed_segment,
        segment,
        files: files
            .iter()
            .zip(dispositions)
            .map(|(file, (_, disposition))| PlannedFile {
                name: file.name.clone(),
                sha256: file.sha256.clone(),
                size: file.size,
                disposition: match disposition {
                    CandidateDisposition::Held(evidence) => FileDisposition::Held {
                        evidence: *evidence,
                    },
                    CandidateDisposition::NeedsWrite(reason) => {
                        FileDisposition::NeedsWrite { reason: *reason }
                    }
                    CandidateDisposition::SidecarConflict => FileDisposition::Unwritten {
                        reason: UnwrittenReason::SidecarConflict,
                    },
                    CandidateDisposition::ContentConflict => {
                        unreachable!("content-conflicted candidates are rejected")
                    }
                },
            })
            .collect(),
        created_segment: false,
    }
}

fn all_held(dispositions: &[(IncomingFact, CandidateDisposition)]) -> bool {
    dispositions
        .iter()
        .all(|(_, disposition)| matches!(disposition, CandidateDisposition::Held(_)))
}

fn status_for(requested: &str, landed: &str, duplicate: bool) -> PlanStatus {
    if duplicate {
        PlanStatus::Duplicate
    } else if requested == landed {
        PlanStatus::Ok
    } else {
        PlanStatus::Collision
    }
}

fn segment_exists(path: &Path) -> Result<bool, ResolveError> {
    match fs::metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(ResolveError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::UNIX_EPOCH;

    use serde_json::{Value, json};
    use solstone_core_segment::ContentName;

    use super::*;

    fn root() -> tempfile::TempDir {
        let temporary = tempfile::TempDir::new().unwrap();
        fs::create_dir(temporary.path().join("journal")).unwrap();
        temporary
    }

    fn file<'a>(name: &str, bytes: &'a [u8]) -> IngestFile<'a> {
        IngestFile {
            name: ContentName::new(name).unwrap(),
            bytes,
        }
    }

    fn segment(root: &Path, day: &str, stream: &str, key: &str) -> PathBuf {
        let path = root.join("chronicle").join(day).join(stream).join(key);
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn plan(result: Resolution) -> ApplyPlan {
        match result {
            Resolution::Apply(plan) => plan,
            other => panic!("expected apply plan, got {other:?}"),
        }
    }

    fn resolve_with_processing_record(record: Value) -> ApplyPlan {
        resolve_with_processing_record_for_file("audio.flac", record)
    }

    fn resolve_with_processing_record_for_file(name: &str, record: Value) -> ApplyPlan {
        let temporary = root();
        let journal = temporary.path().join("journal");
        let bytes = b"sound";
        let directory = segment(&journal, "20260804", "device", "120000_60");
        fs::write(
            directory.join(Path::new(name).with_extension("jsonl")),
            json!({"_solstone_processing":record}).to_string() + "\nsecond\n",
        )
        .unwrap();

        plan(
            resolve_ingest(
                &journal,
                "20260804",
                "device",
                "120000_60",
                &[file(name, bytes)],
            )
            .unwrap(),
        )
    }

    #[test]
    fn candidate_order_is_requested_then_lexicographic() {
        let temporary = root();
        let journal = temporary.path().join("journal");
        let bytes = b"new";
        let requested = segment(&journal, "20260804", "device", "120000_300");
        fs::write(requested.join("notes.json"), b"old").unwrap();
        for key in ["120000_60", "120000_3000"] {
            let directory = segment(&journal, "20260804", "device", key);
            fs::write(directory.join("notes.json"), bytes).unwrap();
        }

        let result = plan(
            resolve_ingest(
                &journal,
                "20260804",
                "device",
                "120000_300",
                &[file("notes.json", bytes)],
            )
            .unwrap(),
        );

        assert_eq!(result.landed_segment, "120000_3000");
        assert_eq!(result.status, PlanStatus::Duplicate);
    }

    #[test]
    fn joins_on_disk_bytes_without_a_write() {
        let temporary = root();
        let journal = temporary.path().join("journal");
        let bytes = b"same";
        let directory = segment(&journal, "20260804", "device", "120000_60");
        fs::write(directory.join("notes.json"), bytes).unwrap();

        let result = plan(
            resolve_ingest(
                &journal,
                "20260804",
                "device",
                "120000_60",
                &[file("notes.json", bytes)],
            )
            .unwrap(),
        );

        assert_eq!(
            result.files[0].disposition,
            FileDisposition::Held {
                evidence: HeldEvidence::OnDisk
            }
        );
        assert_eq!(result.status, PlanStatus::Duplicate);
    }

    #[test]
    fn matching_manifest_without_proof_needs_missing_content_write() {
        let temporary = root();
        let journal = temporary.path().join("journal");
        let bytes = b"sound";
        let directory = segment(&journal, "20260804", "device", "120000_60");
        let digest = sha256(bytes);
        fs::write(
            directory.join("ingest.json"),
            json!({"schema_version":1,"requested_segment":"120000_60","files":{"audio.flac":{"sha256":digest,"size":bytes.len()}}}).to_string(),
        )
        .unwrap();

        let result = plan(
            resolve_ingest(
                &journal,
                "20260804",
                "device",
                "120000_60",
                &[file("audio.flac", bytes)],
            )
            .unwrap(),
        );

        assert_eq!(
            result.files[0].disposition,
            FileDisposition::NeedsWrite {
                reason: MissingWriteReason::MissingContent
            }
        );
        assert_eq!(result.status, PlanStatus::Ok);
    }

    #[test]
    fn terminal_proof_holds_absent_media() {
        let temporary = root();
        let journal = temporary.path().join("journal");
        let bytes = b"sound";
        let directory = segment(&journal, "20260804", "device", "120000_60");
        fs::write(
            directory.join("audio.jsonl"),
            json!({"_solstone_processing":{"schema":"solstone.processing.v1","state":"analyzed","handler":"transcribe","input_size":bytes.len()}}).to_string() + "\nsecond\n",
        )
        .unwrap();

        let result = plan(
            resolve_ingest(
                &journal,
                "20260804",
                "device",
                "120000_60",
                &[file("audio.flac", bytes)],
            )
            .unwrap(),
        );

        assert_eq!(
            result.files[0].disposition,
            FileDisposition::Held {
                evidence: HeldEvidence::TerminalProof
            }
        );
        assert_eq!(result.status, PlanStatus::Duplicate);
    }

    #[test]
    fn terminal_proof_refuses_schema_mismatch() {
        let result = resolve_with_processing_record(json!({
            "schema":"solstone.processing.v0",
            "state":"analyzed",
            "handler":"transcribe",
            "input_size":5,
        }));

        assert_eq!(
            result.files[0].disposition,
            FileDisposition::NeedsWrite {
                reason: MissingWriteReason::MissingContent
            }
        );
        assert_eq!(result.status, PlanStatus::Ok);
    }

    #[test]
    fn terminal_proof_refuses_state_mismatch() {
        let result = resolve_with_processing_record(json!({
            "schema":"solstone.processing.v1",
            "state":"failed",
            "handler":"transcribe",
            "input_size":5,
        }));

        assert_eq!(
            result.files[0].disposition,
            FileDisposition::NeedsWrite {
                reason: MissingWriteReason::MissingContent
            }
        );
        assert_eq!(result.status, PlanStatus::Ok);
    }

    #[test]
    fn terminal_proof_refuses_handler_mismatch() {
        let result = resolve_with_processing_record(json!({
            "schema":"solstone.processing.v1",
            "state":"analyzed",
            "handler":"describe",
            "input_size":5,
        }));

        assert_eq!(
            result.files[0].disposition,
            FileDisposition::NeedsWrite {
                reason: MissingWriteReason::MissingContent
            }
        );
        assert_eq!(result.status, PlanStatus::Ok);
    }

    #[test]
    fn terminal_proof_refuses_input_size_mismatch() {
        let result = resolve_with_processing_record(json!({
            "schema":"solstone.processing.v1",
            "state":"analyzed",
            "handler":"transcribe",
            "input_size":6,
        }));

        assert_eq!(
            result.files[0].disposition,
            FileDisposition::NeedsWrite {
                reason: MissingWriteReason::MissingContent
            }
        );
        assert_eq!(result.status, PlanStatus::Ok);
    }

    #[test]
    fn terminal_proof_refuses_stemmed_video_handler_mismatch() {
        let result = resolve_with_processing_record_for_file(
            "video.mp4",
            json!({
                "schema":"solstone.processing.v1",
                "state":"analyzed",
                "handler":"transcribe",
                "input_size":5,
            }),
        );

        assert_eq!(
            result.files[0].disposition,
            FileDisposition::NeedsWrite {
                reason: MissingWriteReason::MissingContent
            }
        );
        assert_eq!(result.status, PlanStatus::Ok);
    }

    #[test]
    fn terminal_proof_refuses_stemmed_audio_handler_mismatch() {
        let result = resolve_with_processing_record_for_file(
            "audio.flac",
            json!({
                "schema":"solstone.processing.v1",
                "state":"analyzed",
                "handler":"describe",
                "input_size":5,
            }),
        );

        assert_eq!(
            result.files[0].disposition,
            FileDisposition::NeedsWrite {
                reason: MissingWriteReason::MissingContent
            }
        );
        assert_eq!(result.status, PlanStatus::Ok);
    }

    #[test]
    fn missing_content_outranks_sidecar_conflict() {
        let temporary = root();
        let journal = temporary.path().join("journal");
        let audio = b"sound";
        let notes = b"new-notes";
        let directory = segment(&journal, "20260804", "device", "120000_60");
        fs::write(directory.join("notes.json"), b"old-notes").unwrap();
        fs::write(
            directory.join("ingest.json"),
            json!({"schema_version":1,"files":{"audio.flac":{"sha256":sha256(audio),"size":audio.len()}}}).to_string(),
        )
        .unwrap();

        let result = plan(
            resolve_ingest(
                &journal,
                "20260804",
                "device",
                "120000_60",
                &[file("audio.flac", audio), file("notes.json", notes)],
            )
            .unwrap(),
        );

        assert_eq!(result.status, PlanStatus::Ok);
        assert_eq!(
            result.files[0].disposition,
            FileDisposition::NeedsWrite {
                reason: MissingWriteReason::MissingContent
            }
        );
        assert_eq!(
            result.files[1].disposition,
            FileDisposition::Unwritten {
                reason: UnwrittenReason::SidecarConflict
            }
        );
    }

    #[test]
    fn no_media_content_conflict_reallocates_instead_of_dropping() {
        let temporary = root();
        let journal = temporary.path().join("journal");
        let directory = segment(&journal, "20260804", "device", "120000_300");
        fs::write(directory.join("notes.json"), b"old").unwrap();

        let result = plan(
            resolve_ingest(
                &journal,
                "20260804",
                "device",
                "120000_300",
                &[file("notes.json", b"new")],
            )
            .unwrap(),
        );

        assert_eq!(result.landed_segment, "120000_301");
        assert_eq!(result.status, PlanStatus::Collision);
        assert!(matches!(
            result.files[0].disposition,
            FileDisposition::NeedsWrite {
                reason: MissingWriteReason::NewFile
            }
        ));
    }

    #[test]
    fn attempt_bound_finds_first_free_key_and_reports_exhaustion() {
        let temporary = root();
        let journal = temporary.path().join("journal");
        let first = segment(&journal, "20260804", "device", "120000_300");
        fs::write(first.join("notes.json"), b"old").unwrap();
        let free = plan(
            resolve_ingest(
                &journal,
                "20260804",
                "device",
                "120000_300",
                &[file("notes.json", b"new")],
            )
            .unwrap(),
        );
        assert_eq!(free.landed_segment, "120000_301");
        assert_eq!(free.status, PlanStatus::Collision);

        let exhausted = root();
        let exhausted_journal = exhausted.path().join("journal");
        for offset in 0..MAX_INGEST_SEGMENT_ATTEMPTS {
            let directory = segment(
                &exhausted_journal,
                "20260804",
                "device",
                &format!("120000_{}", 300 + offset),
            );
            fs::write(directory.join("notes.json"), b"old").unwrap();
        }
        assert!(matches!(
            resolve_ingest(
                &exhausted_journal,
                "20260804",
                "device",
                "120000_300",
                &[file("notes.json", b"new")],
            )
            .unwrap(),
            Resolution::Failed(_)
        ));
    }

    #[test]
    fn resolution_never_creates_journal_state() {
        let temporary = root();
        let journal = temporary.path().join("journal");
        let populated = segment(&journal, "20260804", "device", "120000_60");
        fs::write(populated.join("notes.json"), b"same").unwrap();
        let before = tree_snapshot(&journal);

        resolve_ingest(
            &journal,
            "20260804",
            "device",
            "120000_60",
            &[file("notes.json", b"same")],
        )
        .unwrap();
        resolve_ingest(
            &journal,
            "20260804",
            "device",
            "130000_60",
            &[file("notes.json", b"fresh")],
        )
        .unwrap();
        // This specifically guards against accidentally using day_path(..., true).
        resolve_ingest(
            &journal,
            "20260805",
            "device",
            "120000_60",
            &[file("notes.json", b"absent-day")],
        )
        .unwrap();

        assert_eq!(tree_snapshot(&journal), before);
    }

    #[test]
    fn fresh_requested_key_is_ok() {
        let temporary = root();
        let journal = temporary.path().join("journal");
        let result = plan(
            resolve_ingest(
                &journal,
                "20260804",
                "device",
                "120000_60",
                &[file("notes.json", b"fresh")],
            )
            .unwrap(),
        );
        assert_eq!(result.landed_segment, "120000_60");
        assert_eq!(result.status, PlanStatus::Ok);
    }

    fn tree_snapshot(root: &Path) -> Vec<(PathBuf, u64, u128)> {
        let mut entries = Vec::new();
        collect_snapshot(root, root, &mut entries);
        entries.sort();
        entries
    }

    fn collect_snapshot(root: &Path, path: &Path, entries: &mut Vec<(PathBuf, u64, u128)>) {
        let metadata = fs::metadata(path).unwrap();
        let modified = metadata
            .modified()
            .unwrap()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        entries.push((
            path.strip_prefix(root).unwrap().to_path_buf(),
            metadata.len(),
            modified,
        ));
        if metadata.is_dir() {
            for entry in fs::read_dir(path).unwrap() {
                collect_snapshot(root, &entry.unwrap().path(), entries);
            }
        }
    }
}
