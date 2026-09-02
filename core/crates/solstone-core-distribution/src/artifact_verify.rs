// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Handle-safe revalidation of the pre-signing outputs named by a controlled
//! build receipt.
//!
//! This verifies only the output bytes and PE census recorded in an already
//! structurally valid receipt. It neither creates an artifact, nor admits a
//! signature, package, loader, or runtime.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use solstone_core_journal_io::{JournalRoot, JournalRootError, check_portable_component};

use crate::controlled_build::{
    ControlledBuildReceipt, ControlledBuildReceiptError, ControlledBuildReceiptPersistenceError,
    OutputIdentityEntry, read_controlled_build_receipt,
};
use crate::digest::sha256_hex;
use crate::pe::{self, PeError};

/// Bounded resources for one all-or-nothing artifact-verification operation.
///
/// A controlled build supplies these limits from its reviewed build slot. The
/// verifier never infers a safe bound from a path spelling or from an artifact
/// that it has not opened through the retained root.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ControlledBuildArtifactVerificationLimits {
    pub maximum_entries: usize,
    pub maximum_depth: usize,
    pub maximum_total_bytes: usize,
}

impl ControlledBuildArtifactVerificationLimits {
    #[must_use]
    pub const fn new(
        maximum_entries: usize,
        maximum_depth: usize,
        maximum_total_bytes: usize,
    ) -> Self {
        Self {
            maximum_entries,
            maximum_depth,
            maximum_total_bytes,
        }
    }
}

/// Verified output identities from a receipt whose bytes were re-read through
/// platform-native retained handles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedControlledBuildArtifacts {
    receipt: ControlledBuildReceipt,
}

impl VerifiedControlledBuildArtifacts {
    /// The receipt whose pre-signing output bytes and PE census were verified.
    #[must_use]
    pub fn receipt(&self) -> &ControlledBuildReceipt {
        &self.receipt
    }
}

/// A strict output verification refusal.
#[derive(Debug)]
pub enum ControlledBuildArtifactVerificationError {
    ReceiptValidation {
        source: ControlledBuildReceiptError,
    },
    ReceiptRead {
        path: PathBuf,
        source: Box<ControlledBuildReceiptPersistenceError>,
    },
    Root {
        path: PathBuf,
        source: JournalRootError,
    },
    InvalidOutputLabel {
        label: String,
        reason: String,
    },
    NoOutputs,
    OutputCountExceedsLimit {
        count: usize,
        limit: usize,
    },
    DuplicateOutputLabel {
        label: String,
    },
    DeclaredTotalExceedsLimit {
        declared: u64,
        limit: usize,
    },
    OutputSizeExceedsAddressSpace {
        label: String,
        size: u64,
    },
    OutputMissing {
        label: String,
    },
    OutputRead {
        label: String,
        source: OutputReadError,
    },
    OutputSizeMismatch {
        label: String,
        expected: u64,
        actual: u64,
    },
    OutputDigestMismatch {
        label: String,
        expected: String,
        actual: String,
    },
    OutputCensusParse {
        label: String,
        source: PeError,
    },
    OutputCensusMismatch {
        label: String,
    },
}

