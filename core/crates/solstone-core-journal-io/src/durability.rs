// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Unified journal artifact durability authority, classes, and readers.
//!
//! Every file the journal reads is registered in [`JOURNAL_ARTIFACTS`].
//! [`read_json_durable`] and [`read_jsonl_durable`] enforce the declared class
//! policy when reading: `MustBeValid` fails on corruption with the original file
//! untouched; other classes preserve corrupted data as `<stem>.wedged-<stamp><ext>`
//! so journal operations can proceed and `journal doctor` can report what was set aside.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::de::DeserializeOwned;

use crate::atomic::{AtomicWriteOptions, atomic_replace};

/// What the journal may do with an artifact it cannot read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DurabilityClass {
    /// The owner's configuration. Written atomically, never healed: an
    /// unreadable one is an error the owner has to see.
    MustBeValid,
    /// Derived state the next run rebuilds from its sources.
    RegenerableCache,
    /// Bookkeeping whose loss costs at most some repeated work.
    Wipeable,
    /// State with its own repair routine; the reader sets it aside and the
    /// routine rebuilds from what survives.
    Healable,
}

/// The parser/reader paradigm for an artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ParserKind {
    /// Whole-file JSON parsed as a single document.
    WholeJson,
    /// Record-framed newline-delimited JSON. Bad records are individually wedged.
    RecordFramedJsonl,
    /// Custom in-place heartbeat classification (e.g. BoundedMalformed/IdentityMismatch).
    InPlaceClassify,
    /// IPC socket endpoint (stale socket unlinked before bind).
    Socket,
}

/// Strong identifier for every declared journal artifact family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ArtifactId {
    JournalConfig,
    SchedulesConfig,
    ConveyConfig,
    ParentLossActive,
    ParentLossRecord,
    Entity,
    FacetDeclaration,
    SchedulerState,
    DirectDoor,
    DirectDoorGeneration,
    Cadence,
    ProviderRuntimeHealth,
    CortexUseId,
    SpeakersInstallGeneration,
    SpeakersInstallOwner,
    ActivityState,
    AwarenessCurrent,
    HealthMarkerStream,
    HealthMarkerDaily,
    CatchupState,
    DailyAdoption,
    SyncHeartbeat,
    CallosumSock,
    ActivityWork,
    ProviderRetryToken,
    DailyUnits,
    SegmentFacets,
    SegmentSense,
    SegmentSpeakers,
    SegmentSpeakerLabels,
    SegmentSpeakerCorrections,
    SegmentStream,
    SegmentIngest,
    TalentProvenance,
    OperationalLog,
    OffloadLedger,
    EntityReviewCandidates,
    EntityAmbiguities,
    FacetReviewCandidates,
    SpeakerReviewCandidates,
    SpeakerPairReviewCandidates,
    AwarenessLog,
    TalentDayAccumulator,
}

/// One artifact the journal reads, with its declared class and path glob.
#[derive(Debug, Clone, Copy)]
pub struct DurableArtifact {
    pub id: ArtifactId,
    /// Journal-relative path glob (`*` matches one segment, `**` matches any remaining).
    pub path: &'static str,
    pub class: DurabilityClass,
    pub parser: ParserKind,
    pub rationale: &'static str,
}

