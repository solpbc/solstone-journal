// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! In-memory controlled-build receipt: identity categories plus a PE census
//! of pre-signing output bytes.

use crate::digest::sha256_hex;
use crate::pe::{self, PeError, PeInfo};
use crate::provenance::Provenance;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceIdentity {
    pub product: Provenance,
    pub windows_dependency: DependencySource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencySource {
    pub repository: String,
    pub revision: String,
    pub content_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputIdentityEntry {
    pub label: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuilderIdentity {
    pub host: String,
    pub toolchain: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildConfiguration {
    pub target_triple: String,
    pub profile: String,
    pub flags: Vec<String>,
    pub network_access_denied: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputIdentityEntry {
    /// SHA-256 of the built PE file's bytes BEFORE any Authenticode or catalog signing.
    /// Never a post-signing digest — signing is out of scope for this receipt.
    pub pre_signing_sha256: String,
    pub label: String,
    pub size: u64,
    pub census: PeInfo,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupportingArtifactRef {
    pub label: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationReference {
    pub description: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
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

#[derive(Debug, Clone, PartialEq, Eq)]
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
    use crate::pe::{DebugInfoKind, FixtureSpec, PeSymbolSpec, fixture, fixture_pe32, parse_pe};

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
}