impl std::fmt::Display for ControlledBuildArtifactVerificationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ReceiptValidation { source } => {
                write!(formatter, "invalid controlled-build receipt: {source}")
            }
            Self::ReceiptRead { path, source } => write!(
                formatter,
                "could not read controlled-build receipt {}: {source}",
                path.display()
            ),
            Self::Root { path, source } => write!(
                formatter,
                "could not admit controlled-build output root {}: {source}",
                path.display()
            ),
            Self::InvalidOutputLabel { label, reason } => {
                write!(
                    formatter,
                    "invalid controlled-build output label {label:?}: {reason}"
                )
            }
            Self::NoOutputs => formatter
                .write_str("controlled-build artifact verification requires at least one output"),
            Self::OutputCountExceedsLimit { count, limit } => write!(
                formatter,
                "controlled-build receipt declares {count} outputs above the {limit}-entry verification limit"
            ),
            Self::DuplicateOutputLabel { label } => {
                write!(
                    formatter,
                    "duplicate controlled-build output label {label:?}"
                )
            }
            Self::DeclaredTotalExceedsLimit { declared, limit } => write!(
                formatter,
                "controlled-build receipt declares {declared} output bytes above the {limit}-byte verification limit"
            ),
            Self::OutputSizeExceedsAddressSpace { label, size } => write!(
                formatter,
                "controlled-build output {label:?} declares {size} bytes beyond this process address space"
            ),
            Self::OutputMissing { label } => {
                write!(formatter, "controlled-build output {label:?} is absent")
            }
            Self::OutputRead { label, source } => {
                write!(
                    formatter,
                    "could not safely read controlled-build output {label:?}: {source}"
                )
            }
            Self::OutputSizeMismatch {
                label,
                expected,
                actual,
            } => write!(
                formatter,
                "controlled-build output {label:?} size mismatch: expected {expected}, actual {actual}"
            ),
            Self::OutputDigestMismatch {
                label,
                expected,
                actual,
            } => write!(
                formatter,
                "controlled-build output {label:?} pre-signing SHA-256 mismatch: expected {expected}, actual {actual}"
            ),
            Self::OutputCensusParse { label, source } => {
                write!(
                    formatter,
                    "could not parse controlled-build PE output {label:?}: {source}"
                )
            }
            Self::OutputCensusMismatch { label } => {
                write!(
                    formatter,
                    "controlled-build output {label:?} PE census differs from its receipt"
                )
            }
        }
    }
}

impl std::error::Error for ControlledBuildArtifactVerificationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ReceiptValidation { source } => Some(source),
            Self::ReceiptRead { source, .. } => Some(source),
            Self::Root { source, .. } => Some(source),
            Self::OutputRead { source, .. } => Some(source),
            Self::OutputCensusParse { source, .. } => Some(source),
            Self::InvalidOutputLabel { .. }
            | Self::NoOutputs
            | Self::OutputCountExceedsLimit { .. }
            | Self::DuplicateOutputLabel { .. }
            | Self::DeclaredTotalExceedsLimit { .. }
            | Self::OutputSizeExceedsAddressSpace { .. }
            | Self::OutputMissing { .. }
            | Self::OutputSizeMismatch { .. }
            | Self::OutputDigestMismatch { .. }
            | Self::OutputCensusMismatch { .. } => None,
        }
    }
}

#[derive(Debug)]
pub enum OutputReadError {
    #[cfg(unix)]
    Unix(solstone_core_journal_io::FlatDirectoryError),
    #[cfg(windows)]
    Windows(solstone_core_journal_io::WindowsInventoryError),
}

impl std::fmt::Display for OutputReadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            #[cfg(unix)]
            Self::Unix(source) => source.fmt(formatter),
            #[cfg(windows)]
            Self::Windows(source) => source.fmt(formatter),
        }
    }
}

impl std::error::Error for OutputReadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            #[cfg(unix)]
            Self::Unix(source) => Some(source),
            #[cfg(windows)]
            Self::Windows(source) => Some(source),
        }
    }
}

/// Read a persisted receipt structurally, then rehash and re-census every
/// named pre-signing output below `output_root`.
pub fn verify_persisted_controlled_build_artifacts(
    receipt_path: impl AsRef<Path>,
    output_root: impl AsRef<Path>,
    limits: ControlledBuildArtifactVerificationLimits,
) -> Result<VerifiedControlledBuildArtifacts, ControlledBuildArtifactVerificationError> {
    let receipt_path = receipt_path.as_ref().to_path_buf();
    let receipt = read_controlled_build_receipt(&receipt_path).map_err(|source| {
        ControlledBuildArtifactVerificationError::ReceiptRead {
            path: receipt_path,
            source: Box::new(source),
        }
    })?;
    verify_controlled_build_artifacts(output_root, &receipt, limits)
}