/// The authoritative list of all journal artifacts.
pub const JOURNAL_ARTIFACTS: &[DurableArtifact] = &[
    // MustBeValid
    DurableArtifact {
        id: ArtifactId::JournalConfig,
        path: "config/journal.json",
        class: DurabilityClass::MustBeValid,
        parser: ParserKind::WholeJson,
        rationale: "the owner's configuration; written under a lock with atomic replace",
    },
    // Healable
    DurableArtifact {
        id: ArtifactId::SchedulesConfig,
        path: "config/schedules.json",
        class: DurabilityClass::Healable,
        parser: ParserKind::WholeJson,
        rationale: "an unreadable file falls back to the default schedules with a diagnostic",
    },
    DurableArtifact {
        id: ArtifactId::ConveyConfig,
        path: "config/convey.json",
        class: DurabilityClass::Healable,
        parser: ParserKind::WholeJson,
        rationale: "navigation and UI configuration; falls back to default layout",
    },
    DurableArtifact {
        id: ArtifactId::ParentLossActive,
        path: "health/parent-loss/active-generation.json",
        class: DurabilityClass::Healable,
        parser: ParserKind::WholeJson,
        rationale: "set aside; the successor is allocated above every generation on disk and the open one is closed from its admissions",
    },
    DurableArtifact {
        id: ArtifactId::ParentLossRecord,
        path: "health/parent-loss/generations/*/record.json",
        class: DurabilityClass::Healable,
        parser: ParserKind::WholeJson,
        rationale: "set aside; the generation is closed from its admissions directory",
    },
    DurableArtifact {
        id: ArtifactId::Entity,
        path: "entities/*/entity.json",
        class: DurabilityClass::Healable,
        parser: ParserKind::WholeJson,
        rationale: "entity metadata; rebuilt or merged by entity store routines",
    },
    DurableArtifact {
        id: ArtifactId::FacetDeclaration,
        path: "facets/*/facet.json",
        class: DurabilityClass::Healable,
        parser: ParserKind::WholeJson,
        rationale: "facet declaration; healed by facet store routines",
    },
    // RegenerableCache
    DurableArtifact {
        id: ArtifactId::SchedulerState,
        path: "health/scheduler.json",
        class: DurabilityClass::RegenerableCache,
        parser: ParserKind::WholeJson,
        rationale: "last-run bookkeeping; an unreadable file reads as an empty map",
    },
    DurableArtifact {
        id: ArtifactId::DirectDoor,
        path: "health/direct-door.json",
        class: DurabilityClass::RegenerableCache,
        parser: ParserKind::WholeJson,
        rationale: "rewritten unconditionally at every boot",
    },
    DurableArtifact {
        id: ArtifactId::DirectDoorGeneration,
        path: "health/.direct-door-generation.json",
        class: DurabilityClass::RegenerableCache,
        parser: ParserKind::WholeJson,
        rationale: "direct door generation tracking sidecar; recreated at boot",
    },
    DurableArtifact {
        id: ArtifactId::Cadence,
        path: "health/cadence.json",
        class: DurabilityClass::RegenerableCache,
        parser: ParserKind::WholeJson,
        rationale: "cadence state; unreadable defaults to empty map",
    },
    DurableArtifact {
        id: ArtifactId::ProviderRuntimeHealth,
        path: "health/providers/runtime/local.json",
        class: DurabilityClass::RegenerableCache,
        parser: ParserKind::WholeJson,
        rationale: "local provider runtime state; set aside and rebuilt on next write",
    },
    DurableArtifact {
        id: ArtifactId::ProviderRuntimeHealth,
        path: "health/providers/runtime/parakeet.json",
        class: DurabilityClass::RegenerableCache,
        parser: ParserKind::WholeJson,
        rationale: "parakeet provider runtime state; set aside and rebuilt on next write",
    },
    DurableArtifact {
        id: ArtifactId::CortexUseId,
        path: "health/cortex-use-id.json",
        class: DurabilityClass::RegenerableCache,
        parser: ParserKind::WholeJson,
        rationale: "cortex use counter; unreadable or negative sets aside and falls back to census",
    },
    DurableArtifact {
        id: ArtifactId::SpeakersInstallGeneration,
        path: "health/speakers-analyze/install-generation.json",
        class: DurabilityClass::RegenerableCache,
        parser: ParserKind::WholeJson,
        rationale: "speakers-analyze helper install generation tracking",
    },
    DurableArtifact {
        id: ArtifactId::SpeakersInstallOwner,
        path: "health/speakers-analyze/owner.json",
        class: DurabilityClass::RegenerableCache,
        parser: ParserKind::WholeJson,
        rationale: "speakers-analyze helper owner tracking",
    },
    DurableArtifact {
        id: ArtifactId::ActivityState,
        path: "awareness/activity_state.json",
        class: DurabilityClass::RegenerableCache,
        parser: ParserKind::WholeJson,
        rationale: "current activity state machine snapshot; rebuilt from chronicle segments",
    },
    DurableArtifact {
        id: ArtifactId::AwarenessCurrent,
        path: "awareness/current.json",
        class: DurabilityClass::RegenerableCache,
        parser: ParserKind::WholeJson,
        rationale: "system awareness current state; regenerated by awareness routines",
    },
    DurableArtifact {
        id: ArtifactId::HealthMarkerStream,
        path: "chronicle/*/health/stream.updated",
        class: DurabilityClass::RegenerableCache,
        parser: ParserKind::WholeJson,
        rationale: "stream health update marker; invalid non-empty copied aside and kept as MalformedNonEmpty",
    },
    DurableArtifact {
        id: ArtifactId::HealthMarkerDaily,
        path: "chronicle/*/health/daily.updated",
        class: DurabilityClass::RegenerableCache,
        parser: ParserKind::WholeJson,
        rationale: "daily health update marker; invalid non-empty copied aside and kept as MalformedNonEmpty",
    },
    // Wipeable
    DurableArtifact {
        id: ArtifactId::CatchupState,
        path: "health/catchup-state.json",
        class: DurabilityClass::Wipeable,
        parser: ParserKind::WholeJson,
        rationale: "retry attempts and backoff watermarks; set aside and rebuilt by the next write",
    },
    DurableArtifact {
        id: ArtifactId::DailyAdoption,
        path: "health/daily-adoption.json",
        class: DurabilityClass::Wipeable,
        parser: ParserKind::WholeJson,
        rationale: "adoption bookkeeping; the accepted unit records are the authority",
    },
    DurableArtifact {
        id: ArtifactId::SyncHeartbeat,
        path: "health/sync/*.check",
        class: DurabilityClass::Wipeable,
        parser: ParserKind::InPlaceClassify,
        rationale: "heartbeats; classified in-place by sync collector without moving the file",
    },
    DurableArtifact {
        id: ArtifactId::CallosumSock,
        path: "health/callosum.sock",
        class: DurabilityClass::Wipeable,
        parser: ParserKind::Socket,
        rationale: "a stale socket is removed before bind",
    },
    DurableArtifact {
        id: ArtifactId::ActivityWork,
        path: "health/activity-work/*.json",
        class: DurabilityClass::Wipeable,
        parser: ParserKind::WholeJson,
        rationale: "activity work in-flight tracker; unreadable sets aside and skips retry",
    },
    DurableArtifact {
        id: ArtifactId::ProviderRetryToken,
        path: "health/providers/runtime/local.retry-token.json",
        class: DurabilityClass::Wipeable,
        parser: ParserKind::WholeJson,
        rationale: "local provider runtime retry token; rebuilt on next retry attempt",
    },
    DurableArtifact {
        id: ArtifactId::ProviderRetryToken,
        path: "health/providers/runtime/parakeet.retry-token.json",
        class: DurabilityClass::Wipeable,
        parser: ParserKind::WholeJson,
        rationale: "parakeet provider runtime retry token; rebuilt on next retry attempt",
    },
    DurableArtifact {
        id: ArtifactId::DailyUnits,
        path: "chronicle/*/health/daily-units/*.json",
        class: DurabilityClass::Wipeable,
        parser: ParserKind::WholeJson,
        rationale: "daily talent execution unit records; unreadable sets aside and reports absent",
    },
    DurableArtifact {
        id: ArtifactId::SegmentFacets,
        path: "chronicle/**/talents/facets.json",
        class: DurabilityClass::Wipeable,
        parser: ParserKind::WholeJson,
        rationale: "segment facet classification; unreadable sets aside and skips segment",
    },
    DurableArtifact {
        id: ArtifactId::SegmentSense,
        path: "chronicle/**/talents/sense.json",
        class: DurabilityClass::Wipeable,
        parser: ParserKind::WholeJson,
        rationale: "segment sense output; unreadable sets aside and skips segment in think replay",
    },
    DurableArtifact {
        id: ArtifactId::SegmentSpeakers,
        path: "chronicle/**/talents/speakers.json",
        class: DurabilityClass::Wipeable,
        parser: ParserKind::WholeJson,
        rationale: "segment diarization speakers output",
    },
    DurableArtifact {
        id: ArtifactId::SegmentSpeakerLabels,
        path: "chronicle/**/talents/speaker_labels.json",
        class: DurabilityClass::Wipeable,
        parser: ParserKind::WholeJson,
        rationale: "segment speaker labels output",
    },
    DurableArtifact {
        id: ArtifactId::SegmentSpeakerCorrections,
        path: "chronicle/**/talents/speaker_corrections.json",
        class: DurabilityClass::Wipeable,
        parser: ParserKind::WholeJson,
        rationale: "segment speaker corrections input",
    },
    DurableArtifact {
        id: ArtifactId::SegmentStream,
        path: "chronicle/**/stream.json",
        class: DurabilityClass::Wipeable,
        parser: ParserKind::WholeJson,
        rationale: "capture stream segment record",
    },
    DurableArtifact {
        id: ArtifactId::SegmentIngest,
        path: "chronicle/**/ingest.json",
        class: DurabilityClass::Wipeable,
        parser: ParserKind::WholeJson,
        rationale: "capture ingest provenance record",
    },
    DurableArtifact {
        id: ArtifactId::TalentProvenance,
        path: "chronicle/*/health/talent-provenance/**/*.json",
        class: DurabilityClass::Wipeable,
        parser: ParserKind::WholeJson,
        rationale: "talent execution input provenance hash records",
    },
    DurableArtifact {
        id: ArtifactId::OperationalLog,
        path: "chronicle/*/health/oplog--*",
        class: DurabilityClass::Wipeable,
        parser: ParserKind::RecordFramedJsonl,
        rationale: "canonical operational log files",
    },
    DurableArtifact {
        id: ArtifactId::OffloadLedger,
        path: "health/offload/*.jsonl",
        class: DurabilityClass::Wipeable,
        parser: ParserKind::RecordFramedJsonl,
        rationale: "offload run ledger records",
    },
    DurableArtifact {
        id: ArtifactId::EntityReviewCandidates,
        path: "entities/review-candidates.jsonl",
        class: DurabilityClass::Wipeable,
        parser: ParserKind::RecordFramedJsonl,
        rationale: "entity review candidate items",
    },
    DurableArtifact {
        id: ArtifactId::EntityAmbiguities,
        path: "entities/ambiguities.jsonl",
        class: DurabilityClass::Wipeable,
        parser: ParserKind::RecordFramedJsonl,
        rationale: "entity ambiguity observation records",
    },
    DurableArtifact {
        id: ArtifactId::FacetReviewCandidates,
        path: "facets/review-candidates.jsonl",
        class: DurabilityClass::Wipeable,
        parser: ParserKind::RecordFramedJsonl,
        rationale: "facet review candidate items",
    },
    DurableArtifact {
        id: ArtifactId::SpeakerReviewCandidates,
        path: "speakers/review-candidates.jsonl",
        class: DurabilityClass::Wipeable,
        parser: ParserKind::RecordFramedJsonl,
        rationale: "speaker review candidate items",
    },
    DurableArtifact {
        id: ArtifactId::SpeakerPairReviewCandidates,
        path: "speakers/candidate-pair-review-candidates.jsonl",
        class: DurabilityClass::Wipeable,
        parser: ParserKind::RecordFramedJsonl,
        rationale: "speaker candidate pair review candidate items",
    },
    DurableArtifact {
        id: ArtifactId::AwarenessLog,
        path: "awareness/*.jsonl",
        class: DurabilityClass::Wipeable,
        parser: ParserKind::RecordFramedJsonl,
        rationale: "awareness append log",
    },
    DurableArtifact {
        id: ArtifactId::TalentDayAccumulator,
        path: "chronicle/*/talents/*.jsonl",
        class: DurabilityClass::Wipeable,
        parser: ParserKind::RecordFramedJsonl,
        rationale: "day-level talent accumulator JSONL records",
    },
];

