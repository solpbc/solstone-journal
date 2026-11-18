// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Archive evidence and consent data. None of these values authorizes removal.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use solstone_core_journal_io::paths::{DEFAULT_STREAM, SegmentLayout};

use crate::content::{ClosedHandlerSet, ContentName, HandlerRegistry};
use crate::eligibility::{FileVerdict, SidecarFacts, resolve_file};
use crate::receipt::Target;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProofError {
    InvalidBinding,
    InvalidFile,
    InvalidProcessing,
    InvalidConsent,
    Overflow,
    Entropy,
    Encoding,
}

impl fmt::Display for ProofError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "archive evidence refused: {self:?}")
    }
}
impl std::error::Error for ProofError {}

/// The current observation accepted by the existing terminal-proof predicate.
/// This does not establish which historical bytes a handler read.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcceptedProcessingObservation {
    pub record: Option<Value>,
    pub has_analysis_row: bool,
}

impl AcceptedProcessingObservation {
    pub fn capture(
        registry: &dyn HandlerRegistry,
        target: &Target,
        name: &ContentName,
        bytes: u64,
        facts: &SidecarFacts,
    ) -> Result<Self, ProofError> {
        if !matches!(
            resolve_file(
                registry,
                &target.day,
                &target.stream,
                &target.dir,
                name,
                bytes,
                facts,
            ),
            FileVerdict::Releasable(_)
        ) {
            return Err(ProofError::InvalidProcessing);
        }
        Ok(Self {
            record: facts.record.clone(),
            has_analysis_row: facts.has_analysis_row,
        })
    }

