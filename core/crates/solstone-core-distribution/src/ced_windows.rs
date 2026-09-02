// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Windows CED (ced.cpp + ggml) controlled-build identity: pins, validators,
//! and receipt-draft assembly. No build is performed here.

use std::collections::BTreeSet;
use std::fmt;

use crate::controlled_build::{
    BuildConfiguration, BuilderIdentity, ControlledBuildReceiptDraft, DependencySource,
    InputIdentityEntry, OutputIdentityEntry, SourceIdentity, SupportingArtifactRef,
};
use crate::digest::sha256_hex;
use crate::pe::{PeInfo, PeSymbol};
use crate::provenance::{self, ProvenanceError};

pub const CED_CPP_COMMIT: &str = "c04ac14b7992d00584d9e812c9bb6268598a6ce7";
pub const GGML_COMMIT: &str = "e705c5fed490514458bdd2eaddc43bd098fcce9b";
pub const CED_CPP_REPOSITORY: &str = "https://github.com/localai-org/ced.cpp.git";
pub const CED_GGML_REPOSITORY: &str = "https://github.com/ggml-org/ggml";

/// Strict sidecar schema retained beside a controlled-build receipt. Receipt V1
/// names one Windows dependency source, so this document binds its CED source
/// identity to the nested ggml source and to the exact CMake evidence that V1
/// otherwise cannot represent structurally.
pub const CED_WINDOWS_BUILD_EVIDENCE_SCHEMA_V1: &str = "solstone.ced-windows-build-evidence.v1";
pub const CED_WINDOWS_BUILD_EVIDENCE_LABEL: &str =
    "provenance/windows-x86_64/ced-build-evidence.json";
pub const CED_WINDOWS_SOURCE_ARCHIVE_LABEL: &str = "sources/ced.cpp-with-ggml.tar.gz";
pub const CED_WINDOWS_CMAKE_ARCHIVE_LABEL: &str = "tools/cmake-windows-x86_64.zip";

pub const CED_WINDOWS_TARGET_TRIPLE: &str = "x86_64-pc-windows-msvc";
pub const CED_WINDOWS_BUILD_PROFILE: &str = "Release";
// CPU-only/shared-library CMake inputs verified against `ced.cpp`
// `c04ac14b`: its top-level target is controlled by `CED_SHARED`, forwards
// the four CED_GGML backend toggles, and otherwise defaults to native CPU
// specialization and a CLI we do not ship.
pub const CED_WINDOWS_BUILD_FLAGS: &[&str] = &[
    "-DCED_SHARED=ON",
    "-DCED_BUILD_CLI=OFF",
    "-DCED_BUILD_TESTS=OFF",
    "-DCED_GGML_CUDA=OFF",
    "-DCED_GGML_METAL=OFF",
    "-DCED_GGML_VULKAN=OFF",
    "-DCED_GGML_HIP=OFF",
    "-DBUILD_SHARED_LIBS=OFF",
    "-DGGML_NATIVE=OFF",
    "-DGGML_LLAMAFILE=OFF",
    "-DGGML_OPENCL=OFF",
    "-DGGML_BACKEND_DL=OFF",
    "-DGGML_OPENMP=ON",
];

pub const CED_WINDOWS_EXPORTS: &[&str] = &[
    "ced_capi_abi_version",
    "ced_capi_load",
    "ced_capi_free",
    "ced_capi_last_error",
    "ced_capi_classify_pcm_json",
    "ced_capi_free_string",
];

/// Exact MSVC module-definition document for the approved Rust-facing C API.
///
/// The pinned upstream target enables CMake's `WINDOWS_EXPORT_ALL_SYMBOLS`,
/// while its header defines additional inspection and path-classification
/// functions. A controlled source overlay must disable that broad export and
/// link this document so the produced DLL has precisely the reviewed ABI.
pub const CED_WINDOWS_EXPORT_DEFINITION: &str = "LIBRARY ced\nEXPORTS\n    ced_capi_abi_version\n    ced_capi_load\n    ced_capi_free\n    ced_capi_last_error\n    ced_capi_classify_pcm_json\n    ced_capi_free_string\n";