/// Look up the first matching declared artifact by ID.
pub fn artifact(id: ArtifactId) -> &'static DurableArtifact {
    JOURNAL_ARTIFACTS
        .iter()
        .find(|a| a.id == id)
        .expect("declared artifact")
}

/// The slice of all declared artifacts.
pub fn artifacts() -> &'static [DurableArtifact] {
    JOURNAL_ARTIFACTS
}

/// What a durable read found.
#[derive(Debug)]
pub enum DurableRead<T> {
    Present(T),
    Absent,
    /// The artifact could not be parsed and was set aside at this path.
    SetAside(PathBuf),
    /// The artifact could not be read (e.g. permission denied or wrong file kind)
    /// and could not be set aside. Non-MustBeValid callers treat this as Absent.
    Unreadable {
        path: PathBuf,
        error: String,
    },
}

/// Result of reading a record-framed JSONL artifact.
#[derive(Debug)]
pub struct DurableJsonl<T> {
    pub records: Vec<T>,
    pub malformed_line_count: usize,
    pub aside_paths: Vec<PathBuf>,
}

/// Read a whole JSON artifact under its declared artifact identity.
pub fn read_json_durable<T: DeserializeOwned>(
    id: ArtifactId,
    path: &Path,
) -> io::Result<DurableRead<T>> {
    let decl = artifact(id);
    let class = decl.class;

    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(DurableRead::Absent),
        Err(error) => {
            if class == DurabilityClass::MustBeValid {
                return Err(error);
            }
            match set_aside(path) {
                Ok(aside) => return Ok(DurableRead::SetAside(aside)),
                Err(_) => {
                    return Ok(DurableRead::Unreadable {
                        path: path.to_path_buf(),
                        error: error.to_string(),
                    });
                }
            }
        }
    };

    if bytes.trim_ascii().is_empty() {
        return Ok(DurableRead::Absent);
    }

    match serde_json::from_slice(&bytes) {
        Ok(value) => Ok(DurableRead::Present(value)),
        Err(error) if class == DurabilityClass::MustBeValid => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{}: {error}", path.display()),
        )),
        Err(_) => set_aside(path).map(DurableRead::SetAside),
    }
}