    pub fn facts(&self) -> SidecarFacts {
        SidecarFacts {
            record: self.record.clone(),
            has_analysis_row: self.has_analysis_row,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveFileProof {
    pub witness_id: String,
    pub name: String,
    pub bytes: u64,
    pub sha256: String,
    pub archive_path: String,
    pub processing: AcceptedProcessingObservation,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OffloadProof {
    pub journal_id: String,
    pub manifest_id: String,
    pub target: Target,
    pub layout: SegmentLayout,
    pub repository_id: String,
    pub snapshot_id: String,
    pub files: Vec<ArchiveFileProof>,
}

fn plain_component(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && !value
            .chars()
            .any(|c| c.is_control() || c == '/' || c == '\\')
}

use solstone_core_backup::archive_readback::exact_archive_path;
pub use solstone_core_backup::archive_readback::full_hex_identity;

impl OffloadProof {
    pub fn validate(&self) -> Result<(), ProofError> {
        if !solstone_core_journal_io::journal_root::is_canonical_identity_observation(
            &self.journal_id,
        ) || !plain_component(&self.manifest_id)
            || self.target.day.len() != 8
            || !self.target.day.bytes().all(|byte| byte.is_ascii_digit())
            || solstone_core_time::parse_day(&self.target.day).is_none()
            || !plain_component(&self.target.stream)
            || (self.layout == SegmentLayout::Direct && self.target.stream != DEFAULT_STREAM)
            || !plain_component(&self.target.dir)
            || !full_hex_identity(&self.repository_id)
            || !full_hex_identity(&self.snapshot_id)
            || self.files.is_empty()
        {
            return Err(ProofError::InvalidBinding);
        }
        let mut previous: Option<&str> = None;
        let mut witnesses = std::collections::BTreeSet::new();
        for file in &self.files {
            let name = ContentName::new(&file.name).ok_or(ProofError::InvalidFile)?;
            if !plain_component(&file.name)
                || previous.is_some_and(|previous| previous >= file.name.as_str())
                || !plain_component(&file.witness_id)
                || !witnesses.insert(file.witness_id.as_str())
                || !full_hex_identity(&file.sha256)
                || !exact_archive_path(&file.archive_path)
                || file.archive_path.rsplit('/').next() != Some(file.name.as_str())
            {
                return Err(ProofError::InvalidFile);
            }
            AcceptedProcessingObservation::capture(
                &ClosedHandlerSet,
                &self.target,
                &name,
                file.bytes,
                &file.processing.facts(),
            )?;
            previous = Some(&file.name);
        }
        self.bytes()?;
        Ok(())
    }

    pub fn bytes(&self) -> Result<u64, ProofError> {
        self.files.iter().try_fold(0_u64, |total, file| {
            total.checked_add(file.bytes).ok_or(ProofError::Overflow)
        })
    }
}

macro_rules! hex_value {
    ($name:ident) => {
        #[derive(Clone, Debug, Eq, PartialEq, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);
        impl $name {
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let value = String::deserialize(deserializer)?;
                if !full_hex_identity(&value) {
                    return Err(serde::de::Error::custom("invalid consent identity"));
                }
                Ok(Self(value))
            }
        }
    };
}
hex_value!(ConsentNonce);
hex_value!(ApprovalRevision);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OffloadOffer {
    pub proof: OffloadProof,
    pub nonce: ConsentNonce,
    pub revision: ApprovalRevision,
}

fn canonical(value: Value) -> Value {
    match value {
        Value::Object(values) => {
            let sorted: BTreeMap<_, _> = values.into_iter().collect();
            Value::Object(
                sorted
                    .into_iter()
                    .map(|(key, value)| (key, canonical(value)))
                    .collect(),
            )
        }
        Value::Array(values) => Value::Array(values.into_iter().map(canonical).collect()),
        value => value,
    }
}

impl OffloadOffer {
    /// A new offer always requests new consent, even for the same archive.
    pub fn new(proof: OffloadProof) -> Result<Self, ProofError> {
        proof.validate()?;
        let mut random = [0_u8; 32];
        getrandom::fill(&mut random).map_err(|_| ProofError::Entropy)?;
        let nonce = ConsentNonce(random.iter().map(|byte| format!("{byte:02x}")).collect());
        Self::with_nonce(proof, nonce)
    }

    pub(crate) fn with_nonce(proof: OffloadProof, nonce: ConsentNonce) -> Result<Self, ProofError> {
        proof.validate()?;
        let revision = Self::digest(&proof, &nonce)?;
        Ok(Self {
            proof,
            nonce,
            revision,
        })
    }

    fn digest(proof: &OffloadProof, nonce: &ConsentNonce) -> Result<ApprovalRevision, ProofError> {
        let value = serde_json::to_value((proof, nonce)).map_err(|_| ProofError::Encoding)?;
        let bytes = serde_json::to_vec(&canonical(value)).map_err(|_| ProofError::Encoding)?;
        let mut digest = Sha256::new();
        digest.update(b"solstone.offload.offer.v2\0");
        digest.update(bytes);
        Ok(ApprovalRevision(format!("{:x}", digest.finalize())))
    }

    pub fn validate(&self) -> Result<(), ProofError> {
        self.proof.validate()?;
        if !full_hex_identity(self.nonce.as_str())
            || Self::digest(&self.proof, &self.nonce)? != self.revision
        {
            return Err(ProofError::InvalidConsent);
        }
        Ok(())
    }

    pub fn check_revision(&self, revision: &ApprovalRevision) -> Result<(), ProofError> {
        self.validate()?;
        if &self.revision != revision {
            return Err(ProofError::InvalidConsent);
        }
        Ok(())
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]
mod tests {
    use super::*;

    fn proof() -> OffloadProof {
        OffloadProof {
            journal_id: "native-root-v1:unix:0000000000000001:0000000000000002".into(),
            manifest_id: "manifest".into(),
            target: Target {
                day: "20261009".into(),
                stream: DEFAULT_STREAM.into(),
                dir: "120000_60_suffix".into(),
            },
            layout: SegmentLayout::Direct,
            repository_id: "a".repeat(64),
            snapshot_id: "b".repeat(64),
            files: vec![ArchiveFileProof {
                witness_id: "witness".into(),
                name: "audio.flac".into(),
                bytes: 7,
                sha256: "c".repeat(64),
                archive_path: "/private-stage/audio.flac".into(),
                processing: AcceptedProcessingObservation {
                    record: None,
                    has_analysis_row: true,
                },
            }],
        }
    }

    #[test]
    fn new_offer_renews_consent_and_tampering_refuses() {
        let first = OffloadOffer::new(proof()).unwrap();
        let second = OffloadOffer::new(proof()).unwrap();
        assert_ne!(first.nonce, second.nonce);
        assert_ne!(first.revision, second.revision);
        assert!(second.check_revision(&first.revision).is_err());
        let mut changed = first.clone();
        changed.proof.files[0].archive_path = "/other/audio.flac".into();
        assert!(changed.validate().is_err());
        let encoded = serde_json::to_vec(&first).unwrap();
        assert_eq!(
            serde_json::from_slice::<OffloadOffer>(&encoded).unwrap(),
            first
        );
    }

    #[test]
    fn canonical_processing_objects_ignore_insertion_order() {
        let mut first = proof();
        first.files[0].processing.record = Some(serde_json::json!({
            "schema":"solstone.processing.v1", "handler":"transcribe", "state":"empty",
            "input_size":7, "reason_code":"no_decodable_audio", "attempted_at":"2026-10-09T00:00:00Z"
        }));
        let first = OffloadOffer::new(first).unwrap();
        let mut second = first.proof.clone();
        let record = second.files[0].processing.record.take().unwrap();
        let values: BTreeMap<_, _> = record.as_object().unwrap().clone().into_iter().collect();
        second.files[0].processing.record = Some(Value::Object(values.into_iter().rev().collect()));
        assert_eq!(
            OffloadOffer::with_nonce(second, first.nonce.clone())
                .unwrap()
                .revision,
            first.revision
        );
    }

    #[test]
    fn snapshot_volume_paths_roundtrip_and_refuse_alternatives() {
        let exact = r"/\\?\Volume{08fa948d-3336-40fd-b369-dd44a1c89601}/Users/solbuild/AppData/Local/Temp/offload-native-ntfs/.tmp8AdIlH/day/.tmp-offload-d3e400a3875111b7235a56b48c3298dd/audio.flac";
        for path in [
            exact.to_owned(),
            exact
                .to_uppercase()
                .replace("AUDIO.FLAC", "audio.flac")
                .replace("VOLUME", "Volume"),
            "/C:/audio.flac".into(),
        ] {
            let mut proof = proof();
            proof.files[0].archive_path = path.clone();
            let offered = OffloadOffer::new(proof).unwrap();
            let decoded: OffloadOffer =
                serde_json::from_slice(&serde_json::to_vec(&offered).unwrap()).unwrap();
            decoded.validate().unwrap();
            assert_eq!(decoded.proof.files[0].archive_path, path);
        }
        for component in [
            r"\\?\Volume{08fa948d-3336-40fd-b369-dd44a1c8960}",
            r"\\?\Volume{08fa948d3336-40fd-b369-dd44a1c89601}",
            r"\\?\Volume{08fa948d-3336-40fd-b369-dd44a1c8960g}",
            r"\\?\Volume{08fa948d-3336-40fd-b369-dd44a1c89601}}",
            r"\?\Volume{08fa948d-3336-40fd-b369-dd44a1c89601}",
            r"\\.\Volume{08fa948d-3336-40fd-b369-dd44a1c89601}",
            r"\\?\volume{08fa948d-3336-40fd-b369-dd44a1c89601}",
            r"\\?\C:",
            r"\\server\share",
        ] {
            let mut value = proof();
            value.files[0].archive_path = format!("/{component}/audio.flac");
            assert!(value.validate().is_err());
        }
        let volume = r"/\\?\Volume{08fa948d-3336-40fd-b369-dd44a1c89601}";
        for tail in [
            "",
            "/",
            "//audio.flac",
            "/../audio.flac",
            "/./audio.flac",
            r"/stage\part/audio.flac",
            "/stage/other.flac",
            "/stage/\naudio.flac",
        ] {
            let mut value = proof();
            value.files[0].archive_path = format!("{volume}{tail}");
            assert!(value.validate().is_err());
        }
        let mut nested = proof();
        nested.files[0].archive_path = format!("/stage{volume}/audio.flac");
        assert!(nested.validate().is_err());
    }

    #[test]
    fn invalid_paths_identity_order_and_processing_refuse() {
        let mut windows_source = proof();
        windows_source.journal_id =
            "native-root-v1:windows:0000000000000001:01000000000000000100000000000000".into();
        assert!(windows_source.validate().is_ok());
        windows_source.journal_id = "native-root-v1:unix:1:2".into();
        assert!(windows_source.validate().is_err());
        for path in [
            "relative/audio.flac",
            "/x/../audio.flac",
            "/x//audio.flac",
            "/x/other.flac",
        ] {
            let mut value = proof();
            value.files[0].archive_path = path.into();
            assert!(value.validate().is_err());
        }
        let mut value = proof();
        value.target.day = "20260230".into();
        assert!(value.validate().is_err());
        let mut value = proof();
        value.target.stream = "other".into();
        assert!(value.validate().is_err());
        value.layout = SegmentLayout::Named;
        assert!(value.validate().is_ok());
        value.target.stream = DEFAULT_STREAM.into();
        assert!(value.validate().is_ok());
        let mut value = proof();
        value.snapshot_id = "latest".into();
        assert!(value.validate().is_err());
        let mut value = proof();
        value.files[0].processing.has_analysis_row = false;
        assert!(value.validate().is_err());
        let mut value = proof();
        value.files.push(value.files[0].clone());
        assert!(value.validate().is_err());
        let mut value = proof();
        value.files[0].bytes = u64::MAX;
        let mut other = value.files[0].clone();
        other.name = "z.flac".into();
        other.witness_id = "other".into();
        other.archive_path = "/x/z.flac".into();
        value.files.push(other);
        assert_eq!(value.validate(), Err(ProofError::Overflow));
    }
}
