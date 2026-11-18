// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Archive history and fresh retained catalog coverage. Historical records and
//! decoded models describe storage; they do not authorize removal.

use std::ffi::OsStr;
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use solstone_core_journal_io::{
    JournalEntryKind, JournalRoot, media::MediaDirectory, paths::SegmentLayout,
};

use crate::archive_proof::{AcceptedProcessingObservation, ArchiveFileProof, OffloadProof};
use codec::{fold_lines, valid_day};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OffloadFile {
    pub name: String,
    pub bytes: u64,
    pub sha256: String,
}
/// A single archived generation. Historical records leave unstored bindings unknown.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveWitness {
    pub manifest_id: String,
    pub witness_id: String,
    pub snapshot_id: String,
    pub repository_id: Option<String>,
    pub archive_path: Option<String>,
    pub journal_id: Option<String>,
    pub layout: Option<SegmentLayout>,
    pub segment_dir: Option<String>,
    /// Accepted historical observation, never current release authority.
    pub processing: Option<AcceptedProcessingObservation>,
    pub file: OffloadFile,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveManifest {
    pub manifest_id: String,
    pub files: Vec<ArchiveWitness>,
}
/// Reviewed content in a current repository, without historical provenance or
/// removal authority. Decoding this value does not establish owner consent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LegacyRestoreBinding {
    pub adoption_id: String,
    pub revision: String,
    pub legacy_manifest: ArchiveManifest,
    pub target: crate::receipt::Target,
    pub layout: SegmentLayout,
    pub repository_id: String,
    pub snapshot_id: String,
    pub files: Vec<LegacyRestoreFileBinding>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LegacyRestoreFileBinding {
    pub witness_id: String,
    pub name: String,
    pub bytes: u64,
    pub sha256: String,
    pub archive_path: String,
}
impl LegacyRestoreBinding {
    /// Canonical data revision, including the nonce and original file order.
    /// This hash carries no admission, content verification or consent.
    pub fn canonical_revision(&self) -> Result<String, String> {
        use sha2::{Digest, Sha256};

        let bytes = serde_json::to_vec(&(
            &self.adoption_id,
            &self.legacy_manifest,
            &self.target,
            self.layout,
            &self.repository_id,
            &self.snapshot_id,
            &self.files,
        ))
        .map_err(|_| "legacy restore binding could not be encoded")?;
        let mut digest = Sha256::new();
        digest.update(b"solstone.offload.restore-binding.v1\0");
        digest.update(bytes);
        Ok(format!("{:x}", digest.finalize()))
    }

