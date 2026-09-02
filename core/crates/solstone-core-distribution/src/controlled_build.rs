// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! In-memory controlled-build receipt: identity categories plus a PE census
//! of pre-signing output bytes.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::digest::sha256_hex;
use crate::pe::{self, PeError, PeInfo};
use crate::provenance::Provenance;
use solstone_core_journal_io::{
    AtomicWriteOptions, DetailedAtomicError, ExclusivePublication, FinalNameConfirmation,
    MetadataDurability, StageCleanup, write_bytes_exclusive_detailed,
};

pub const CONTROLLED_BUILD_RECEIPT_SCHEMA_V1: &str = "solstone.controlled-build-receipt.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiptCategory {
    Schema,
    SourceIdentity,
    InputIdentity,
    BuilderIdentity,
    BuildConfiguration,
    OutputIdentity,
    SupportingArtifacts,
    ValidationReference,
}

impl ReceiptCategory {
    fn as_str(self) -> &'static str {
        match self {
            Self::Schema => "schema",
            Self::SourceIdentity => "source identity",
            Self::InputIdentity => "input identity",
            Self::BuilderIdentity => "builder identity",
            Self::BuildConfiguration => "build configuration",
            Self::OutputIdentity => "output identity",
            Self::SupportingArtifacts => "supporting artifacts",
            Self::ValidationReference => "validation reference",
        }
    }
}

impl std::fmt::Display for ReceiptCategory {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceIdentity {
    pub product: Provenance,
    pub windows_dependency: DependencySource,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DependencySource {
    pub repository: String,
    pub revision: String,
    pub content_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputIdentityEntry {
    pub label: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuilderIdentity {
    pub host: String,
    pub toolchain: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildConfiguration {
    pub target_triple: String,
    pub profile: String,
    pub flags: Vec<String>,
    pub network_access_denied: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputIdentityEntry {
    /// SHA-256 of the built PE file's bytes BEFORE any Authenticode or catalog signing.
    /// Never a post-signing digest — signing is out of scope for this receipt.
    pub pre_signing_sha256: String,
    pub label: String,
    pub size: u64,
    pub census: PeInfo,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SupportingArtifactRef {
    pub label: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValidationReference {
    pub description: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlledBuildReceiptDraft {
    pub schema: Option<String>,
    pub source: Option<SourceIdentity>,
    pub inputs: Option<Vec<InputIdentityEntry>>,
    pub builder: Option<BuilderIdentity>,
    pub configuration: Option<BuildConfiguration>,
    pub outputs: Option<Vec<OutputIdentityEntry>>,
    pub supporting: Option<Vec<SupportingArtifactRef>>,
    pub validation: Option<ValidationReference>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlledBuildReceipt {
    pub schema: String,
    pub source: SourceIdentity,
    pub inputs: Vec<InputIdentityEntry>,
    pub builder: BuilderIdentity,
    pub configuration: BuildConfiguration,
    pub outputs: Vec<OutputIdentityEntry>,
    pub supporting: Vec<SupportingArtifactRef>,
    pub validation: ValidationReference,
}

#[derive(Debug)]
pub enum ControlledBuildReceiptError {
    Missing {
        category: ReceiptCategory,
    },
    UnrecognizedSchema {
        found: String,
    },
    OutputParse {
        index: usize,
        label: String,
        source: PeError,
    },
}

impl std::fmt::Display for ControlledBuildReceiptError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing { category } => {
                write!(formatter, "missing required:\n  {category}")
            }
            Self::UnrecognizedSchema { found } => {
                write!(formatter, "unexpected:\n  schema {found}")
            }
            Self::OutputParse {
                index,
                label,
                source,
            } => {
                write!(
                    formatter,
                    "unexpected:\n  output identity[{index}] {label}: {source}"
                )
            }
        }
    }
}

impl std::error::Error for ControlledBuildReceiptError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::OutputParse { source, .. } => Some(source),
            Self::Missing { .. } | Self::UnrecognizedSchema { .. } => None,
        }
    }
}

impl ControlledBuildReceiptDraft {
    pub fn validate(self) -> Result<ControlledBuildReceipt, ControlledBuildReceiptError> {
        if let Some(found) = &self.schema
            && found != CONTROLLED_BUILD_RECEIPT_SCHEMA_V1
        {
            return Err(ControlledBuildReceiptError::UnrecognizedSchema {
                found: found.clone(),
            });
        }
        let schema = self.schema.ok_or(ControlledBuildReceiptError::Missing {
            category: ReceiptCategory::Schema,
        })?;
        let source = self.source.ok_or(ControlledBuildReceiptError::Missing {
            category: ReceiptCategory::SourceIdentity,
        })?;
        let inputs = self.inputs.ok_or(ControlledBuildReceiptError::Missing {
            category: ReceiptCategory::InputIdentity,
        })?;
        let builder = self.builder.ok_or(ControlledBuildReceiptError::Missing {
            category: ReceiptCategory::BuilderIdentity,
        })?;
        let configuration = self
            .configuration
            .ok_or(ControlledBuildReceiptError::Missing {
                category: ReceiptCategory::BuildConfiguration,
            })?;
        let outputs = self.outputs.ok_or(ControlledBuildReceiptError::Missing {
            category: ReceiptCategory::OutputIdentity,
        })?;
        let supporting = self
            .supporting
            .ok_or(ControlledBuildReceiptError::Missing {
                category: ReceiptCategory::SupportingArtifacts,
            })?;
        let validation = self
            .validation
            .ok_or(ControlledBuildReceiptError::Missing {
                category: ReceiptCategory::ValidationReference,
            })?;
        Ok(ControlledBuildReceipt {
            schema,
            source,
            inputs,
            builder,
            configuration,
            outputs,
            supporting,
            validation,
        })
    }
}

impl ControlledBuildReceipt {
    /// Re-run the receipt's existing category and schema checks without
    /// changing an already complete receipt into a second schema.
    pub fn validate(&self) -> Result<(), ControlledBuildReceiptError> {
        ControlledBuildReceiptDraft {
            schema: Some(self.schema.clone()),
            source: Some(self.source.clone()),
            inputs: Some(self.inputs.clone()),
            builder: Some(self.builder.clone()),
            configuration: Some(self.configuration.clone()),
            outputs: Some(self.outputs.clone()),
            supporting: Some(self.supporting.clone()),
            validation: Some(self.validation.clone()),
        }
        .validate()
        .map(|_| ())
    }
}

/// Errors from the one canonical receipt representation.
#[derive(Debug)]
pub enum ControlledBuildReceiptCodecError {
    Encode { source: serde_json::Error },
    Decode { source: serde_json::Error },
    Validation { source: ControlledBuildReceiptError },
}

impl std::fmt::Display for ControlledBuildReceiptCodecError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Encode { source } => write!(formatter, "could not encode receipt: {source}"),
            Self::Decode { source } => write!(formatter, "could not decode receipt: {source}"),
            Self::Validation { source } => write!(formatter, "invalid receipt: {source}"),
        }
    }
}

impl std::error::Error for ControlledBuildReceiptCodecError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Encode { source } | Self::Decode { source } => Some(source),
            Self::Validation { source } => Some(source),
        }
    }
}