/// Rehash and re-census every pre-signing output named by an in-memory receipt.
///
/// The retained root and descriptor-relative opens, rather than the input path
/// spelling, establish containment. A successful result is output evidence;
/// it is deliberately not a signing admission.
pub fn verify_controlled_build_artifacts(
    output_root: impl AsRef<Path>,
    receipt: &ControlledBuildReceipt,
    limits: ControlledBuildArtifactVerificationLimits,
) -> Result<VerifiedControlledBuildArtifacts, ControlledBuildArtifactVerificationError> {
    receipt
        .validate()
        .map_err(|source| ControlledBuildArtifactVerificationError::ReceiptValidation { source })?;
    let labels = prepare_output_labels(&receipt.outputs, limits)?;
    let root_path = output_root.as_ref().to_path_buf();
    let root = JournalRoot::open(&root_path).map_err(|source| {
        ControlledBuildArtifactVerificationError::Root {
            path: root_path,
            source,
        }
    })?;

    #[cfg(unix)]
    verify_outputs_unix(&root, &receipt.outputs, &labels)?;
    #[cfg(windows)]
    verify_outputs_windows(&root, &receipt.outputs, &labels, limits)?;

    Ok(VerifiedControlledBuildArtifacts {
        receipt: receipt.clone(),
    })
}

fn prepare_output_labels(
    outputs: &[OutputIdentityEntry],
    limits: ControlledBuildArtifactVerificationLimits,
) -> Result<Vec<Vec<String>>, ControlledBuildArtifactVerificationError> {
    if outputs.is_empty() {
        return Err(ControlledBuildArtifactVerificationError::NoOutputs);
    }
    if outputs.len() > limits.maximum_entries {
        return Err(
            ControlledBuildArtifactVerificationError::OutputCountExceedsLimit {
                count: outputs.len(),
                limit: limits.maximum_entries,
            },
        );
    }
    let mut seen = BTreeSet::new();
    let mut total = 0_u64;
    let mut labels = Vec::with_capacity(outputs.len());
    for output in outputs {
        if !seen.insert(output.label.clone()) {
            return Err(
                ControlledBuildArtifactVerificationError::DuplicateOutputLabel {
                    label: output.label.clone(),
                },
            );
        }
        total = total.checked_add(output.size).ok_or(
            ControlledBuildArtifactVerificationError::DeclaredTotalExceedsLimit {
                declared: u64::MAX,
                limit: limits.maximum_total_bytes,
            },
        )?;
        if total > limits.maximum_total_bytes as u64 {
            return Err(
                ControlledBuildArtifactVerificationError::DeclaredTotalExceedsLimit {
                    declared: total,
                    limit: limits.maximum_total_bytes,
                },
            );
        }
        let components = admit_output_label(&output.label)?;
        if components.len() > limits.maximum_depth.saturating_add(1) {
            return Err(
                ControlledBuildArtifactVerificationError::InvalidOutputLabel {
                    label: output.label.clone(),
                    reason: format!(
                        "path depth {} exceeds the {}-directory verification limit",
                        components.len().saturating_sub(1),
                        limits.maximum_depth
                    ),
                },
            );
        }
        if usize::try_from(output.size).is_err() {
            return Err(
                ControlledBuildArtifactVerificationError::OutputSizeExceedsAddressSpace {
                    label: output.label.clone(),
                    size: output.size,
                },
            );
        }
        labels.push(components);
    }
    Ok(labels)
}

fn admit_output_label(
    label: &str,
) -> Result<Vec<String>, ControlledBuildArtifactVerificationError> {
    let components = label
        .split('/')
        .map(|component| {
            check_portable_component(component).map_err(|reason| {
                ControlledBuildArtifactVerificationError::InvalidOutputLabel {
                    label: label.to_owned(),
                    reason: reason.to_string(),
                }
            })?;
            Ok(component.to_owned())
        })
        .collect::<Result<Vec<_>, ControlledBuildArtifactVerificationError>>()?;
    if components.is_empty() {
        return Err(
            ControlledBuildArtifactVerificationError::InvalidOutputLabel {
                label: label.to_owned(),
                reason: "path must name one portable component".to_owned(),
            },
        );
    }
    Ok(components)
}