/// Catalog identity for the GGUF model. `solstone-core-assets::ARTIFACTS`'s
/// `ced-model` row is the sole authority for that unit's sha256 and it is
/// deliberately not duplicated here.
pub const CED_MODEL_UNIT: &str = "ced-model";
pub const CED_MODEL_VERSION: &str = "b5e9a4aad6438763c8da16079d77563fbed35c65";
pub const CED_MODEL_FILENAME: &str = "ced-tiny-q8_0.gguf";
pub const CED_MODEL_SIZE_BYTES: u64 = 6_211_616;

pub const CED_DLL_OUTPUT_LABEL: &str = "bin/ced.dll";

const FORBIDDEN_IMPORT_NEEDLES: &[&str] = &[
    "cuda", "nvcuda", "cudart", "cublas", "cudnn", "nvrtc", "vulkan", "opencl",
];

/// Nested source and generated-input facts for one controlled CED build.
///
/// The record is deliberately separate from the V1 receipt rather than
/// extending a general receipt with a CED-specific field. Its canonical bytes
/// are bound as a `SupportingArtifactRef` by [`assemble_receipt_draft`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CedWindowsBuildEvidence {
    pub schema: String,
    pub source_archive: InputIdentityEntry,
    pub cmake_archive: InputIdentityEntry,
    pub ggml: DependencySource,
    pub export_definition_sha256: String,
    pub cmake_cache_sha256: String,
}

#[derive(Debug)]
pub enum CedWindowsBuildEvidenceError {
    Schema {
        found: String,
    },
    Source {
        source: ProvenanceError,
    },
    Unexpected {
        field: &'static str,
        expected: String,
        found: String,
    },
    Missing {
        field: &'static str,
    },
    InvalidSha256 {
        field: &'static str,
        found: String,
    },
    OutputCount {
        found: usize,
    },
    OutputCensus {
        source: CedWindowsError,
    },
}

impl fmt::Display for CedWindowsBuildEvidenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Schema { found } => write!(
                formatter,
                "unexpected:\n  build evidence schema {found}\n  expected: {CED_WINDOWS_BUILD_EVIDENCE_SCHEMA_V1}"
            ),
            Self::Source { source } => write!(formatter, "source identity: {source}"),
            Self::Unexpected {
                field,
                expected,
                found,
            } => write!(
                formatter,
                "unexpected:\n  {field}\n  expected: {expected}\n  found: {found}"
            ),
            Self::Missing { field } => write!(formatter, "missing required:\n  {field}"),
            Self::InvalidSha256 { field, found } => write!(
                formatter,
                "unexpected:\n  {field}\n  expected: 64 hex characters\n  found: {found}"
            ),
            Self::OutputCount { found } => write!(
                formatter,
                "unexpected:\n  CED output count\n  expected: 1\n  found: {found}"
            ),
            Self::OutputCensus { source } => write!(formatter, "CED output census: {source}"),
        }
    }
}

impl std::error::Error for CedWindowsBuildEvidenceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Source { source } => Some(source),
            Self::OutputCensus { source } => Some(source),
            Self::Schema { .. }
            | Self::Unexpected { .. }
            | Self::Missing { .. }
            | Self::InvalidSha256 { .. }
            | Self::OutputCount { .. } => None,
        }
    }
}

#[derive(Debug)]
pub enum CedWindowsBuildEvidenceCodecError {
    Encode {
        source: serde_json::Error,
    },
    Decode {
        source: serde_json::Error,
    },
    Validation {
        source: CedWindowsBuildEvidenceError,
    },
}

impl fmt::Display for CedWindowsBuildEvidenceCodecError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Encode { source } => {
                write!(formatter, "could not encode CED build evidence: {source}")
            }
            Self::Decode { source } => {
                write!(formatter, "could not decode CED build evidence: {source}")
            }
            Self::Validation { source } => {
                write!(formatter, "invalid CED build evidence: {source}")
            }
        }
    }
}