/// Encode the complete V1 receipt in stable struct-field order.
///
/// The model has no map-valued fields, so JSON object ordering is entirely
/// determined by the declared receipt and census structures. This is a
/// structural receipt document only; it does not rehash the values recorded
/// in the document.
pub fn encode_controlled_build_receipt(
    receipt: &ControlledBuildReceipt,
) -> Result<Vec<u8>, ControlledBuildReceiptCodecError> {
    receipt
        .validate()
        .map_err(|source| ControlledBuildReceiptCodecError::Validation { source })?;
    serde_json::to_vec(receipt)
        .map_err(|source| ControlledBuildReceiptCodecError::Encode { source })
}

/// Decode and re-run the existing V1 category and schema validation.
///
/// The draft form intentionally makes an absent category distinguishable from
/// an explicitly present empty list before `validate` reconstructs the
/// complete receipt.
pub fn decode_controlled_build_receipt(
    bytes: &[u8],
) -> Result<ControlledBuildReceipt, ControlledBuildReceiptCodecError> {
    let draft = serde_json::from_slice::<ControlledBuildReceiptDraft>(bytes)
        .map_err(|source| ControlledBuildReceiptCodecError::Decode { source })?;
    draft
        .validate()
        .map_err(|source| ControlledBuildReceiptCodecError::Validation { source })
}

/// The caller-visible result of receipt publication.
///
/// Only `Durable` has all three shared-I/O facts confirmed. That remains a
/// structural receipt result, not a signing admission: a later artifact
/// verifier must rehash and re-census output bytes before any pre-signing
/// decision. `PublishedButNotDurable` is the normal Windows result: the final
/// name and stage cleanup are confirmed, while parent-directory metadata
/// durability is accurately reported as unproven. Every other incomplete fact
/// is retained verbatim in `PublicationUnconfirmed`.
#[derive(Debug)]
pub enum ControlledBuildReceiptPublication {
    Durable {
        path: PathBuf,
        receipt: ControlledBuildReceipt,
        publication: ExclusivePublication,
    },
    PublishedButNotDurable {
        path: PathBuf,
        receipt: ControlledBuildReceipt,
        publication: ExclusivePublication,
    },
    PublicationUnconfirmed {
        path: PathBuf,
        receipt: ControlledBuildReceipt,
        publication: ExclusivePublication,
    },
}