fn verify_output_bytes(
    expected: &OutputIdentityEntry,
    bytes: Vec<u8>,
) -> Result<(), ControlledBuildArtifactVerificationError> {
    let actual_size = bytes.len() as u64;
    if actual_size != expected.size {
        return Err(
            ControlledBuildArtifactVerificationError::OutputSizeMismatch {
                label: expected.label.clone(),
                expected: expected.size,
                actual: actual_size,
            },
        );
    }
    let actual_digest = sha256_hex(&bytes);
    if actual_digest != expected.pre_signing_sha256 {
        return Err(
            ControlledBuildArtifactVerificationError::OutputDigestMismatch {
                label: expected.label.clone(),
                expected: expected.pre_signing_sha256.clone(),
                actual: actual_digest,
            },
        );
    }
    let actual_census = pe::parse_pe(&bytes).map_err(|source| {
        ControlledBuildArtifactVerificationError::OutputCensusParse {
            label: expected.label.clone(),
            source,
        }
    })?;
    if actual_census != expected.census {
        return Err(
            ControlledBuildArtifactVerificationError::OutputCensusMismatch {
                label: expected.label.clone(),
            },
        );
    }
    Ok(())
}

#[cfg(unix)]
fn verify_outputs_unix(
    root: &JournalRoot,
    outputs: &[OutputIdentityEntry],
    labels: &[Vec<String>],
) -> Result<(), ControlledBuildArtifactVerificationError> {
    use std::ffi::OsStr;

    use solstone_core_journal_io::{
        FlatDirectory, read_observed_file_bounded, read_observed_root_file_bounded,
    };

    for (expected, components) in outputs.iter().zip(labels) {
        let (leaf, parents) = components
            .split_last()
            .expect("admitted output labels are nonempty");
        let maximum = usize::try_from(expected.size).map_err(|_| {
            ControlledBuildArtifactVerificationError::OutputSizeExceedsAddressSpace {
                label: expected.label.clone(),
                size: expected.size,
            }
        })?;
        let bytes = if parents.is_empty() {
            read_observed_root_file_bounded(root, OsStr::new(leaf), maximum).map_err(|source| {
                ControlledBuildArtifactVerificationError::OutputRead {
                    label: expected.label.clone(),
                    source: OutputReadError::Unix(source),
                }
            })?
        } else {
            let parent = parents.iter().collect::<PathBuf>();
            let directory = FlatDirectory::open(root, &parent).map_err(|source| {
                ControlledBuildArtifactVerificationError::OutputRead {
                    label: expected.label.clone(),
                    source: OutputReadError::Unix(source),
                }
            })?;
            read_observed_file_bounded(&directory, OsStr::new(leaf), maximum).map_err(|source| {
                ControlledBuildArtifactVerificationError::OutputRead {
                    label: expected.label.clone(),
                    source: OutputReadError::Unix(source),
                }
            })?
        }
        .ok_or_else(|| ControlledBuildArtifactVerificationError::OutputMissing {
            label: expected.label.clone(),
        })?
        .bytes;
        verify_output_bytes(expected, bytes)?;
    }
    Ok(())
}

#[cfg(windows)]
fn verify_outputs_windows(
    root: &JournalRoot,
    outputs: &[OutputIdentityEntry],
    labels: &[Vec<String>],
    limits: ControlledBuildArtifactVerificationLimits,
) -> Result<(), ControlledBuildArtifactVerificationError> {
    use solstone_core_journal_io::{
        InventoryBudget, WindowsCheckedReadSession, enumerate_windows_inventory,
    };

    let budget = InventoryBudget::new(
        limits.maximum_entries,
        limits.maximum_depth,
        255,
        32 * 1024,
        limits.maximum_total_bytes,
    );
    let inventory = enumerate_windows_inventory(root, budget).map_err(|source| {
        ControlledBuildArtifactVerificationError::OutputRead {
            label: "<output root inventory>".to_owned(),
            source: OutputReadError::Windows(source),
        }
    })?;
    let mut session = WindowsCheckedReadSession::new(budget);
    for (expected, components) in outputs.iter().zip(labels) {
        let entry = inventory
            .entries()
            .iter()
            .find(|entry| output_components_match(entry.relative_path(), components))
            .ok_or_else(|| ControlledBuildArtifactVerificationError::OutputMissing {
                label: expected.label.clone(),
            })?;
        let bytes = session.read(root, entry).map_err(|source| {
            ControlledBuildArtifactVerificationError::OutputRead {
                label: expected.label.clone(),
                source: OutputReadError::Windows(source),
            }
        })?;
        verify_output_bytes(expected, bytes)?;
    }
    Ok(())
}