/// Read a whole JSON artifact and apply semantic validation.
pub fn read_json_durable_validated<T: DeserializeOwned>(
    id: ArtifactId,
    path: &Path,
    validate: impl FnOnce(&T) -> Result<(), String>,
) -> io::Result<DurableRead<T>> {
    let decl = artifact(id);
    let class = decl.class;

    match read_json_durable::<T>(id, path)? {
        DurableRead::Present(value) => match validate(&value) {
            Ok(()) => Ok(DurableRead::Present(value)),
            Err(semantic_err) => {
                if class == DurabilityClass::MustBeValid {
                    Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("{}: {semantic_err}", path.display()),
                    ))
                } else {
                    set_aside(path).map(DurableRead::SetAside)
                }
            }
        },
        other => Ok(other),
    }
}

/// Read a record-framed JSONL artifact, setting aside any malformed lines and
/// rewriting the clean lines in place.
pub fn read_jsonl_durable<T: DeserializeOwned>(
    _id: ArtifactId,
    path: &Path,
) -> io::Result<DurableJsonl<T>> {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(DurableJsonl {
                records: Vec::new(),
                malformed_line_count: 0,
                aside_paths: Vec::new(),
            });
        }
        Err(error) => return Err(error),
    };

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default();
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("artifact");
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| format!(".{extension}"))
        .unwrap_or_else(|| ".jsonl".to_owned());

    let mut good_records = Vec::new();
    let mut good_lines = Vec::new();
    let mut aside_paths = Vec::new();
    let mut malformed_line_count = 0;

    for (line_idx, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match serde_json::from_str::<T>(trimmed) {
            Ok(record) => {
                good_records.push(record);
                good_lines.push(trimmed);
            }
            Err(_) => {
                malformed_line_count += 1;
                let line_num = line_idx + 1;
                let mut aside = path
                    .with_file_name(format!("{stem}.wedged-{stamp}-line-{line_num}{extension}"));
                let mut sequence = 1;
                while aside.exists() {
                    aside = path.with_file_name(format!(
                        "{stem}.wedged-{stamp}-line-{line_num}-{sequence}{extension}"
                    ));
                    sequence += 1;
                }
                fs::write(&aside, line.as_bytes())?;
                aside_paths.push(aside);
            }
        }
    }

    if malformed_line_count > 0 {
        let new_content = if good_lines.is_empty() {
            String::new()
        } else {
            format!("{}\n", good_lines.join("\n"))
        };
        atomic_replace(path, new_content.as_bytes(), AtomicWriteOptions::default())
            .map_err(|e| io::Error::other(e.to_string()))?;
    }

    Ok(DurableJsonl {
        records: good_records,
        malformed_line_count,
        aside_paths,
    })
}