impl ControlledBuildReceiptPublication {
    #[must_use]
    pub fn is_durable(&self) -> bool {
        matches!(self, Self::Durable { .. })
    }

    /// Receipt persistence never admits signing. Artifact-byte revalidation
    /// and signing admission are intentionally owned by a later seam.
    #[must_use]
    pub fn is_signing_eligible(&self) -> bool {
        false
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        match self {
            Self::Durable { path, .. }
            | Self::PublishedButNotDurable { path, .. }
            | Self::PublicationUnconfirmed { path, .. } => path,
        }
    }

    #[must_use]
    pub fn publication(&self) -> &ExclusivePublication {
        match self {
            Self::Durable { publication, .. }
            | Self::PublishedButNotDurable { publication, .. }
            | Self::PublicationUnconfirmed { publication, .. } => publication,
        }
    }
}

/// Role- and path-specific failures while reading or publishing a receipt.
#[derive(Debug)]
pub enum ControlledBuildReceiptPersistenceError {
    Encode {
        destination: PathBuf,
        source: ControlledBuildReceiptCodecError,
    },
    Publish {
        destination: PathBuf,
        source: DetailedAtomicError,
    },
    Read {
        path: PathBuf,
        source: io::Error,
    },
    Decode {
        path: PathBuf,
        source: ControlledBuildReceiptCodecError,
    },
}

impl std::fmt::Display for ControlledBuildReceiptPersistenceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Encode {
                destination,
                source,
            } => write!(
                formatter,
                "controlled-build receipt writer {}: {source}",
                destination.display()
            ),
            Self::Publish {
                destination,
                source,
            } => write!(
                formatter,
                "controlled-build receipt writer {}: publication failed: {source}",
                destination.display()
            ),
            Self::Read { path, source } => write!(
                formatter,
                "controlled-build receipt reader {}: could not read: {source}",
                path.display()
            ),
            Self::Decode { path, source } => write!(
                formatter,
                "controlled-build receipt reader {}: {source}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for ControlledBuildReceiptPersistenceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Encode { source, .. } | Self::Decode { source, .. } => Some(source),
            Self::Publish { source, .. } => Some(source),
            Self::Read { source, .. } => Some(source),
        }
    }
}

/// Create a receipt name only when it does not already exist.
///
/// This delegates all staging, publication, post-publication observation,
/// metadata durability, and cleanup behavior to the shared detailed rail.
/// It deliberately makes no retry, overwrite, deletion, or signing decision.
pub fn write_controlled_build_receipt_exclusive(
    path: impl AsRef<Path>,
    receipt: &ControlledBuildReceipt,
) -> Result<ControlledBuildReceiptPublication, ControlledBuildReceiptPersistenceError> {
    let destination = path.as_ref().to_path_buf();
    let bytes = encode_controlled_build_receipt(receipt).map_err(|source| {
        ControlledBuildReceiptPersistenceError::Encode {
            destination: destination.clone(),
            source,
        }
    })?;
    let publication =
        write_bytes_exclusive_detailed(&destination, &bytes, AtomicWriteOptions::default())
            .map_err(|source| ControlledBuildReceiptPersistenceError::Publish {
                destination: destination.clone(),
                source,
            })?;
    Ok(classify_receipt_publication(
        destination,
        receipt.clone(),
        publication,
    ))
}

/// Strictly read and structurally re-validate one persisted receipt document.
pub fn read_controlled_build_receipt(
    path: impl AsRef<Path>,
) -> Result<ControlledBuildReceipt, ControlledBuildReceiptPersistenceError> {
    let path = path.as_ref().to_path_buf();
    let bytes = fs::read(&path).map_err(|source| ControlledBuildReceiptPersistenceError::Read {
        path: path.clone(),
        source,
    })?;
    decode_controlled_build_receipt(&bytes)
        .map_err(|source| ControlledBuildReceiptPersistenceError::Decode { path, source })
}

fn classify_receipt_publication(
    path: PathBuf,
    receipt: ControlledBuildReceipt,
    publication: ExclusivePublication,
) -> ControlledBuildReceiptPublication {
    if publication.is_fully_confirmed() {
        return ControlledBuildReceiptPublication::Durable {
            path,
            receipt,
            publication,
        };
    }
    if matches!(
        publication.final_name,
        FinalNameConfirmation::Confirmed { .. }
    ) && matches!(publication.cleanup, StageCleanup::Removed)
        && matches!(publication.durability, MetadataDurability::Unproven { .. })
    {
        return ControlledBuildReceiptPublication::PublishedButNotDurable {
            path,
            receipt,
            publication,
        };
    }
    ControlledBuildReceiptPublication::PublicationUnconfirmed {
        path,
        receipt,
        publication,
    }
}