#[cfg(windows)]
fn output_components_match(path: &Path, components: &[String]) -> bool {
    let actual = path
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(component) => component.to_str(),
            _ => None,
        })
        .collect::<Vec<_>>();
    actual.len() == components.len()
        && actual
            .iter()
            .zip(components)
            .all(|(actual, expected)| *actual == expected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controlled_build::{
        BuildConfiguration, BuilderIdentity, ControlledBuildReceiptDraft, DependencySource,
        InputIdentityEntry, SourceIdentity, SupportingArtifactRef, ValidationReference,
        census_outputs, write_controlled_build_receipt_exclusive,
    };
    use crate::pe::{DebugInfoKind, FixtureSpec, PeSymbolSpec, fixture};
    use crate::provenance::Provenance;

    fn limits() -> ControlledBuildArtifactVerificationLimits {
        ControlledBuildArtifactVerificationLimits::new(16, 4, 1024 * 1024)
    }

    fn receipt_for(label: &str, bytes: &[u8]) -> ControlledBuildReceipt {
        ControlledBuildReceiptDraft {
            schema: Some(crate::controlled_build::CONTROLLED_BUILD_RECEIPT_SCHEMA_V1.into()),
            source: Some(SourceIdentity {
                product: Provenance {
                    commit: "product".into(),
                    lock_sha256: "lock".into(),
                },
                windows_dependency: DependencySource {
                    repository: "dependency".into(),
                    revision: "revision".into(),
                    content_sha256: "source".into(),
                },
            }),
            inputs: Some(vec![InputIdentityEntry {
                label: "input".into(),
                sha256: "input".into(),
                size: 1,
            }]),
            builder: Some(BuilderIdentity {
                host: "builder".into(),
                toolchain: "msvc".into(),
            }),
            configuration: Some(BuildConfiguration {
                target_triple: "x86_64-pc-windows-msvc".into(),
                profile: "Release".into(),
                flags: vec![],
                network_access_denied: true,
            }),
            outputs: Some(census_outputs(&[(label, bytes)]).expect("PE census")),
            supporting: Some(vec![SupportingArtifactRef {
                label: "notice".into(),
                sha256: "notice".into(),
            }]),
            validation: Some(ValidationReference {
                description: "validation".into(),
                sha256: "validation".into(),
            }),
        }
        .validate()
        .expect("receipt")
    }

    fn fixture_pe() -> Vec<u8> {
        let exports = [PeSymbolSpec::Named("ArtifactEntry")];
        fixture(&FixtureSpec {
            dll: true,
            exports: &exports,
            debug: Some(DebugInfoKind::Repro),
            ..FixtureSpec::default()
        })
    }

    #[test]
    fn controlled_build_artifact_verification_rehashes_direct_and_nested_outputs() {
        let temporary = tempfile::tempdir().expect("temporary root");
        let bytes = fixture_pe();
        std::fs::create_dir(temporary.path().join("bin")).expect("bin");
        std::fs::write(temporary.path().join("bin/artifact.dll"), &bytes).expect("artifact");
        let receipt = receipt_for("bin/artifact.dll", &bytes);
        let verified = verify_controlled_build_artifacts(temporary.path(), &receipt, limits())
            .expect("verify nested output");
        assert_eq!(verified.receipt(), &receipt);

        std::fs::write(temporary.path().join("direct.dll"), &bytes).expect("direct artifact");
        let direct = receipt_for("direct.dll", &bytes);
        assert!(verify_controlled_build_artifacts(temporary.path(), &direct, limits()).is_ok());
    }

    #[test]
    fn controlled_build_artifact_verification_refuses_mutated_or_missing_outputs() {
        let temporary = tempfile::tempdir().expect("temporary root");
        let bytes = fixture_pe();
        std::fs::write(temporary.path().join("artifact.dll"), &bytes).expect("artifact");
        let receipt = receipt_for("artifact.dll", &bytes);
        std::fs::write(temporary.path().join("artifact.dll"), b"not a PE").expect("mutate");
        assert!(matches!(
            verify_controlled_build_artifacts(temporary.path(), &receipt, limits()),
            Err(ControlledBuildArtifactVerificationError::OutputSizeMismatch { .. })
                | Err(ControlledBuildArtifactVerificationError::OutputDigestMismatch { .. })
        ));
        std::fs::remove_file(temporary.path().join("artifact.dll")).expect("remove");
        assert!(matches!(
            verify_controlled_build_artifacts(temporary.path(), &receipt, limits()),
            Err(ControlledBuildArtifactVerificationError::OutputMissing { .. })
        ));
    }

    #[test]
    fn controlled_build_artifact_verification_rejects_a_mismatched_pe_census() {
        let temporary = tempfile::tempdir().expect("temporary root");
        let bytes = fixture_pe();
        std::fs::write(temporary.path().join("artifact.dll"), &bytes).expect("artifact");
        let mut receipt = receipt_for("artifact.dll", &bytes);
        receipt.outputs[0].census.machine = 0xAA64;
        assert!(matches!(
            verify_controlled_build_artifacts(temporary.path(), &receipt, limits()),
            Err(ControlledBuildArtifactVerificationError::OutputCensusMismatch { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn controlled_build_artifact_verification_refuses_a_symlinked_parent() {
        use std::os::unix::fs::symlink;

        let temporary = tempfile::tempdir().expect("temporary root");
        let outside = tempfile::tempdir().expect("outside root");
        let bytes = fixture_pe();
        std::fs::write(outside.path().join("artifact.dll"), &bytes).expect("outside artifact");
        symlink(outside.path(), temporary.path().join("bin")).expect("symlink parent");
        let receipt = receipt_for("bin/artifact.dll", &bytes);
        assert!(matches!(
            verify_controlled_build_artifacts(temporary.path(), &receipt, limits()),
            Err(ControlledBuildArtifactVerificationError::OutputRead {
                source: OutputReadError::Unix(_),
                ..
            })
        ));
    }

    #[test]
    fn controlled_build_artifact_verification_revalidates_a_persisted_receipt() {
        let temporary = tempfile::tempdir().expect("temporary root");
        let bytes = fixture_pe();
        std::fs::write(temporary.path().join("artifact.dll"), &bytes).expect("artifact");
        let receipt = receipt_for("artifact.dll", &bytes);
        let receipt_path = temporary.path().join("receipt.json");
        write_controlled_build_receipt_exclusive(&receipt_path, &receipt).expect("persist receipt");
        let verified =
            verify_persisted_controlled_build_artifacts(&receipt_path, temporary.path(), limits())
                .expect("verify persisted receipt");
        assert_eq!(verified.receipt(), &receipt);
    }

    #[test]
    fn controlled_build_artifact_verification_refuses_duplicate_or_unsafe_labels() {
        let bytes = fixture_pe();
        let mut duplicate = receipt_for("artifact.dll", &bytes);
        duplicate.outputs.push(duplicate.outputs[0].clone());
        assert!(matches!(
            verify_controlled_build_artifacts(
                tempfile::tempdir().unwrap().path(),
                &duplicate,
                limits()
            ),
            Err(ControlledBuildArtifactVerificationError::DuplicateOutputLabel { .. })
        ));
        let unsafe_label = receipt_for("../artifact.dll", &bytes);
        assert!(matches!(
            verify_controlled_build_artifacts(
                tempfile::tempdir().unwrap().path(),
                &unsafe_label,
                limits()
            ),
            Err(ControlledBuildArtifactVerificationError::InvalidOutputLabel { .. })
        ));
        let mut empty = receipt_for("artifact.dll", &bytes);
        empty.outputs.clear();
        assert!(matches!(
            verify_controlled_build_artifacts(
                tempfile::tempdir().unwrap().path(),
                &empty,
                limits()
            ),
            Err(ControlledBuildArtifactVerificationError::NoOutputs)
        ));
    }
}