    /// Structural validation only. The caller must freshly join this entire
    /// original manifest and verify its content through the admitted repository.
    pub fn validate(&self) -> Result<(), String> {
        use solstone_core_backup::archive_readback::exact_archive_path;
        use solstone_core_journal_io::name_admission::check_portable_component;

        let manifest = &self.legacy_manifest;
        let prefix = format!("v1:{}:", self.target.day);
        let ordinal = manifest
            .manifest_id
            .strip_prefix(&prefix)
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|ordinal| *ordinal > 0);
        if !codec::digest(&self.adoption_id)
            || !codec::digest(&self.revision)
            || !codec::digest(&self.repository_id)
            || !codec::digest(&self.snapshot_id)
            || solstone_core_time::parse_day(&self.target.day).is_none()
            || !codec::valid_day(&self.target.day)
            || check_portable_component(&self.target.stream).is_err()
            || check_portable_component(&self.target.dir).is_err()
            || (self.layout == SegmentLayout::Direct && self.target.stream != "_default")
            || solstone_core_format::segment::segment_key(&self.target.dir).is_none()
            || ordinal.is_none_or(|ordinal| manifest.manifest_id != format!("{prefix}{ordinal}"))
            || manifest.files.is_empty()
            || manifest.files.len() != self.files.len()
        {
            return Err("legacy restore binding is invalid".into());
        }
        let mut names = std::collections::BTreeSet::new();
        let mut historical_snapshot = None;
        for (original, file) in manifest.files.iter().zip(&self.files) {
            let snapshot = original.snapshot_id.as_str();
            if original.manifest_id != manifest.manifest_id
                || original.witness_id != format!("{}:{}", manifest.manifest_id, original.file.name)
                || original.repository_id.is_some()
                || original.archive_path.is_some()
                || original.journal_id.is_some()
                || original.layout.is_some()
                || original.segment_dir.is_some()
                || original.processing.is_some()
                || snapshot.is_empty()
                || snapshot.len() > 64
                || !snapshot
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                || historical_snapshot.is_some_and(|prior| prior != snapshot)
                || !self.snapshot_id.starts_with(snapshot)
                || file.witness_id != original.witness_id
                || file.name != original.file.name
                || file.bytes != original.file.bytes
                || file.sha256 != original.file.sha256
                || !codec::digest(&file.sha256)
                || check_portable_component(&file.name).is_err()
                || !names.insert(file.name.as_str())
                || !exact_archive_path(&file.archive_path)
                || file.archive_path.rsplit('/').next() != Some(file.name.as_str())
            {
                return Err("legacy restore file binding is invalid".into());
            }
            historical_snapshot = Some(snapshot);
        }
        if self.canonical_revision()? != self.revision {
            return Err("legacy restore binding revision changed".into());
        }
        Ok(())
    }
}
/// Restore-only history retains its exact reviewed binding reference.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LegacyRestoreOperation {
    pub adoption_id: String,
    pub operation: RestoreOperation,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SegmentCatalog {
    pub day: String,
    pub stream: String,
    pub segment: String,
    pub history: Vec<ArchiveManifest>,
    pub restores: Vec<RestoreOperation>,
    pub restore_bindings: Vec<LegacyRestoreBinding>,
    pub legacy_restores: Vec<LegacyRestoreOperation>,
    pub active: Vec<ArchiveWitness>,
    pub skipped_records: u64,
    pub unreadable_ledgers: Vec<String>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DayCatalog {
    pub day: String,
    pub segments: Vec<SegmentCatalog>,
    pub skipped_records: u64,
    pub unreadable_ledgers: Vec<String>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct JournalCatalog {
    pub days: Vec<DayCatalog>,
    pub skipped_records: u64,
    pub unreadable_ledgers: Vec<String>,
}
impl SegmentCatalog {
    pub fn degraded(&self) -> bool {
        self.skipped_records > 0 || !self.unreadable_ledgers.is_empty()
    }
}
impl DayCatalog {
    pub fn degraded(&self) -> bool {
        self.skipped_records > 0 || !self.unreadable_ledgers.is_empty()
    }
}
impl JournalCatalog {
    pub fn degraded(&self) -> bool {
        self.skipped_records > 0 || !self.unreadable_ledgers.is_empty()
    }

    /// Returns uniquely referenced historical data; duplicate adoption IDs are
    /// ambiguous even across days. This lookup grants no restore authority.
    pub fn restore_binding(&self, adoption_id: &str) -> Option<&LegacyRestoreBinding> {
        let mut matches = self
            .days
            .iter()
            .flat_map(|day| &day.segments)
            .flat_map(|segment| &segment.restore_bindings)
            .filter(|binding| binding.adoption_id == adoption_id);
        let binding = matches.next()?;
        matches.next().is_none().then_some(binding)
    }

    /// Finds one exact active historical witness. This data comparison does not
    /// establish current input, consent, catalog coverage or release authority.
    pub fn active_witness(
        &self,
        proof: &OffloadProof,
        file: &ArchiveFileProof,
    ) -> Option<&ArchiveWitness> {
        let key = solstone_core_format::segment::segment_key(&proof.target.dir)?;
        let mut matches = self
            .days
            .iter()
            .filter(|day| day.day == proof.target.day)
            .flat_map(|day| &day.segments)
            .filter(|segment| segment.stream == proof.target.stream && segment.segment == key)
            .flat_map(|segment| &segment.active)
            .filter(|witness| exact_witness(witness, proof, file));
        let witness = matches.next()?;
        matches.next().is_none().then_some(witness)
    }

    /// Finds one exact historical witness, including generations already restored.
    /// This data comparison supplies no current input or mutation authority.
    pub fn historical_witness(
        &self,
        proof: &OffloadProof,
        file: &ArchiveFileProof,
    ) -> Option<&ArchiveWitness> {
        let key = solstone_core_format::segment::segment_key(&proof.target.dir)?;
        let mut matches = self
            .days
            .iter()
            .filter(|day| day.day == proof.target.day)
            .flat_map(|day| &day.segments)
            .filter(|segment| segment.stream == proof.target.stream && segment.segment == key)
            .flat_map(|segment| &segment.history)
            .flat_map(|manifest| &manifest.files)
            .filter(|witness| exact_witness(witness, proof, file));
        let witness = matches.next()?;
        matches.next().is_none().then_some(witness)
    }
}

fn exact_witness(witness: &ArchiveWitness, proof: &OffloadProof, file: &ArchiveFileProof) -> bool {
    witness.manifest_id == proof.manifest_id
        && witness.witness_id == file.witness_id
        && witness.snapshot_id == proof.snapshot_id
        && witness.journal_id.as_deref() == Some(proof.journal_id.as_str())
        && witness.layout == Some(proof.layout)
        && witness.segment_dir.as_deref() == Some(proof.target.dir.as_str())
        && witness.repository_id.as_deref() == Some(proof.repository_id.as_str())
        && witness.archive_path.as_deref() == Some(file.archive_path.as_str())
        && witness.file.name == file.name
        && witness.file.bytes == file.bytes
        && witness.file.sha256 == file.sha256
        && witness.processing.as_ref() == Some(&file.processing)
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RestoreDisposition {
    Published,
    AlreadyPresent,
    Conflict,
    Unverified,
    Unattempted,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RestoreFileV2 {
    pub witness_id: String,
    pub name: String,
    pub disposition: RestoreDisposition,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RestoreOperation {
    pub operation_id: String,
    pub files: Vec<RestoreFileV2>,
}
/// Shared storage codec for archive adapters. Decoded values carry no authority.
#[doc(hidden)]
pub mod codec {
    use std::collections::{BTreeMap, BTreeSet};

    use serde::{Deserialize, Serialize};
    use solstone_core_journal_io::paths::SegmentLayout;

    use super::{
        ArchiveManifest, ArchiveWitness, DayCatalog, LegacyRestoreBinding, LegacyRestoreOperation,
        OffloadFile, RestoreDisposition, RestoreFileV2, RestoreOperation, SegmentCatalog,
    };
    use crate::archive_proof::{ArchiveFileProof, OffloadProof};
    use crate::receipt::Target;

    #[derive(Clone, Debug, Serialize, Deserialize)]
    #[serde(tag = "event_kind", deny_unknown_fields)]
    pub enum Event {
        #[serde(rename = "offload")]
        Archive {
            time: u64,
            day: String,
            stream: String,
            segment: String,
            snapshot_id: String,
            files: Vec<OffloadFile>,
        },
        #[serde(rename = "restore")]
        Restore {
            time: u64,
            day: String,
            stream: String,
            segment: String,
        },
        #[serde(rename = "offload_archive_v2")]
        ArchiveV2 {
            schema_version: u32,
            time: u64,
            day: String,
            stream: String,
            segment: String,
            segment_dir: String,
            layout: SegmentLayout,
            journal_id: String,
            manifest_id: String,
            repository_id: String,
            snapshot_id: String,
            files: Vec<ArchiveFileProof>,
        },
        #[serde(rename = "offload_restore_v2")]
        RestoreV2 {
            schema_version: u32,
            time: u64,
            day: String,
            stream: String,
            segment: String,
            operation_id: String,
            files: Vec<RestoreFileV2>,
        },
        #[serde(rename = "offload_restore_binding_v1")]
        RestoreBindingV1 {
            schema_version: u32,
            time: u64,
            day: String,
            stream: String,
            segment: String,
            binding: LegacyRestoreBinding,
        },
        #[serde(rename = "offload_legacy_restore_v1")]
        LegacyRestoreV1 {
            schema_version: u32,
            time: u64,
            day: String,
            stream: String,
            segment: String,
            segment_dir: String,
            layout: SegmentLayout,
            adoption_id: String,
            operation_id: String,
            files: Vec<RestoreFileV2>,
        },
    }
    pub fn valid_day(day: &str) -> bool {
        day.len() == 8 && day.bytes().all(|byte| byte.is_ascii_digit())
    }
    pub fn component(name: &str) -> bool {
        !name.is_empty() && !matches!(name, "." | "..") && !name.contains(['/', '\\', ':', '\0'])
    }
    pub(super) fn digest(value: &str) -> bool {
        value.len() == 64
            && value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }
    pub fn valid_file(file: &OffloadFile) -> bool {
        component(&file.name) && digest(&file.sha256)
    }
    #[derive(Default)]
    struct Fold {
        catalog: SegmentCatalog,
        // Layout and exact basename are part of the target, not inferred from stream/key.
        active: BTreeMap<(Option<SegmentLayout>, Option<String>, String), ArchiveWitness>,
        latest_legacy_scope: Vec<String>,
    }
    pub fn fold_lines(day: &str, bytes: &[u8]) -> DayCatalog {
        let mut folds = BTreeMap::<(String, String), Fold>::new();
        let mut skipped = 0_u64;
        let mut manifests = BTreeSet::new();
        let mut witnesses = BTreeSet::new();
        let mut operations = BTreeSet::new();
        let mut archive_records = BTreeSet::new();
        let mut adoptions = BTreeSet::new();
        for (index, line) in bytes.split_inclusive(|b| *b == b'\n').enumerate() {
            if !line.ends_with(b"\n") {
                skipped = skipped.saturating_add(1);
                continue;
            }
            if line.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            let Ok(event) = serde_json::from_slice::<Event>(line) else {
                skipped = skipped.saturating_add(1);
                continue;
            };
            if matches!(&event, Event::Archive { .. } | Event::ArchiveV2 { .. }) {
                let Ok(canonical) = serde_json::to_string(&event) else {
                    skipped = skipped.saturating_add(1);
                    continue;
                };
                if !archive_records.insert(canonical) {
                    skipped = skipped.saturating_add(1);
                    continue;
                }
            }
            let (event_day, stream, segment) = match &event {
                Event::Archive {
                    day,
                    stream,
                    segment,
                    ..
                }
                | Event::Restore {
                    day,
                    stream,
                    segment,
                    ..
                }
                | Event::ArchiveV2 {
                    day,
                    stream,
                    segment,
                    ..
                }
                | Event::RestoreV2 {
                    day,
                    stream,
                    segment,
                    ..
                }
                | Event::RestoreBindingV1 {
                    day,
                    stream,
                    segment,
                    ..
                }
                | Event::LegacyRestoreV1 {
                    day,
                    stream,
                    segment,
                    ..
                } => (day, stream, segment),
            };
            if event_day != day || !component(stream) || !component(segment) {
                skipped = skipped.saturating_add(1);
                continue;
            }
            let key = (stream.clone(), segment.clone());
            let fold = folds.entry(key).or_insert_with(|| Fold {
                catalog: SegmentCatalog {
                    day: day.into(),
                    stream: stream.clone(),
                    segment: segment.clone(),
                    ..Default::default()
                },
                ..Default::default()
            });
            let archive =
                match event {
                    Event::Archive {
                        snapshot_id,
                        files,
                        time: _time,
                        ..
                    } => {
                        if snapshot_id.is_empty()
                            || files.is_empty()
                            || files.iter().any(|f| !valid_file(f))
                            || files.iter().map(|f| &f.name).collect::<BTreeSet<_>>().len()
                                != files.len()
                        {
                            skipped = skipped.saturating_add(1);
                            continue;
                        }
                        let Some(ordinal) = index.checked_add(1) else {
                            skipped = skipped.saturating_add(1);
                            continue;
                        };
                        let manifest_id = format!("v1:{day}:{ordinal}");
                        let files = files
                            .into_iter()
                            .map(|file| ArchiveWitness {
                                witness_id: format!("{manifest_id}:{}", file.name),
                                manifest_id: manifest_id.clone(),
                                snapshot_id: snapshot_id.clone(),
                                repository_id: None,
                                archive_path: None,
                                journal_id: None,
                                layout: None,
                                segment_dir: None,
                                processing: None,
                                file,
                            })
                            .collect();
                        Some(ArchiveManifest { manifest_id, files })
                    }
                    Event::ArchiveV2 {
                        schema_version,
                        manifest_id,
                        repository_id,
                        snapshot_id,
                        journal_id,
                        layout,
                        segment_dir,
                        files,
                        time: _time,
                        ..
                    } => {
                        let proof = OffloadProof {
                            journal_id,
                            manifest_id,
                            target: Target {
                                day: day.into(),
                                stream: fold.catalog.stream.clone(),
                                dir: segment_dir,
                            },
                            layout,
                            repository_id,
                            snapshot_id,
                            files,
                        };
                        if schema_version != 2
                        || proof.manifest_id.starts_with("v1:")
                        || manifests.contains(&proof.manifest_id)
                        || !component(&proof.target.dir)
                        // Keep names representable by the catalog's restore decoder.
                        || proof.files.iter().any(|f| {
                            f.witness_id.starts_with("v1:")
                                || witnesses.contains(&f.witness_id)
                                || !component(&f.name)
                        })
                        || proof.validate().is_err()
                        {
                            skipped = skipped.saturating_add(1);
                            continue;
                        }
                        let OffloadProof {
                            journal_id,
                            manifest_id,
                            target,
                            layout,
                            repository_id,
                            snapshot_id,
                            files,
                        } = proof;
                        let files = files
                            .into_iter()
                            .map(|f| ArchiveWitness {
                                manifest_id: manifest_id.clone(),
                                witness_id: f.witness_id,
                                snapshot_id: snapshot_id.clone(),
                                repository_id: Some(repository_id.clone()),
                                journal_id: Some(journal_id.clone()),
                                layout: Some(layout),
                                segment_dir: Some(target.dir.clone()),
                                archive_path: Some(f.archive_path),
                                processing: Some(f.processing),
                                file: OffloadFile {
                                    name: f.name,
                                    bytes: f.bytes,
                                    sha256: f.sha256,
                                },
                            })
                            .collect();
                        Some(ArchiveManifest { manifest_id, files })
                    }
                    Event::Restore { time: _time, .. } => {
                        // Repeated historical restore records reuse this scope, never cascade backwards.
                        let scope = &fold.latest_legacy_scope;
                        fold.active
                            .retain(|_, witness| !scope.contains(&witness.witness_id));
                        None
                    }
                    Event::RestoreBindingV1 {
                        schema_version,
                        binding,
                        time: _time,
                        ..
                    } => {
                        if schema_version != 1
                            || adoptions.contains(&binding.adoption_id)
                            || binding.validate().is_err()
                            || binding.target.day != day
                            || binding.target.stream != fold.catalog.stream
                            || solstone_core_format::segment::segment_key(&binding.target.dir)
                                .as_deref()
                                != Some(fold.catalog.segment.as_str())
                            || fold
                                .catalog
                                .history
                                .iter()
                                .filter(|m| **m == binding.legacy_manifest)
                                .count()
                                != 1
                        {
                            skipped = skipped.saturating_add(1);
                            continue;
                        }
                        adoptions.insert(binding.adoption_id.clone());
                        fold.catalog.restore_bindings.push(binding);
                        None
                    }
                    Event::LegacyRestoreV1 {
                        schema_version,
                        segment_dir,
                        layout,
                        adoption_id,
                        operation_id,
                        files,
                        time: _time,
                        ..
                    } => {
                        let Some(binding) = fold
                            .catalog
                            .restore_bindings
                            .iter()
                            .find(|binding| binding.adoption_id == adoption_id)
                        else {
                            skipped = skipped.saturating_add(1);
                            continue;
                        };
                        if schema_version != 1
                            || binding.target.dir != segment_dir
                            || binding.layout != layout
                            || operation_id.is_empty()
                            || operations.contains(&operation_id)
                            || files.is_empty()
                            || files.iter().any(|file| {
                                !binding.files.iter().any(|bound| {
                                    bound.witness_id == file.witness_id && bound.name == file.name
                                })
                            })
                            || files
                                .iter()
                                .map(|file| &file.witness_id)
                                .collect::<BTreeSet<_>>()
                                .len()
                                != files.len()
                            || files
                                .iter()
                                .map(|file| &file.name)
                                .collect::<BTreeSet<_>>()
                                .len()
                                != files.len()
                        {
                            skipped = skipped.saturating_add(1);
                            continue;
                        }
                        let operation = RestoreOperation {
                            operation_id,
                            files,
                        };
                        operations.insert(operation.operation_id.clone());
                        for file in &operation.files {
                            if matches!(
                                file.disposition,
                                RestoreDisposition::Published | RestoreDisposition::AlreadyPresent
                            ) {
                                fold.active
                                    .retain(|_, witness| witness.witness_id != file.witness_id);
                            }
                        }
                        fold.catalog.restores.push(operation.clone());
                        fold.catalog.legacy_restores.push(LegacyRestoreOperation {
                            adoption_id,
                            operation,
                        });
                        None
                    }
                    Event::RestoreV2 {
                        schema_version,
                        operation_id,
                        files,
                        time: _time,
                        ..
                    } => {
                        if schema_version != 2
                            || operation_id.is_empty()
                            || files.is_empty()
                            || operations.contains(&operation_id)
                            || files.iter().any(|f| {
                                !component(&f.name)
                                    || f.witness_id.is_empty()
                                    || !fold.catalog.history.iter().flat_map(|m| &m.files).any(
                                        |w| w.witness_id == f.witness_id && w.file.name == f.name,
                                    )
                            })
                            || files
                                .iter()
                                .map(|f| &f.witness_id)
                                .collect::<BTreeSet<_>>()
                                .len()
                                != files.len()
                            || files.iter().map(|f| &f.name).collect::<BTreeSet<_>>().len()
                                != files.len()
                        {
                            skipped = skipped.saturating_add(1);
                            continue;
                        }
                        operations.insert(operation_id.clone());
                        fold.catalog.restores.push(RestoreOperation {
                            operation_id,
                            files: files.clone(),
                        });
                        for file in files {
                            if matches!(
                                file.disposition,
                                RestoreDisposition::Published | RestoreDisposition::AlreadyPresent
                            ) {
                                fold.active.retain(|_, w| w.witness_id != file.witness_id);
                            }
                        }
                        None
                    }
                };
            if let Some(archive) = archive {
                // A legacy restore has authority only over a preceding legacy manifest.
                fold.latest_legacy_scope =
                    if archive.files.iter().all(|w| w.repository_id.is_none()) {
                        archive.files.iter().map(|w| w.witness_id.clone()).collect()
                    } else {
                        Vec::new()
                    };
                manifests.insert(archive.manifest_id.clone());
                for witness in &archive.files {
                    witnesses.insert(witness.witness_id.clone());
                    fold.active.insert(
                        (
                            witness.layout,
                            witness.segment_dir.clone(),
                            witness.file.name.clone(),
                        ),
                        witness.clone(),
                    );
                }
                fold.catalog.history.push(archive);
            }
        }
        DayCatalog {
            day: day.into(),
            skipped_records: skipped,
            unreadable_ledgers: vec![],
            segments: folds
                .into_values()
                .map(|mut fold| {
                    fold.catalog.active = fold.active.into_values().collect();
                    fold.catalog.skipped_records = skipped;
                    fold.catalog
                })
                .collect(),
        }
    }
}

/// Fresh catalog coverage for an operation retaining its own admitted root.
///
/// Retained directories and exact leaf observations provide read integrity, not
/// an immutable snapshot or backup-safety capability. Callers retain their own
/// policy/admission and read again at each boundary.
/// Local byte identity and unresolved restore obligations are not checked here.
pub fn read_complete_catalog(root: &Arc<JournalRoot>) -> Result<JournalCatalog, String> {
    read_complete_catalog_with_checkpoint(root, |_| {})
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CatalogReadCheckpoint {
    AfterListing,
    AfterRead,
    BeforeAbsentRecheck,
}

fn read_complete_catalog_with_checkpoint(
    root: &Arc<JournalRoot>,
    mut checkpoint: impl FnMut(CatalogReadCheckpoint),
) -> Result<JournalCatalog, String> {
    root.revalidate_canonical_binding()
        .map_err(|error| error.to_string())?;
    let mut directory =
        MediaDirectory::open(Arc::clone(root), Path::new("")).map_err(|error| error.to_string())?;
    for name in ["health", "offload"] {
        let name = OsStr::new(name);
        match directory
            .open_child(name)
            .map_err(|error| error.to_string())?
        {
            Some(child) => directory = child,
            None => {
                checkpoint(CatalogReadCheckpoint::BeforeAbsentRecheck);
                if directory
                    .open_child(name)
                    .map_err(|error| error.to_string())?
                    .is_some()
                {
                    return Err("offload catalog changed during read".into());
                }
                directory.revalidate().map_err(|error| error.to_string())?;
                root.revalidate_canonical_binding()
                    .map_err(|error| error.to_string())?;
                return Ok(JournalCatalog::default());
            }
        }
    }
    let entries = directory
        .entries(usize::MAX)
        .map_err(|error| error.to_string())?
        .ok_or("offload catalog coverage is incomplete")?;
    checkpoint(CatalogReadCheckpoint::AfterListing);
    let mut catalog = JournalCatalog::default();
    for entry in &entries {
        let file = Path::new(&entry.name);
        if file
            .extension()
            .is_none_or(|extension| extension != "jsonl")
        {
            continue;
        }
        let day = file
            .file_stem()
            .and_then(|stem| stem.to_str())
            .filter(|day| valid_day(day))
            .ok_or("offload catalog has an invalid ledger name")?;
        if entry.kind != JournalEntryKind::RegularFile {
            return Err("offload catalog ledger is not a plain file".into());
        }
        let maximum = usize::try_from(entry.size).map_err(|_| "offload ledger is too large")?;
        let observed = directory
            .read_file_bounded(&entry.name, maximum)
            .map_err(|error| error.to_string())?
            .ok_or("offload catalog changed during read")?;
        if !observed.complete || observed.entry != *entry || observed.bytes.len() != maximum {
            return Err("offload catalog changed during read".into());
        }
        let day = fold_lines(day, &observed.bytes);
        if day.degraded() {
            return Err("offload catalog coverage is incomplete".into());
        }
        catalog.days.push(day);
        checkpoint(CatalogReadCheckpoint::AfterRead);
    }
    let current = directory
        .entries(usize::MAX)
        .map_err(|error| error.to_string())?
        .ok_or("offload catalog coverage is incomplete")?;
    if entries != current {
        return Err("offload catalog changed during read".into());
    }
    directory.revalidate().map_err(|error| error.to_string())?;
    root.revalidate_canonical_binding()
        .map_err(|error| error.to_string())?;
    catalog.days.sort_by(|a, b| a.day.cmp(&b.day));
    Ok(catalog)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod legacy_binding_tests {
    use super::codec::{Event, fold_lines};
    use super::*;
    use crate::receipt::Target;

    const DAY: &str = "20261009";
    const SEGMENT: &str = "120000_60";

    fn archive(day: &str, names: &[&str], time: u64) -> Event {
        Event::Archive {
            time,
            day: day.into(),
            stream: "_default".into(),
            segment: SEGMENT.into(),
            snapshot_id: "b".repeat(8),
            files: names
                .iter()
                .map(|name| OffloadFile {
                    name: (*name).into(),
                    bytes: 7,
                    sha256: "a".repeat(64),
                })
                .collect(),
        }
    }

    fn lines(events: &[Event]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for event in events {
            bytes.extend(serde_json::to_vec(event).unwrap());
            bytes.push(b'\n');
        }
        bytes
    }

    fn binding(day: &str, original: &Event) -> LegacyRestoreBinding {
        let catalog = fold_lines(day, &lines(std::slice::from_ref(original)));
        let manifest = catalog.segments[0].history[0].clone();
        let mut binding = LegacyRestoreBinding {
            adoption_id: "c".repeat(64),
            revision: String::new(),
            files: manifest
                .files
                .iter()
                .map(|witness| LegacyRestoreFileBinding {
                    witness_id: witness.witness_id.clone(),
                    name: witness.file.name.clone(),
                    bytes: witness.file.bytes,
                    sha256: witness.file.sha256.clone(),
                    archive_path: format!("/stage/{}", witness.file.name),
                })
                .collect(),
            legacy_manifest: manifest,
            target: Target {
                day: day.into(),
                stream: "_default".into(),
                dir: format!("{SEGMENT}_fixture"),
            },
            layout: SegmentLayout::Direct,
            repository_id: "d".repeat(64),
            snapshot_id: "b".repeat(64),
        };
        binding.revision = binding.canonical_revision().unwrap();
        binding
    }

    fn binding_event(binding: &LegacyRestoreBinding) -> Event {
        Event::RestoreBindingV1 {
            schema_version: 1,
            time: 2,
            day: binding.target.day.clone(),
            stream: binding.target.stream.clone(),
            segment: SEGMENT.into(),
            binding: binding.clone(),
        }
    }

    fn operation_event(
        binding: &LegacyRestoreBinding,
        dispositions: &[RestoreDisposition],
    ) -> Event {
        Event::LegacyRestoreV1 {
            schema_version: 1,
            time: 4,
            day: binding.target.day.clone(),
            stream: binding.target.stream.clone(),
            segment: SEGMENT.into(),
            segment_dir: binding.target.dir.clone(),
            layout: binding.layout,
            adoption_id: binding.adoption_id.clone(),
            operation_id: "legacy-operation".into(),
            files: binding
                .files
                .iter()
                .zip(dispositions)
                .map(|(file, disposition)| RestoreFileV2 {
                    witness_id: file.witness_id.clone(),
                    name: file.name.clone(),
                    disposition: *disposition,
                })
                .collect(),
        }
    }

    #[test]
    fn whole_manifest_binding_preserves_legacy_data_and_requires_strict_revision() {
        let original = archive(DAY, &["audio.flac", "frame.jpg"], 1);
        let binding = binding(DAY, &original);
        assert_eq!(binding.validate(), Ok(()));
        assert_eq!(binding.legacy_manifest.manifest_id, "v1:20261009:1");
        assert!(binding.legacy_manifest.files.iter().all(
            |witness| witness.repository_id.is_none()
                && witness.archive_path.is_none()
                && witness.journal_id.is_none()
                && witness.layout.is_none()
                && witness.segment_dir.is_none()
                && witness.processing.is_none()
        ));
        let encoded = serde_json::to_vec(&binding).unwrap();
        assert_eq!(
            serde_json::from_slice::<LegacyRestoreBinding>(&encoded).unwrap(),
            binding
        );
        let old = fold_lines(DAY, &lines(std::slice::from_ref(&original)));
        let observed = fold_lines(DAY, &lines(&[original.clone(), binding_event(&binding)]));
        assert!(!observed.degraded());
        assert_eq!(old.segments[0].history, observed.segments[0].history);
        assert_eq!(old.segments[0].active, observed.segments[0].active);
        assert_eq!(observed.segments[0].restore_bindings, vec![binding.clone()]);

        for path in ["manifest", "witness", "file"] {
            let mut value = serde_json::to_value(&binding).unwrap();
            match path {
                "manifest" => value["legacy_manifest"]["unknown"] = true.into(),
                "witness" => value["legacy_manifest"]["files"][0]["unknown"] = true.into(),
                _ => value["files"][0]["unknown"] = true.into(),
            }
            assert!(serde_json::from_value::<LegacyRestoreBinding>(value).is_err());
        }
        let mut changed = binding.clone();
        changed.revision = "e".repeat(64);
        assert_eq!(changed.canonical_revision().unwrap(), binding.revision);
        assert!(changed.validate().is_err());
        changed.adoption_id = "f".repeat(64);
        assert_ne!(changed.canonical_revision().unwrap(), binding.revision);
        changed.revision = changed.canonical_revision().unwrap();
        assert!(changed.validate().is_ok());

        let mut reordered = binding.clone();
        reordered.legacy_manifest.files.reverse();
        reordered.files.reverse();
        reordered.revision = reordered.canonical_revision().unwrap();
        assert!(reordered.validate().is_ok());
        let refused = fold_lines(DAY, &lines(&[original.clone(), binding_event(&reordered)]));
        assert!(refused.degraded());
        assert!(refused.segments[0].restore_bindings.is_empty());
        assert_eq!(refused.segments[0].active, old.segments[0].active);

        for name in ["CON.flac", "raw?.flac"] {
            let nonportable = archive(DAY, &[name], 1);
            assert!(!fold_lines(DAY, &lines(std::slice::from_ref(&nonportable))).degraded());
            assert!(self::binding(DAY, &nonportable).validate().is_err());
        }
        let restored = Event::Restore {
            time: 3,
            day: DAY.into(),
            stream: "_default".into(),
            segment: SEGMENT.into(),
        };
        let prior_scope = fold_lines(DAY, &lines(&[original.clone(), restored.clone()]));
        let with_binding = fold_lines(DAY, &lines(&[original, binding_event(&binding), restored]));
        assert_eq!(
            prior_scope.segments[0].active,
            with_binding.segments[0].active
        );
        assert!(with_binding.segments[0].active.is_empty());
    }

    #[test]
    fn legacy_restore_dispositions_join_exact_binding_and_retire_only_original_generation() {
        use RestoreDisposition::{AlreadyPresent, Conflict, Published, Unattempted, Unverified};
        let original = archive(
            DAY,
            &[
                "audio.flac",
                "frame.jpg",
                "other.flac",
                "unknown.jpg",
                "later.jpg",
            ],
            1,
        );
        let binding = binding(DAY, &original);
        let reviewed = binding_event(&binding);
        let operation = operation_event(
            &binding,
            &[Published, AlreadyPresent, Conflict, Unverified, Unattempted],
        );
        let newer = archive(DAY, &["audio.flac"], 3);
        let mut valid = vec![original.clone(), reviewed.clone(), newer, operation.clone()];
        let observed = fold_lines(DAY, &lines(&valid));
        assert!(!observed.degraded());
        let segment = &observed.segments[0];
        assert_eq!(segment.restores.len(), 1);
        assert_eq!(segment.legacy_restores.len(), 1);
        assert_eq!(segment.restores[0], segment.legacy_restores[0].operation);
        assert_eq!(segment.active.len(), 4);
        assert!(
            segment
                .active
                .iter()
                .any(|file| file.file.name == "audio.flac" && file.manifest_id == "v1:20261009:3")
        );
        assert!(
            !segment
                .active
                .iter()
                .any(|file| file.witness_id == binding.files[1].witness_id)
        );
        valid.push(operation.clone());
        let repeated = fold_lines(DAY, &lines(&valid));
        assert!(repeated.degraded());
        assert_eq!(repeated.segments[0].active, segment.active);
        assert_eq!(repeated.segments[0].restores, segment.restores);

        let without_binding = fold_lines(DAY, &lines(&[original.clone(), operation.clone()]));
        assert!(without_binding.degraded());
        assert_eq!(without_binding.segments[0].active.len(), 5);
        for field in [
            "schema_version",
            "adoption_id",
            "segment_dir",
            "layout",
            "witness_id",
            "name",
        ] {
            let mut value = serde_json::to_value(&operation).unwrap();
            match field {
                "schema_version" => value[field] = 2.into(),
                "layout" => value[field] = "named".into(),
                "witness_id" | "name" => value["files"][0][field] = "wrong".into(),
                _ => value[field] = "wrong".into(),
            }
            let invalid: Event = serde_json::from_value(value).unwrap();
            let refused = fold_lines(DAY, &lines(&[original.clone(), reviewed.clone(), invalid]));
            assert!(refused.degraded(), "{field}");
            assert!(refused.segments[0].restores.is_empty());
            assert_eq!(refused.segments[0].active.len(), 5);
        }
        let duplicate_binding = fold_lines(
            DAY,
            &lines(&[original.clone(), reviewed.clone(), reviewed.clone()]),
        );
        assert!(duplicate_binding.degraded());
        assert_eq!(duplicate_binding.segments[0].restore_bindings.len(), 1);
        let mut torn = lines(&[original, reviewed]);
        torn.extend(serde_json::to_vec(&operation).unwrap());
        let refused = fold_lines(DAY, &torn);
        assert!(refused.degraded());
        assert!(refused.segments[0].restores.is_empty());
        assert_eq!(refused.segments[0].active.len(), 5);
    }

    #[test]
    fn adoption_lookup_refuses_cross_day_ambiguity_without_changing_day_fold() {
        let mut catalog = JournalCatalog::default();
        let mut first = None;
        for day in [DAY, "20261010"] {
            let original = archive(day, &["audio.flac"], 1);
            let binding = binding(day, &original);
            if first.is_none() {
                first = Some(binding.clone());
            }
            let observed = fold_lines(day, &lines(&[original, binding_event(&binding)]));
            assert!(!observed.degraded());
            catalog.days.push(observed);
            if catalog.days.len() == 1 {
                assert_eq!(
                    catalog.restore_binding(&binding.adoption_id),
                    first.as_ref()
                );
            }
        }
        assert!(!catalog.degraded());
        assert!(catalog.restore_binding(&"c".repeat(64)).is_none());
        assert!(catalog.restore_binding(&"f".repeat(64)).is_none());
    }
}

#[cfg(all(test, feature = "full-tests"))]
#[allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::unreachable
)]
#[path = "../tests/support/archive_catalog_reader.rs"]
mod reader_tests;