/// Move an artifact beside itself as `<stem>.wedged-<unix seconds><ext>` and
/// return the new path. Never deletes.
pub fn set_aside(path: &Path) -> io::Result<PathBuf> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default();
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("artifact");
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| format!(".{extension}"))
        .unwrap_or_default();
    let mut aside = path.with_file_name(format!("{stem}.wedged-{stamp}{extension}"));
    let mut sequence = 1;
    while aside.exists() {
        aside = path.with_file_name(format!("{stem}.wedged-{stamp}-{sequence}{extension}"));
        sequence += 1;
    }
    fs::rename(path, &aside)?;
    Ok(aside)
}

/// Copy an artifact beside itself as `<stem>.wedged-<unix seconds><ext>`,
/// leaving the original file in place (used for health markers).
pub fn copy_aside(path: &Path) -> io::Result<PathBuf> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default();
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("artifact");
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| format!(".{extension}"))
        .unwrap_or_default();
    let mut aside = path.with_file_name(format!("{stem}.wedged-{stamp}{extension}"));
    let mut sequence = 1;
    while aside.exists() {
        aside = path.with_file_name(format!("{stem}.wedged-{stamp}-{sequence}{extension}"));
        sequence += 1;
    }
    fs::copy(path, &aside)?;
    Ok(aside)
}