impl std::error::Error for CedWindowsBuildEvidenceCodecError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Encode { source } | Self::Decode { source } => Some(source),
            Self::Validation { source } => Some(source),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CedWindowsError {
    ExportAllowlist {
        missing: Vec<String>,
        unexpected: Vec<String>,
    },
    GpuImport {
        libraries: Vec<String>,
    },
}

impl fmt::Display for CedWindowsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ExportAllowlist {
                missing,
                unexpected,
            } => write!(
                formatter,
                "CED Windows export set is not the reviewed C API\n  missing: {}\n  unexpected: {}",
                format_diff_side(missing),
                format_diff_side(unexpected)
            ),
            Self::GpuImport { libraries } => write!(
                formatter,
                "CED Windows import closure is not CPU-only\n  missing: <none>\n  unexpected: {}",
                format_diff_side(libraries)
            ),
        }
    }
}

impl std::error::Error for CedWindowsError {}

fn format_diff_side(names: &[String]) -> String {
    if names.is_empty() {
        "<none>".to_owned()
    } else {
        names.join(", ")
    }
}

fn require_sha256(field: &'static str, value: &str) -> Result<(), CedWindowsBuildEvidenceError> {
    if value.is_empty() {
        return Err(CedWindowsBuildEvidenceError::Missing { field });
    }
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(CedWindowsBuildEvidenceError::InvalidSha256 {
            field,
            found: value.to_owned(),
        });
    }
    Ok(())
}

impl CedWindowsBuildEvidence {
    /// Validate fields that do not need the enclosing receipt source identity.
    pub fn validate(&self) -> Result<(), CedWindowsBuildEvidenceError> {
        if self.schema != CED_WINDOWS_BUILD_EVIDENCE_SCHEMA_V1 {
            return Err(CedWindowsBuildEvidenceError::Schema {
                found: self.schema.clone(),
            });
        }
        if self.source_archive.label != CED_WINDOWS_SOURCE_ARCHIVE_LABEL {
            return Err(CedWindowsBuildEvidenceError::Unexpected {
                field: "CED source archive label",
                expected: CED_WINDOWS_SOURCE_ARCHIVE_LABEL.to_owned(),
                found: self.source_archive.label.clone(),
            });
        }
        if self.source_archive.size == 0 {
            return Err(CedWindowsBuildEvidenceError::Unexpected {
                field: "CED source archive size",
                expected: "nonzero".to_owned(),
                found: "0".to_owned(),
            });
        }
        require_sha256("CED source archive SHA-256", &self.source_archive.sha256)?;
        if self.cmake_archive.label != CED_WINDOWS_CMAKE_ARCHIVE_LABEL {
            return Err(CedWindowsBuildEvidenceError::Unexpected {
                field: "CMake archive label",
                expected: CED_WINDOWS_CMAKE_ARCHIVE_LABEL.to_owned(),
                found: self.cmake_archive.label.clone(),
            });
        }
        if self.cmake_archive.size == 0 {
            return Err(CedWindowsBuildEvidenceError::Unexpected {
                field: "CMake archive size",
                expected: "nonzero".to_owned(),
                found: "0".to_owned(),
            });
        }
        require_sha256("CMake archive SHA-256", &self.cmake_archive.sha256)?;
        provenance::require_repository(CED_GGML_REPOSITORY, &self.ggml.repository)
            .map_err(|source| CedWindowsBuildEvidenceError::Source { source })?;
        provenance::require_commit(GGML_COMMIT, &self.ggml.revision)
            .map_err(|source| CedWindowsBuildEvidenceError::Source { source })?;
        require_sha256("ggml source SHA-256", &self.ggml.content_sha256)?;
        let expected_definition_sha256 = sha256_hex(CED_WINDOWS_EXPORT_DEFINITION.as_bytes());
        if self.export_definition_sha256 != expected_definition_sha256 {
            return Err(CedWindowsBuildEvidenceError::Unexpected {
                field: "CED export definition SHA-256",
                expected: expected_definition_sha256,
                found: self.export_definition_sha256.clone(),
            });
        }
        require_sha256("CMake cache SHA-256", &self.cmake_cache_sha256)?;
        Ok(())
    }