pub fn census_outputs(
    files: &[(&str, &[u8])],
) -> Result<Vec<OutputIdentityEntry>, ControlledBuildReceiptError> {
    let mut outputs = Vec::with_capacity(files.len());
    for (index, (label, bytes)) in files.iter().enumerate() {
        let census =
            pe::parse_pe(bytes).map_err(|source| ControlledBuildReceiptError::OutputParse {
                index,
                label: (*label).to_owned(),
                source,
            })?;
        outputs.push(OutputIdentityEntry {
            pre_signing_sha256: sha256_hex(bytes),
            label: (*label).to_owned(),
            size: bytes.len() as u64,
            census,
        });
    }
    Ok(outputs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pe::{
        DebugInfoKind, FixtureSpec, ImportSpec, PeSymbolSpec, fixture, fixture_pe32, parse_pe,
    };
    use serde_json::{Value, json};
    use std::process::{Command, Stdio};

    const RECEIPT_RACE_PATH: &str = "SOLSTONE_CONTROLLED_BUILD_RECEIPT_RACE_PATH";
    const RECEIPT_RACE_WRITER: &str = "SOLSTONE_CONTROLLED_BUILD_RECEIPT_RACE_WRITER";

    fn sample_source() -> SourceIdentity {
        SourceIdentity {
            product: Provenance {
                commit: "aaa".into(),
                lock_sha256: "bbb".into(),
            },
            windows_dependency: DependencySource {
                repository: "dep-fixture".into(),
                revision: "rev-1".into(),
                content_sha256: sha256_hex(b"dep-bytes"),
            },
        }
    }

    fn sample_builder() -> BuilderIdentity {
        BuilderIdentity {
            host: "builder-1".into(),
            toolchain: "clang-cl 19.40".into(),
        }
    }

    fn sample_configuration() -> BuildConfiguration {
        BuildConfiguration {
            target_triple: "windows-x86_64".into(),
            profile: "release".into(),
            flags: vec!["/O2".into()],
            network_access_denied: true,
        }
    }

    fn sample_validation() -> ValidationReference {
        ValidationReference {
            description: "check".into(),
            sha256: sha256_hex(b"val"),
        }
    }

    fn valid_draft() -> ControlledBuildReceiptDraft {
        ControlledBuildReceiptDraft {
            schema: Some(CONTROLLED_BUILD_RECEIPT_SCHEMA_V1.to_owned()),
            source: Some(sample_source()),
            inputs: Some(vec![InputIdentityEntry {
                label: "src.obj".into(),
                sha256: sha256_hex(b"obj"),
                size: 3,
            }]),
            builder: Some(sample_builder()),
            configuration: Some(sample_configuration()),
            outputs: Some(vec![]),
            supporting: Some(vec![]),
            validation: Some(sample_validation()),
        }
    }

    fn receipt_with_real_pe_census() -> ControlledBuildReceipt {
        let imported_symbols = [PeSymbolSpec::Named("GetVersion")];
        let imports = [ImportSpec {
            name: "KERNEL32.dll",
            symbols: &imported_symbols,
        }];
        let exports = [PeSymbolSpec::Named("ReceiptEntry")];
        let pe = fixture(&FixtureSpec {
            dll: true,
            imports: &imports,
            exports: &exports,
            debug: Some(DebugInfoKind::Repro),
            ..FixtureSpec::default()
        });
        let mut draft = valid_draft();
        draft.outputs = Some(census_outputs(&[("receipt.dll", pe.as_slice())]).expect("census"));
        draft.supporting = Some(vec![SupportingArtifactRef {
            label: "build.log".into(),
            sha256: sha256_hex(b"build-log"),
        }]);
        draft.validate().expect("real PE receipt")
    }

    fn receipt_document(receipt: &ControlledBuildReceipt) -> Value {
        serde_json::to_value(receipt).expect("receipt document")
    }

    fn assert_decode_rejects_unknown_property(
        mut document: Value,
        mutate: impl FnOnce(&mut Value),
    ) {
        mutate(&mut document);
        let bytes = serde_json::to_vec(&document).expect("mutated document");
        assert!(
            matches!(
                decode_controlled_build_receipt(&bytes),
                Err(ControlledBuildReceiptCodecError::Decode { .. })
            ),
            "unknown property must be rejected: {document}"
        );
    }

    fn missing_category(mut draft: ControlledBuildReceiptDraft, category: ReceiptCategory) {
        match category {
            ReceiptCategory::Schema => draft.schema = None,
            ReceiptCategory::SourceIdentity => draft.source = None,
            ReceiptCategory::InputIdentity => draft.inputs = None,
            ReceiptCategory::BuilderIdentity => draft.builder = None,
            ReceiptCategory::BuildConfiguration => draft.configuration = None,
            ReceiptCategory::OutputIdentity => draft.outputs = None,
            ReceiptCategory::SupportingArtifacts => draft.supporting = None,
            ReceiptCategory::ValidationReference => draft.validation = None,
        }
        match draft.validate() {
            Err(ControlledBuildReceiptError::Missing { category: found }) => {
                assert_eq!(found, category);
            }
            other => panic!("expected Missing {category:?}, got {other:?}"),
        }
    }

    #[test]
    fn validate_names_each_missing_category() {
        let categories = [
            ReceiptCategory::Schema,
            ReceiptCategory::SourceIdentity,
            ReceiptCategory::InputIdentity,
            ReceiptCategory::BuilderIdentity,
            ReceiptCategory::BuildConfiguration,
            ReceiptCategory::OutputIdentity,
            ReceiptCategory::SupportingArtifacts,
            ReceiptCategory::ValidationReference,
        ];
        for category in categories {
            missing_category(valid_draft(), category);
        }
    }

    #[test]
    fn present_empty_lists_validate_and_differ_from_absent() {
        let mut draft = valid_draft();
        draft.inputs = Some(vec![]);
        draft.outputs = Some(vec![]);
        draft.supporting = Some(vec![]);
        let receipt = draft.validate().expect("present-empty lists validate");
        assert!(receipt.inputs.is_empty());
        assert!(receipt.outputs.is_empty());
        assert!(receipt.supporting.is_empty());

        let mut absent = valid_draft();
        absent.inputs = None;
        match absent.validate() {
            Err(ControlledBuildReceiptError::Missing {
                category: ReceiptCategory::InputIdentity,
            }) => {}
            other => panic!("absent list must be Missing, got {other:?}"),
        }
    }

    #[test]
    fn unrecognized_schema_is_not_missing() {
        let mut draft = valid_draft();
        draft.schema = Some("solstone.nope.v1".into());
        draft.inputs = Some(vec![]);
        draft.outputs = Some(vec![]);
        draft.supporting = Some(vec![]);
        match draft.validate() {
            Err(ControlledBuildReceiptError::UnrecognizedSchema { found }) => {
                assert_eq!(found, "solstone.nope.v1");
            }
            other => panic!("expected UnrecognizedSchema, got {other:?}"),
        }
    }

    #[test]
    fn census_outputs_records_pre_signing_digest_and_pe_census() {
        let first = fixture(&FixtureSpec {
            dll: true,
            exports: &[PeSymbolSpec::Named("LibEntry")],
            ..FixtureSpec::default()
        });
        let second = fixture(&FixtureSpec {
            debug: Some(DebugInfoKind::Repro),
            ..FixtureSpec::default()
        });
        let entries = census_outputs(&[("a.dll", first.as_slice()), ("b.exe", second.as_slice())])
            .expect("census");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].label, "a.dll");
        assert_eq!(entries[0].size, first.len() as u64);
        assert_eq!(entries[0].pre_signing_sha256, sha256_hex(&first));
        assert_eq!(entries[0].census, parse_pe(&first).unwrap());
        assert_eq!(entries[1].label, "b.exe");
        assert_eq!(entries[1].size, second.len() as u64);
        assert_eq!(entries[1].pre_signing_sha256, sha256_hex(&second));
        assert_eq!(entries[1].census, parse_pe(&second).unwrap());
    }

    #[test]
    fn census_outputs_is_fail_closed_and_names_the_file() {
        let good = fixture(&FixtureSpec::default());
        let bad = fixture_pe32();
        match census_outputs(&[
            ("zero", good.as_slice()),
            ("one", bad.as_slice()),
            ("two", good.as_slice()),
        ]) {
            Err(ControlledBuildReceiptError::OutputParse { index, label, .. }) => {
                assert_eq!(index, 1);
                assert_eq!(label, "one");
            }
            Ok(_) => panic!("must not return Ok with a partial census"),
            Err(other) => panic!("expected OutputParse, got {other:?}"),
        }
        assert!(census_outputs(&[]).expect("empty").is_empty());
    }

    #[test]
    fn controlled_build_receipt_canonical_round_trip_preserves_real_pe_census() {
        let receipt = receipt_with_real_pe_census();
        let bytes = encode_controlled_build_receipt(&receipt).expect("encode");
        let reread = decode_controlled_build_receipt(&bytes).expect("decode");
        assert_eq!(reread, receipt);
        assert_eq!(
            encode_controlled_build_receipt(&reread).expect("re-encode"),
            bytes,
            "valid receipt encoding must be byte stable"
        );
        assert_eq!(
            reread.outputs[0].census, receipt.outputs[0].census,
            "PE census and pre-signing digest must survive persistence"
        );
    }

    #[test]
    fn controlled_build_receipt_decoding_distinguishes_empty_lists_from_missing_categories() {
        let mut draft = valid_draft();
        draft.inputs = Some(vec![]);
        draft.outputs = Some(vec![]);
        draft.supporting = Some(vec![]);
        let receipt = draft.validate().expect("present empty receipt");
        let bytes = encode_controlled_build_receipt(&receipt).expect("encode");
        let reread = decode_controlled_build_receipt(&bytes).expect("decode");
        assert!(reread.inputs.is_empty());
        assert!(reread.outputs.is_empty());
        assert!(reread.supporting.is_empty());

        let mut absent = receipt_document(&receipt);
        absent
            .as_object_mut()
            .expect("receipt object")
            .remove("inputs");
        match decode_controlled_build_receipt(&serde_json::to_vec(&absent).unwrap()) {
            Err(ControlledBuildReceiptCodecError::Validation {
                source:
                    ControlledBuildReceiptError::Missing {
                        category: ReceiptCategory::InputIdentity,
                    },
            }) => {}
            other => panic!("absent input list must be rejected, got {other:?}"),
        }
    }

    #[test]
    fn controlled_build_receipt_decoding_rejects_every_absent_category() {
        let receipt = receipt_with_real_pe_census();
        let categories = [
            (ReceiptCategory::Schema, "schema"),
            (ReceiptCategory::SourceIdentity, "source"),
            (ReceiptCategory::InputIdentity, "inputs"),
            (ReceiptCategory::BuilderIdentity, "builder"),
            (ReceiptCategory::BuildConfiguration, "configuration"),
            (ReceiptCategory::OutputIdentity, "outputs"),
            (ReceiptCategory::SupportingArtifacts, "supporting"),
            (ReceiptCategory::ValidationReference, "validation"),
        ];
        for (category, field) in categories {
            let mut document = receipt_document(&receipt);
            document
                .as_object_mut()
                .expect("receipt object")
                .remove(field);
            match decode_controlled_build_receipt(&serde_json::to_vec(&document).unwrap()) {
                Err(ControlledBuildReceiptCodecError::Validation {
                    source: ControlledBuildReceiptError::Missing { category: found },
                }) => assert_eq!(found, category),
                other => panic!("missing {field} must name its category, got {other:?}"),
            }
        }
    }

    #[test]
    fn controlled_build_receipt_decoding_rejects_unknown_properties_at_every_boundary() {
        let receipt = receipt_with_real_pe_census();
        assert_decode_rejects_unknown_property(receipt_document(&receipt), |document| {
            document
                .as_object_mut()
                .unwrap()
                .insert("unknown".into(), json!(true));
        });
        assert_decode_rejects_unknown_property(
            serde_json::to_value(valid_draft()).expect("draft document"),
            |document| {
                document
                    .as_object_mut()
                    .unwrap()
                    .insert("unknown".into(), json!(true));
            },
        );
        assert_decode_rejects_unknown_property(receipt_document(&receipt), |document| {
            document["source"]
                .as_object_mut()
                .unwrap()
                .insert("unknown".into(), json!(true));
        });
        assert_decode_rejects_unknown_property(receipt_document(&receipt), |document| {
            document["source"]["product"]
                .as_object_mut()
                .unwrap()
                .insert("unknown".into(), json!(true));
        });
        assert_decode_rejects_unknown_property(receipt_document(&receipt), |document| {
            document["source"]["windows_dependency"]
                .as_object_mut()
                .unwrap()
                .insert("unknown".into(), json!(true));
        });
        assert_decode_rejects_unknown_property(receipt_document(&receipt), |document| {
            document["inputs"][0]
                .as_object_mut()
                .unwrap()
                .insert("unknown".into(), json!(true));
        });
        assert_decode_rejects_unknown_property(receipt_document(&receipt), |document| {
            document["builder"]
                .as_object_mut()
                .unwrap()
                .insert("unknown".into(), json!(true));
        });
        assert_decode_rejects_unknown_property(receipt_document(&receipt), |document| {
            document["configuration"]
                .as_object_mut()
                .unwrap()
                .insert("unknown".into(), json!(true));
        });
        assert_decode_rejects_unknown_property(receipt_document(&receipt), |document| {
            document["outputs"][0]
                .as_object_mut()
                .unwrap()
                .insert("unknown".into(), json!(true));
        });
        assert_decode_rejects_unknown_property(receipt_document(&receipt), |document| {
            document["outputs"][0]["census"]
                .as_object_mut()
                .unwrap()
                .insert("unknown".into(), json!(true));
        });
        assert_decode_rejects_unknown_property(receipt_document(&receipt), |document| {
            document["outputs"][0]["census"]["imports"][0]
                .as_object_mut()
                .unwrap()
                .insert("unknown".into(), json!(true));
        });
        assert_decode_rejects_unknown_property(receipt_document(&receipt), |document| {
            document["supporting"][0]
                .as_object_mut()
                .unwrap()
                .insert("unknown".into(), json!(true));
        });
        assert_decode_rejects_unknown_property(receipt_document(&receipt), |document| {
            document["validation"]
                .as_object_mut()
                .unwrap()
                .insert("unknown".into(), json!(true));
        });

        let mut unknown_symbol = receipt_document(&receipt);
        unknown_symbol["outputs"][0]["census"]["imports"][0]["symbols"][0] =
            json!({ "UnknownSymbol": "unexpected" });
        assert!(matches!(
            decode_controlled_build_receipt(&serde_json::to_vec(&unknown_symbol).unwrap()),
            Err(ControlledBuildReceiptCodecError::Decode { .. })
        ));

        let mut unknown_debug = receipt_document(&receipt);
        unknown_debug["outputs"][0]["census"]["debug"] = json!({ "UnknownDebug": 1 });
        assert!(matches!(
            decode_controlled_build_receipt(&serde_json::to_vec(&unknown_debug).unwrap()),
            Err(ControlledBuildReceiptCodecError::Decode { .. })
        ));
    }

    #[test]
    fn controlled_build_receipt_decoding_rejects_malformed_and_wrong_schema_documents() {
        assert!(matches!(
            decode_controlled_build_receipt(b"not JSON"),
            Err(ControlledBuildReceiptCodecError::Decode { .. })
        ));
        let receipt = receipt_with_real_pe_census();
        let mut document = receipt_document(&receipt);
        document["schema"] = json!("solstone.controlled-build-receipt.v0");
        match decode_controlled_build_receipt(&serde_json::to_vec(&document).unwrap()) {
            Err(ControlledBuildReceiptCodecError::Validation {
                source: ControlledBuildReceiptError::UnrecognizedSchema { found },
            }) => assert_eq!(found, "solstone.controlled-build-receipt.v0"),
            other => panic!("wrong schema must be rejected, got {other:?}"),
        }
    }

    #[test]
    fn controlled_build_receipt_writer_and_reader_preserve_exact_canonical_document() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let path = temporary.path().join("receipt.json");
        let receipt = receipt_with_real_pe_census();
        let expected = encode_controlled_build_receipt(&receipt).expect("encode");
        let published = write_controlled_build_receipt_exclusive(&path, &receipt).expect("write");
        assert_eq!(published.path(), path);
        assert_eq!(fs::read(&path).expect("on-disk receipt"), expected);
        assert_eq!(
            read_controlled_build_receipt(&path).expect("strict reread"),
            receipt
        );
        #[cfg(unix)]
        assert!(matches!(
            published,
            ControlledBuildReceiptPublication::Durable { .. }
        ));
        #[cfg(windows)]
        assert!(matches!(
            published,
            ControlledBuildReceiptPublication::PublishedButNotDurable { .. }
        ));
    }

    #[test]
    fn controlled_build_receipt_writer_refuses_existing_final_without_changing_bytes() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let path = temporary.path().join("receipt.json");
        let existing = b"other writer owns this final";
        fs::write(&path, existing).expect("existing receipt");
        let error = write_controlled_build_receipt_exclusive(&path, &receipt_with_real_pe_census())
            .expect_err("existing final must refuse");
        match error {
            ControlledBuildReceiptPersistenceError::Publish { source, .. } => {
                assert_eq!(source.source.kind(), io::ErrorKind::AlreadyExists);
            }
            other => panic!("expected publication refusal, got {other:?}"),
        }
        assert_eq!(fs::read(&path).expect("existing bytes"), existing);
    }

    #[test]
    fn controlled_build_receipt_preserves_unconfirmed_recovery_facts_without_durable_success() {
        let path = PathBuf::from("receipt.json");
        let publication = ExclusivePublication {
            bytes_written: 1,
            final_name: FinalNameConfirmation::Unverified {
                destination: path.clone(),
                reason: io::Error::other("post-publication observation failed"),
            },
            durability: MetadataDurability::Unproven { source: None },
            cleanup: StageCleanup::Retained {
                stage: ".receipt.stage".into(),
                source: io::Error::other("cleanup failed"),
            },
        };
        let outcome =
            classify_receipt_publication(path.clone(), receipt_with_real_pe_census(), publication);
        assert!(!outcome.is_durable());
        assert!(!outcome.is_signing_eligible());
        match outcome {
            ControlledBuildReceiptPublication::PublicationUnconfirmed {
                path: actual,
                publication,
                ..
            } => {
                assert_eq!(actual, path);
                assert!(matches!(
                    publication.final_name,
                    FinalNameConfirmation::Unverified { .. }
                ));
                assert!(matches!(
                    publication.durability,
                    MetadataDurability::Unproven { source: None }
                ));
                assert!(matches!(publication.cleanup, StageCleanup::Retained { .. }));
            }
            other => panic!("unconfirmed facts must not become durable, got {other:?}"),
        }
    }

    #[cfg(windows)]
    #[test]
    fn controlled_build_receipt_windows_publication_is_not_durable_or_signing_eligible() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let path = temporary.path().join("receipt.json");
        let outcome =
            write_controlled_build_receipt_exclusive(&path, &receipt_with_real_pe_census())
                .expect("write");
        assert!(!outcome.is_durable());
        assert!(!outcome.is_signing_eligible());
        match outcome {
            ControlledBuildReceiptPublication::PublishedButNotDurable { publication, .. } => {
                assert!(matches!(
                    publication.final_name,
                    FinalNameConfirmation::Confirmed { .. }
                ));
                assert!(matches!(publication.cleanup, StageCleanup::Removed));
                assert!(matches!(
                    publication.durability,
                    MetadataDurability::Unproven { source: None }
                ));
            }
            other => panic!("Windows result must be published-but-not-durable, got {other:?}"),
        }
    }

    #[test]
    fn controlled_build_receipt_two_process_same_path_child() {
        let Some(path) = std::env::var_os(RECEIPT_RACE_PATH) else {
            return;
        };
        let writer = std::env::var(RECEIPT_RACE_WRITER).expect("race writer identity");
        let mut receipt = receipt_with_real_pe_census();
        receipt.builder.host = format!("receipt-race-{writer}");
        match write_controlled_build_receipt_exclusive(PathBuf::from(path), &receipt) {
            Ok(ControlledBuildReceiptPublication::Durable { .. }) => {
                #[cfg(windows)]
                panic!("Windows must not report durable receipt publication");
                #[cfg(not(windows))]
                println!("controlled-build-receipt-race={writer}:winner-durable");
            }
            Ok(ControlledBuildReceiptPublication::PublishedButNotDurable { .. }) => {
                #[cfg(not(windows))]
                panic!("non-Windows test filesystem must fully confirm publication");
                #[cfg(windows)]
                println!("controlled-build-receipt-race={writer}:winner-not-durable");
            }
            Ok(ControlledBuildReceiptPublication::PublicationUnconfirmed {
                publication, ..
            }) => {
                panic!("race winner must have a confirmed final name and cleanup: {publication:?}");
            }
            Err(ControlledBuildReceiptPersistenceError::Publish { source, .. })
                if source.source.kind() == io::ErrorKind::AlreadyExists =>
            {
                println!("controlled-build-receipt-race={writer}:loser-existing");
            }
            Err(other) => panic!("race writer failed unexpectedly: {other:?}"),
        }
    }

    #[test]
    fn controlled_build_receipt_two_process_same_path_has_one_unmodified_winner() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let path = temporary.path().join("receipt.json");
        let executable = std::env::current_exe().expect("current test executable");
        let child_name =
            "controlled_build::tests::controlled_build_receipt_two_process_same_path_child";
        let first = Command::new(&executable)
            .arg("--exact")
            .arg(child_name)
            .arg("--nocapture")
            .env(RECEIPT_RACE_PATH, &path)
            .env(RECEIPT_RACE_WRITER, "first")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("first race writer");
        let second = Command::new(&executable)
            .arg("--exact")
            .arg(child_name)
            .arg("--nocapture")
            .env(RECEIPT_RACE_PATH, &path)
            .env(RECEIPT_RACE_WRITER, "second")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("second race writer");
        let first_output = first.wait_with_output().expect("first result");
        let second_output = second.wait_with_output().expect("second result");
        assert!(
            first_output.status.success(),
            "first race writer failed: {}",
            String::from_utf8_lossy(&first_output.stderr)
        );
        assert!(
            second_output.status.success(),
            "second race writer failed: {}",
            String::from_utf8_lossy(&second_output.stderr)
        );
        let output = format!(
            "{}{}",
            String::from_utf8_lossy(&first_output.stdout),
            String::from_utf8_lossy(&second_output.stdout)
        );
        assert_eq!(
            output.matches(":loser-existing").count(),
            1,
            "exactly one writer must lose without modifying the winner: {output}"
        );
        assert_eq!(
            output.matches(":winner-").count(),
            1,
            "exactly one writer must publish: {output}"
        );

        let winner = read_controlled_build_receipt(&path).expect("strict winner reread");
        assert!(
            ["receipt-race-first", "receipt-race-second"].contains(&winner.builder.host.as_str()),
            "final receipt must equal one distinguishable writer's source value"
        );
        let expected = encode_controlled_build_receipt(&winner).expect("winner encoding");
        assert_eq!(fs::read(&path).expect("final receipt bytes"), expected);
        let names = fs::read_dir(temporary.path())
            .expect("race directory")
            .map(|entry| entry.expect("race entry").file_name())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["receipt.json"]);
    }
}