/// The marker every set-aside artifact carries in its name.
pub const SET_ASIDE_MARKER: &str = ".wedged-";

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn a_readable_artifact_is_present() {
        let directory = TempDir::new().expect("temporary root");
        let path = directory.path().join("state.json");
        fs::write(&path, b"{\"n\":1}").expect("write");
        let read = read_json_durable::<serde_json::Value>(ArtifactId::SchedulerState, &path)
            .expect("read");
        assert!(matches!(read, DurableRead::Present(value) if value["n"] == 1));

        assert!(matches!(
            read_json_durable::<serde_json::Value>(
                ArtifactId::CatchupState,
                &directory.path().join("missing.json"),
            )
            .expect("read"),
            DurableRead::Absent
        ));
    }

    #[test]
    fn the_config_class_alone_refuses_an_unreadable_artifact() {
        let directory = TempDir::new().expect("temporary root");
        let path = directory.path().join("journal.json");
        fs::write(&path, b"{ not json").expect("write");
        let error = read_json_durable::<serde_json::Value>(ArtifactId::JournalConfig, &path)
            .expect_err("must-be-valid refuses");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(path.is_file(), "the config is never moved");
    }

    #[test]
    fn non_config_classes_set_aside_and_keep_evidence() {
        for id in [
            ArtifactId::SchedulerState,
            ArtifactId::CatchupState,
            ArtifactId::SchedulesConfig,
        ] {
            let directory = TempDir::new().expect("temporary root");
            let path = directory.path().join("artifact.json");
            fs::write(&path, b"{ not json").expect("write");
            let read =
                read_json_durable::<serde_json::Value>(id, &path).expect("set aside, not error");
            let DurableRead::SetAside(aside) = read else {
                panic!("expected a set-aside read for {id:?}");
            };
            assert!(
                !path.exists(),
                "the damaged artifact no longer sits at its path"
            );
            assert!(aside.is_file(), "the damaged bytes are preserved");
            let name = aside.file_name().unwrap().to_str().unwrap();
            assert!(name.starts_with("artifact.wedged-") && name.ends_with(".json"));
            assert_eq!(fs::read(&aside).expect("preserved bytes"), b"{ not json");
            assert!(matches!(
                read_json_durable::<serde_json::Value>(id, &path).expect("second read"),
                DurableRead::Absent
            ));
        }
    }

    #[test]
    fn semantic_validation_failure_sets_aside() {
        let directory = TempDir::new().expect("temporary root");
        let path = directory.path().join("adoption.json");
        fs::write(&path, b"{\"version\":2}").expect("write");
        let read = read_json_durable_validated::<serde_json::Value>(
            ArtifactId::DailyAdoption,
            &path,
            |val| {
                if val["version"] == 1 {
                    Ok(())
                } else {
                    Err("version must be 1".to_owned())
                }
            },
        )
        .expect("set aside on semantic failure");
        assert!(matches!(read, DurableRead::SetAside(_)));
        assert!(!path.exists());
    }

    #[test]
    fn jsonl_durable_heals_bad_lines_and_preserves_good() {
        let directory = TempDir::new().expect("temporary root");
        let path = directory.path().join("records.jsonl");
        fs::write(
            &path,
            b"{\"n\":1}\n{bad json}\n{\"n\":2}\n{another bad}\n{\"n\":3}\n",
        )
        .expect("write");

        let result = read_jsonl_durable::<serde_json::Value>(ArtifactId::OperationalLog, &path)
            .expect("read jsonl");
        assert_eq!(result.malformed_line_count, 2);
        assert_eq!(result.aside_paths.len(), 2);
        assert_eq!(result.records.len(), 3);
        assert_eq!(result.records[0]["n"], 1);
        assert_eq!(result.records[1]["n"], 2);
        assert_eq!(result.records[2]["n"], 3);

        // Original file was rewritten cleanly
        let remaining = fs::read_to_string(&path).expect("read remaining");
        assert_eq!(remaining, "{\"n\":1}\n{\"n\":2}\n{\"n\":3}\n");

        // Preserved aside files contain bad bytes
        assert_eq!(fs::read(&result.aside_paths[0]).unwrap(), b"{bad json}");
        assert_eq!(fs::read(&result.aside_paths[1]).unwrap(), b"{another bad}");

        // Idempotent read
        let second = read_jsonl_durable::<serde_json::Value>(ArtifactId::OperationalLog, &path)
            .expect("second read");
        assert_eq!(second.malformed_line_count, 0);
        assert_eq!(second.aside_paths.len(), 0);
        assert_eq!(second.records.len(), 3);
    }

    #[test]
    fn a_second_set_aside_in_the_same_second_does_not_overwrite_the_first() {
        let directory = TempDir::new().expect("temporary root");
        let path = directory.path().join("state.json");
        fs::write(&path, b"one").expect("write");
        let first = set_aside(&path).expect("first");
        fs::write(&path, b"two").expect("write again");
        let second = set_aside(&path).expect("second");
        assert_ne!(first, second);
        assert_eq!(fs::read(&first).unwrap(), b"one");
        assert_eq!(fs::read(&second).unwrap(), b"two");
    }

    #[test]
    fn copy_aside_leaves_original_file_intact() {
        let directory = TempDir::new().expect("temporary root");
        let path = directory.path().join("stream.updated");
        fs::write(&path, b"corrupted marker").expect("write");
        let aside = copy_aside(&path).expect("copy aside");
        assert!(path.is_file());
        assert!(aside.is_file());
        assert_eq!(fs::read(&path).unwrap(), b"corrupted marker");
        assert_eq!(fs::read(&aside).unwrap(), b"corrupted marker");
    }

    #[test]
    fn journal_root_or_device_unavailability_is_fatal() {
        let path = Path::new("/nonexistent-root-impossible/nested/state.json");
        let res = read_json_durable::<serde_json::Value>(ArtifactId::SchedulerState, path);
        assert!(matches!(res, Ok(DurableRead::Absent)));
    }

    #[test]
    fn the_inventory_declares_exactly_one_must_be_valid_artifact() {
        let strict: Vec<_> = JOURNAL_ARTIFACTS
            .iter()
            .filter(|artifact| artifact.class == DurabilityClass::MustBeValid)
            .map(|artifact| artifact.path)
            .collect();
        assert_eq!(strict, vec!["config/journal.json"]);
        for artifact in JOURNAL_ARTIFACTS {
            assert!(
                !artifact.rationale.is_empty(),
                "{} has no rationale",
                artifact.path
            );
        }
    }

    #[test]
    fn artifacts_without_semantic_validators_are_known() {
        let no_semantic = [
            ArtifactId::DirectDoor,
            ArtifactId::DirectDoorGeneration,
            ArtifactId::CallosumSock,
            ArtifactId::SpeakersInstallGeneration,
            ArtifactId::SpeakersInstallOwner,
            ArtifactId::ConveyConfig,
            ArtifactId::AwarenessCurrent,
            ArtifactId::SegmentSpeakers,
            ArtifactId::SegmentSpeakerLabels,
            ArtifactId::SegmentSpeakerCorrections,
            ArtifactId::SegmentStream,
            ArtifactId::SegmentIngest,
        ];
        for id in no_semantic {
            assert!(!artifact(id).rationale.is_empty());
        }
    }
}