    /// Bind this nested-source record to the one source identity carried by a
    /// V1 controlled-build receipt.
    pub fn validate_against_source(
        &self,
        source: &SourceIdentity,
    ) -> Result<(), CedWindowsBuildEvidenceError> {
        self.validate()?;
        verify_source_commits(source, &self.ggml.revision)
            .map_err(|source| CedWindowsBuildEvidenceError::Source { source })?;
        require_sha256(
            "CED source archive SHA-256",
            &source.windows_dependency.content_sha256,
        )?;
        if source.windows_dependency.content_sha256 != self.source_archive.sha256 {
            return Err(CedWindowsBuildEvidenceError::Unexpected {
                field: "CED source archive receipt binding",
                expected: self.source_archive.sha256.clone(),
                found: source.windows_dependency.content_sha256.clone(),
            });
        }
        Ok(())
    }

    /// SHA-256 of the canonical evidence JSON after structural validation.
    pub fn digest(&self) -> Result<String, CedWindowsBuildEvidenceError> {
        self.validate()?;
        Ok(sha256_hex(
            &serde_json::to_vec(self).expect("CED build evidence serialization"),
        ))
    }
}

/// Encode a structurally valid CED build-evidence sidecar. The caller must
/// additionally use [`CedWindowsBuildEvidence::validate_against_source`] when
/// binding it to a receipt's CED source identity.
pub fn encode_ced_windows_build_evidence(
    evidence: &CedWindowsBuildEvidence,
) -> Result<Vec<u8>, CedWindowsBuildEvidenceCodecError> {
    evidence
        .validate()
        .map_err(|source| CedWindowsBuildEvidenceCodecError::Validation { source })?;
    serde_json::to_vec(evidence)
        .map_err(|source| CedWindowsBuildEvidenceCodecError::Encode { source })
}

/// Decode and structurally validate a CED build-evidence sidecar.
pub fn decode_ced_windows_build_evidence(
    bytes: &[u8],
) -> Result<CedWindowsBuildEvidence, CedWindowsBuildEvidenceCodecError> {
    let evidence = serde_json::from_slice::<CedWindowsBuildEvidence>(bytes)
        .map_err(|source| CedWindowsBuildEvidenceCodecError::Decode { source })?;
    evidence
        .validate()
        .map_err(|source| CedWindowsBuildEvidenceCodecError::Validation { source })?;
    Ok(evidence)
}

pub fn verify_source_commits(
    identity: &SourceIdentity,
    ggml_revision: &str,
) -> Result<(), ProvenanceError> {
    provenance::require_repository(CED_CPP_REPOSITORY, &identity.windows_dependency.repository)?;
    provenance::require_commit(CED_CPP_COMMIT, &identity.windows_dependency.revision)?;
    provenance::require_commit(GGML_COMMIT, ggml_revision)?;
    Ok(())
}

pub fn ced_windows_build_configuration() -> BuildConfiguration {
    BuildConfiguration {
        target_triple: CED_WINDOWS_TARGET_TRIPLE.to_owned(),
        profile: CED_WINDOWS_BUILD_PROFILE.to_owned(),
        flags: CED_WINDOWS_BUILD_FLAGS
            .iter()
            .map(|flag| (*flag).to_owned())
            .collect(),
        network_access_denied: true,
    }
}

pub fn verify_exports(census: &PeInfo) -> Result<(), CedWindowsError> {
    let expected: BTreeSet<&str> = CED_WINDOWS_EXPORTS.iter().copied().collect();
    let actual: BTreeSet<String> = census
        .exports
        .iter()
        .map(|symbol| match symbol {
            PeSymbol::Named(name) => name.clone(),
            PeSymbol::Ordinal(ordinal) => format!("#{ordinal}"),
        })
        .collect();
    let missing: Vec<String> = expected
        .iter()
        .filter(|name| !actual.contains(**name))
        .map(|name| (*name).to_owned())
        .collect();
    let unexpected: Vec<String> = actual
        .into_iter()
        .filter(|name| !expected.contains(name.as_str()))
        .collect();
    if missing.is_empty() && unexpected.is_empty() {
        Ok(())
    } else {
        Err(CedWindowsError::ExportAllowlist {
            missing,
            unexpected,
        })
    }
}

pub fn verify_cpu_only_imports(census: &PeInfo) -> Result<(), CedWindowsError> {
    let libraries: Vec<String> = census
        .imports
        .iter()
        .filter(|library| {
            let lower = library.name.to_ascii_lowercase();
            FORBIDDEN_IMPORT_NEEDLES
                .iter()
                .any(|needle| lower.contains(needle))
        })
        .map(|library| library.name.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    if libraries.is_empty() {
        Ok(())
    } else {
        Err(CedWindowsError::GpuImport { libraries })
    }
}

fn verify_ced_output(outputs: &[OutputIdentityEntry]) -> Result<(), CedWindowsBuildEvidenceError> {
    let [output] = outputs else {
        return Err(CedWindowsBuildEvidenceError::OutputCount {
            found: outputs.len(),
        });
    };
    if output.label != CED_DLL_OUTPUT_LABEL {
        return Err(CedWindowsBuildEvidenceError::Unexpected {
            field: "CED output label",
            expected: CED_DLL_OUTPUT_LABEL.to_owned(),
            found: output.label.clone(),
        });
    }
    if output.census.machine != crate::pe::machine_amd64() {
        return Err(CedWindowsBuildEvidenceError::Unexpected {
            field: "CED output machine",
            expected: format!("{:#x}", crate::pe::machine_amd64()),
            found: format!("{:#x}", output.census.machine),
        });
    }
    verify_exports(&output.census)
        .and_then(|()| verify_cpu_only_imports(&output.census))
        .map_err(|source| CedWindowsBuildEvidenceError::OutputCensus { source })
}

pub fn assemble_receipt_draft(
    source: SourceIdentity,
    evidence: CedWindowsBuildEvidence,
    builder: BuilderIdentity,
    outputs: Vec<OutputIdentityEntry>,
) -> Result<ControlledBuildReceiptDraft, CedWindowsBuildEvidenceError> {
    evidence.validate_against_source(&source)?;
    verify_ced_output(&outputs)?;
    let evidence_digest = evidence.digest()?;
    Ok(ControlledBuildReceiptDraft {
        source: Some(source),
        inputs: Some(vec![evidence.source_archive, evidence.cmake_archive]),
        builder: Some(builder),
        configuration: Some(ced_windows_build_configuration()),
        outputs: Some(outputs),
        supporting: Some(vec![SupportingArtifactRef {
            label: CED_WINDOWS_BUILD_EVIDENCE_LABEL.to_owned(),
            sha256: evidence_digest,
        }]),
        ..ControlledBuildReceiptDraft::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controlled_build::{
        ControlledBuildReceiptError, DependencySource, ReceiptCategory, census_outputs,
    };
    use crate::pe::{FixtureSpec, ImportSpec, PeSymbolSpec, fixture, parse_pe};
    use crate::provenance::Provenance;

    fn pinned_source() -> SourceIdentity {
        SourceIdentity {
            product: Provenance {
                commit: "journal-product".to_owned(),
                lock_sha256: String::new(),
            },
            windows_dependency: DependencySource {
                repository: CED_CPP_REPOSITORY.into(),
                revision: CED_CPP_COMMIT.to_owned(),
                content_sha256: "a".repeat(64),
            },
        }
    }

    fn pinned_evidence(source: &SourceIdentity) -> CedWindowsBuildEvidence {
        CedWindowsBuildEvidence {
            schema: CED_WINDOWS_BUILD_EVIDENCE_SCHEMA_V1.to_owned(),
            source_archive: InputIdentityEntry {
                label: CED_WINDOWS_SOURCE_ARCHIVE_LABEL.to_owned(),
                sha256: source.windows_dependency.content_sha256.clone(),
                size: 1,
            },
            cmake_archive: InputIdentityEntry {
                label: CED_WINDOWS_CMAKE_ARCHIVE_LABEL.to_owned(),
                sha256: "d".repeat(64),
                size: 1,
            },
            ggml: DependencySource {
                repository: CED_GGML_REPOSITORY.to_owned(),
                revision: GGML_COMMIT.to_owned(),
                content_sha256: "b".repeat(64),
            },
            export_definition_sha256: sha256_hex(CED_WINDOWS_EXPORT_DEFINITION.as_bytes()),
            cmake_cache_sha256: "c".repeat(64),
        }
    }

    fn dummy_builder() -> BuilderIdentity {
        BuilderIdentity {
            host: "builder-1".into(),
            toolchain: "msvc".into(),
        }
    }

    fn named_exports<'a>(names: &'a [&'a str]) -> Vec<PeSymbolSpec<'a>> {
        names.iter().copied().map(PeSymbolSpec::Named).collect()
    }

    fn census_with_exports(exports: &[PeSymbolSpec<'_>]) -> PeInfo {
        parse_pe(&fixture(&FixtureSpec {
            dll: true,
            exports,
            ..FixtureSpec::default()
        }))
        .expect("fixture parses")
    }

    fn good_outputs() -> Vec<OutputIdentityEntry> {
        let exports = named_exports(CED_WINDOWS_EXPORTS);
        let dll = fixture(&FixtureSpec {
            dll: true,
            exports: &exports,
            ..FixtureSpec::default()
        });
        census_outputs(&[(CED_DLL_OUTPUT_LABEL, dll.as_slice())]).expect("CED fixture census")
    }

    #[test]
    fn assemble_receipt_draft_is_missing_schema_first() {
        let source = pinned_source();
        let draft = assemble_receipt_draft(
            source.clone(),
            pinned_evidence(&source),
            dummy_builder(),
            good_outputs(),
        )
        .expect("bound CED receipt draft");
        match draft.validate() {
            Err(ControlledBuildReceiptError::Missing {
                category: ReceiptCategory::Schema,
            }) => {}
            other => panic!("expected Missing Schema, got {other:?}"),
        }
    }

    #[test]
    fn build_evidence_codec_is_strict_and_round_trips() {
        let source = pinned_source();
        let evidence = pinned_evidence(&source);
        let bytes = encode_ced_windows_build_evidence(&evidence).expect("encode evidence");
        assert_eq!(
            decode_ced_windows_build_evidence(&bytes).expect("decode evidence"),
            evidence
        );

        let mut document = serde_json::to_value(&evidence).expect("evidence document");
        document
            .as_object_mut()
            .expect("evidence object")
            .insert("unknown".into(), serde_json::json!(true));
        assert!(matches!(
            decode_ced_windows_build_evidence(&serde_json::to_vec(&document).unwrap()),
            Err(CedWindowsBuildEvidenceCodecError::Decode { .. })
        ));
    }

    #[test]
    fn build_evidence_binds_the_nested_ggml_and_receipt_source_archive() {
        let source = pinned_source();
        let evidence = pinned_evidence(&source);
        evidence
            .validate_against_source(&source)
            .expect("nested identity binds");

        let mut wrong_ggml = evidence.clone();
        wrong_ggml.ggml.repository = "https://example.invalid/ggml".into();
        assert!(matches!(
            wrong_ggml.validate_against_source(&source),
            Err(CedWindowsBuildEvidenceError::Source { .. })
        ));

        let mut wrong_archive = source;
        wrong_archive.windows_dependency.content_sha256 = "d".repeat(64);
        match evidence.validate_against_source(&wrong_archive) {
            Err(CedWindowsBuildEvidenceError::Unexpected { field, .. }) => {
                assert_eq!(field, "CED source archive receipt binding");
            }
            other => panic!("expected archive binding refusal, got {other:?}"),
        }
    }

    #[test]
    fn receipt_draft_binds_evidence_digest_and_rejects_an_unreviewed_output() {
        let source = pinned_source();
        let evidence = pinned_evidence(&source);
        let evidence_digest = evidence.digest().expect("evidence digest");
        let draft = assemble_receipt_draft(source, evidence, dummy_builder(), good_outputs())
            .expect("bound receipt draft");
        assert_eq!(draft.inputs.expect("build inputs").len(), 2);
        assert_eq!(
            draft.supporting.expect("build evidence sidecar").as_slice(),
            [SupportingArtifactRef {
                label: CED_WINDOWS_BUILD_EVIDENCE_LABEL.to_owned(),
                sha256: evidence_digest,
            }]
        );

        let source = pinned_source();
        let evidence = pinned_evidence(&source);
        let mut exports = named_exports(CED_WINDOWS_EXPORTS);
        exports.push(PeSymbolSpec::Named("ced_capi_unreviewed"));
        let dll = fixture(&FixtureSpec {
            dll: true,
            exports: &exports,
            ..FixtureSpec::default()
        });
        let outputs = census_outputs(&[(CED_DLL_OUTPUT_LABEL, dll.as_slice())])
            .expect("census permits an output before CED admission");
        assert!(matches!(
            assemble_receipt_draft(source, evidence, dummy_builder(), outputs),
            Err(CedWindowsBuildEvidenceError::OutputCensus {
                source: CedWindowsError::ExportAllowlist { .. }
            })
        ));
    }

    #[test]
    fn census_outputs_is_fail_closed_and_names_the_ced_dll() {
        let exports = named_exports(CED_WINDOWS_EXPORTS);
        let good = fixture(&FixtureSpec {
            dll: true,
            exports: &exports,
            ..FixtureSpec::default()
        });
        let bad = b"not-a-pe";
        match census_outputs(&[
            (CED_DLL_OUTPUT_LABEL, bad.as_slice()),
            ("other", good.as_slice()),
        ]) {
            Err(ControlledBuildReceiptError::OutputParse { label, .. }) => {
                assert_eq!(label, CED_DLL_OUTPUT_LABEL);
            }
            Ok(_) => panic!("must not return Ok with a partial census"),
            Err(other) => panic!("expected OutputParse, got {other:?}"),
        }
    }

    #[test]
    fn verify_source_commits_rejects_wrong_ced_cpp_commit() {
        let mut identity = pinned_source();
        identity.windows_dependency.revision = "wrong-ced-cpp".into();
        let error =
            verify_source_commits(&identity, GGML_COMMIT).expect_err("wrong ced.cpp commit");
        assert!(
            error
                .to_string()
                .contains("mismatched-commit wrong-ced-cpp"),
            "{error}"
        );
    }

    #[test]
    fn verify_source_commits_rejects_wrong_ced_cpp_repository() {
        let mut identity = pinned_source();
        identity.windows_dependency.repository = "https://example.invalid/ced.cpp.git".into();
        let error =
            verify_source_commits(&identity, GGML_COMMIT).expect_err("wrong ced.cpp repository");
        assert!(
            error
                .to_string()
                .contains("mismatched-repository https://example.invalid/ced.cpp.git"),
            "{error}"
        );
    }

    #[test]
    fn verify_source_commits_rejects_wrong_ggml_revision() {
        let error =
            verify_source_commits(&pinned_source(), "wrong-ggml").expect_err("wrong ggml revision");
        assert!(
            error.to_string().contains("mismatched-commit wrong-ggml"),
            "{error}"
        );
    }

    #[test]
    fn verify_source_commits_accepts_pinned_commits() {
        verify_source_commits(&pinned_source(), GGML_COMMIT).expect("pinned commits");
    }

    #[test]
    fn build_configuration_names_the_pinned_source_switches() {
        assert_eq!(
            CED_WINDOWS_BUILD_FLAGS,
            [
                "-DCED_SHARED=ON",
                "-DCED_BUILD_CLI=OFF",
                "-DCED_BUILD_TESTS=OFF",
                "-DCED_GGML_CUDA=OFF",
                "-DCED_GGML_METAL=OFF",
                "-DCED_GGML_VULKAN=OFF",
                "-DCED_GGML_HIP=OFF",
                "-DBUILD_SHARED_LIBS=OFF",
                "-DGGML_NATIVE=OFF",
                "-DGGML_LLAMAFILE=OFF",
                "-DGGML_OPENCL=OFF",
                "-DGGML_BACKEND_DL=OFF",
                "-DGGML_OPENMP=ON",
            ]
        );
    }

    #[test]
    fn export_definition_is_the_exact_reviewed_abi() {
        let listed = CED_WINDOWS_EXPORT_DEFINITION
            .lines()
            .skip(2)
            .map(str::trim)
            .collect::<Vec<_>>();
        assert_eq!(listed, CED_WINDOWS_EXPORTS);
    }

    #[test]
    fn verify_exports_rejects_seventh_export() {
        let mut exports = named_exports(CED_WINDOWS_EXPORTS);
        exports.push(PeSymbolSpec::Named("ced_capi_extra_thing"));
        match verify_exports(&census_with_exports(&exports)) {
            Err(CedWindowsError::ExportAllowlist {
                missing,
                unexpected,
            }) => {
                assert!(missing.is_empty(), "{missing:?}");
                assert_eq!(unexpected, vec!["ced_capi_extra_thing".to_owned()]);
            }
            other => panic!("expected ExportAllowlist, got {other:?}"),
        }
    }

    #[test]
    fn verify_exports_rejects_missing_export() {
        let exports = named_exports(&CED_WINDOWS_EXPORTS[..5]);
        match verify_exports(&census_with_exports(&exports)) {
            Err(CedWindowsError::ExportAllowlist {
                missing,
                unexpected,
            }) => {
                assert_eq!(missing, vec!["ced_capi_free_string".to_owned()]);
                assert!(unexpected.is_empty(), "{unexpected:?}");
            }
            other => panic!("expected ExportAllowlist, got {other:?}"),
        }
    }

    #[test]
    fn verify_exports_accepts_exact_six_in_any_order() {
        let mut names = CED_WINDOWS_EXPORTS.to_vec();
        names.reverse();
        let exports = named_exports(&names);
        verify_exports(&census_with_exports(&exports)).expect("six exports in reverse order");
    }

    #[test]
    fn verify_cpu_only_imports_rejects_cuda_library() {
        let info = parse_pe(&fixture(&FixtureSpec {
            imports: &[ImportSpec {
                name: "nvcuda.dll",
                symbols: &[],
            }],
            ..FixtureSpec::default()
        }))
        .expect("fixture parses");
        match verify_cpu_only_imports(&info) {
            Err(CedWindowsError::GpuImport { libraries }) => {
                assert_eq!(libraries, vec!["nvcuda.dll".to_owned()]);
            }
            other => panic!("expected GpuImport, got {other:?}"),
        }
    }

    #[test]
    fn verify_cpu_only_imports_allows_empty_and_system_libs() {
        let empty = parse_pe(&fixture(&FixtureSpec::default())).expect("empty fixture");
        verify_cpu_only_imports(&empty).expect("empty imports");
        let system = parse_pe(&fixture(&FixtureSpec {
            imports: &[ImportSpec {
                name: "KERNEL32.dll",
                symbols: &[],
            }],
            ..FixtureSpec::default()
        }))
        .expect("system fixture");
        verify_cpu_only_imports(&system).expect("system imports");
    }
}
