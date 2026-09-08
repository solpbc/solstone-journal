// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Windows RF-DETR (rf-detr.cpp + ggml) controlled-build identity: pins,
//! evidence schemas, cache/build-option validators, PE checks, and receipt-draft assembly.

use std::collections::BTreeSet;
use std::fmt;

use crate::controlled_build::{
    BuildConfiguration, BuilderIdentity, ControlledBuildReceiptDraft, DependencySource,
    InputIdentityEntry, OutputIdentityEntry, SourceIdentity, SupportingArtifactRef,
};
use crate::digest::sha256_hex;
use crate::pe::PeInfo;
use crate::provenance::{self, ProvenanceError};

pub const RF_DETR_COMMIT: &str = "ec73712e4c933dc36e79963cab47d702c34f02bf";
pub const RF_DETR_REPOSITORY: &str = "https://github.com/solpbc/rf-detr.cpp.git";
pub const GGML_COMMIT: &str = "e705c5fed490514458bdd2eaddc43bd098fcce9b";
pub const GGML_REPOSITORY: &str = "https://github.com/ggml-org/ggml";

pub const ENGINE_VERSION: &str = "v0.1.0-solpbc.5";
pub const TARGET_TRIPLE: &str = "x86_64-pc-windows-msvc";
pub const BUILD_PROFILE: &str = "Release";

pub const RFDETR_CLI_OUTPUT_LABEL: &str = "bin/rfdetr-cli.exe";

pub const RFDETR_WINDOWS_BUILD_EVIDENCE_SCHEMA_V1: &str =
    "solstone.rfdetr-windows-build-evidence.v1";
pub const RFDETR_WINDOWS_BUILD_EVIDENCE_LABEL: &str =
    "provenance/windows-x86_64/rfdetr-build-evidence.json";
pub const RFDETR_WINDOWS_CMAKE_CACHE_LABEL: &str =
    "provenance/windows-x86_64/rfdetr-CMakeCache.txt";
pub const RFDETR_WINDOWS_VCXPROJ_LABEL: &str =
    "provenance/windows-x86_64/rfdetr-cli.Release.vcxproj";
pub const RFDETR_WINDOWS_RF_BUNDLE_LABEL: &str = "sources/rf-detr.cpp.bundle";
pub const RFDETR_WINDOWS_GGML_BUNDLE_LABEL: &str = "sources/ggml.bundle";
pub const RFDETR_WINDOWS_CMAKE_ARCHIVE_LABEL: &str = "tools/cmake-windows-x86_64.zip";

pub const CANONICAL_BOOTSTRAP_LOCK_PATH: &str =
    r"C:\ProgramData\solstone\journal-win-bootstrap.lock";

pub const RFDETR_WINDOWS_BUILD_FLAGS: &[&str] = &[
    "-DRFDETR_SHARED=OFF",
    "-DRFDETR_BUILD_CLI=ON",
    "-DRFDETR_BUILD_TESTS=OFF",
    "-DRFDETR_BUILD_EXAMPLES=OFF",
    "-DRFDETR_GGML_CUDA=OFF",
    "-DRFDETR_GGML_METAL=OFF",
    "-DRFDETR_GGML_VULKAN=OFF",
    "-DRFDETR_GGML_HIPBLAS=OFF",
    "-DBUILD_SHARED_LIBS=OFF",
    "-DGGML_NATIVE=OFF",
    "-DGGML_AVX=OFF",
    "-DGGML_AVX2=OFF",
    "-DGGML_AVX512=OFF",
    "-DGGML_FMA=OFF",
    "-DGGML_F16C=OFF",
    "-DGGML_SSE42=OFF",
    "-DGGML_BMI2=OFF",
    "-DGGML_OPENCL=OFF",
    "-DGGML_BACKEND_DL=OFF",
    "-DGGML_LLAMAFILE=ON",
    "-DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreadedDLL",
    "-DCMAKE_BUILD_TYPE=Release",
    "-DGGML_OPENMP=ON",
    "-DGGML_CPU=ON",
    "-DGGML_CPU_ALL_VARIANTS=OFF",
    "-DGGML_CCACHE=OFF",
    "-DGGML_AVX512_BF16=OFF",
    "-DGGML_AVX512_VBMI=OFF",
    "-DGGML_AVX512_VNNI=OFF",
    "-DGGML_AVX_VNNI=OFF",
    "-DGGML_BLAS=OFF",
    "-DGGML_CUDA=OFF",
    "-DGGML_METAL=OFF",
    "-DGGML_VULKAN=OFF",
    "-DGGML_HIP=OFF",
    "-DGGML_SYCL=OFF",
    "-DGGML_RPC=OFF",
    "-DGGML_OPENVINO=OFF",
    "-DGGML_HEXAGON=OFF",
    "-DGGML_MUSA=OFF",
    "-DGGML_VIRTGPU=OFF",
    "-DGGML_VIRTGPU_BACKEND=OFF",
    "-DGGML_WEBGPU=OFF",
    "-DGGML_ZDNN=OFF",
    "-DGGML_ZENDNN=OFF",
];

const FORBIDDEN_IMPORT_NEEDLES: &[&str] = &[
    "cuda", "nvcuda", "cudart", "cublas", "cudnn", "nvrtc", "vulkan", "opencl", "directml", "ggml",
];

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RfdetrWindowsSubprocessRecord {
    pub label: String,
    pub argv: Vec<String>,
    pub cwd: String,
    pub exit_code: i32,
    pub stdout: InputIdentityEntry,
    pub stderr: InputIdentityEntry,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RfdetrWindowsBuildEvidence {
    pub schema: String,
    pub rf_bundle: InputIdentityEntry,
    pub ggml_bundle: InputIdentityEntry,
    pub ggml: DependencySource,
    pub cmake_archive: InputIdentityEntry,
    pub cmake_cache: InputIdentityEntry,
    pub build_option_evidence: InputIdentityEntry,
    pub subprocesses: Vec<RfdetrWindowsSubprocessRecord>,
}

#[derive(Debug)]
pub enum RfdetrWindowsBuildEvidenceError {
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
        source: RfdetrWindowsError,
    },
    SubprocessFailed {
        label: String,
        exit_code: i32,
    },
    CacheValidation {
        reason: String,
    },
    BuildOptionValidation {
        reason: String,
    },
}

impl fmt::Display for RfdetrWindowsBuildEvidenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Schema { found } => write!(
                formatter,
                "unexpected:\n  build evidence schema {found}\n  expected: {RFDETR_WINDOWS_BUILD_EVIDENCE_SCHEMA_V1}"
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
                "unexpected:\n  RF-DETR output count\n  expected: 1\n  found: {found}"
            ),
            Self::OutputCensus { source } => write!(formatter, "RF-DETR output census: {source}"),
            Self::SubprocessFailed { label, exit_code } => write!(
                formatter,
                "subprocess {label} failed with nonzero exit code {exit_code}"
            ),
            Self::CacheValidation { reason } => {
                write!(formatter, "CMake cache validation failed: {reason}")
            }
            Self::BuildOptionValidation { reason } => {
                write!(formatter, "Build option validation failed: {reason}")
            }
        }
    }
}

impl std::error::Error for RfdetrWindowsBuildEvidenceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Source { source } => Some(source),
            Self::OutputCensus { source } => Some(source),
            Self::Schema { .. }
            | Self::Unexpected { .. }
            | Self::Missing { .. }
            | Self::InvalidSha256 { .. }
            | Self::OutputCount { .. }
            | Self::SubprocessFailed { .. }
            | Self::CacheValidation { .. }
            | Self::BuildOptionValidation { .. } => None,
        }
    }
}

#[derive(Debug)]
pub enum RfdetrWindowsBuildEvidenceCodecError {
    Encode {
        source: serde_json::Error,
    },
    Decode {
        source: serde_json::Error,
    },
    Validation {
        source: RfdetrWindowsBuildEvidenceError,
    },
}

impl fmt::Display for RfdetrWindowsBuildEvidenceCodecError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Encode { source } => {
                write!(
                    formatter,
                    "could not encode RF-DETR build evidence: {source}"
                )
            }
            Self::Decode { source } => {
                write!(
                    formatter,
                    "could not decode RF-DETR build evidence: {source}"
                )
            }
            Self::Validation { source } => {
                write!(formatter, "invalid RF-DETR build evidence: {source}")
            }
        }
    }
}

impl std::error::Error for RfdetrWindowsBuildEvidenceCodecError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Encode { source } | Self::Decode { source } => Some(source),
            Self::Validation { source } => Some(source),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RfdetrWindowsError {
    GpuImport { libraries: Vec<String> },
}

impl fmt::Display for RfdetrWindowsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GpuImport { libraries } => write!(
                formatter,
                "RF-DETR Windows import closure is not CPU-only\n  missing: <none>\n  unexpected: {}",
                format_diff_side(libraries)
            ),
        }
    }
}

impl std::error::Error for RfdetrWindowsError {}

fn format_diff_side(names: &[String]) -> String {
    if names.is_empty() {
        "<none>".to_owned()
    } else {
        names.join(", ")
    }
}

fn require_sha256(field: &'static str, value: &str) -> Result<(), RfdetrWindowsBuildEvidenceError> {
    if value.is_empty() {
        return Err(RfdetrWindowsBuildEvidenceError::Missing { field });
    }
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(RfdetrWindowsBuildEvidenceError::InvalidSha256 {
            field,
            found: value.to_owned(),
        });
    }
    Ok(())
}

impl RfdetrWindowsBuildEvidence {
    pub fn validate(&self) -> Result<(), RfdetrWindowsBuildEvidenceError> {
        if self.schema != RFDETR_WINDOWS_BUILD_EVIDENCE_SCHEMA_V1 {
            return Err(RfdetrWindowsBuildEvidenceError::Schema {
                found: self.schema.clone(),
            });
        }
        if self.rf_bundle.label != RFDETR_WINDOWS_RF_BUNDLE_LABEL {
            return Err(RfdetrWindowsBuildEvidenceError::Unexpected {
                field: "RF-DETR bundle label",
                expected: RFDETR_WINDOWS_RF_BUNDLE_LABEL.to_owned(),
                found: self.rf_bundle.label.clone(),
            });
        }
        if self.rf_bundle.size == 0 {
            return Err(RfdetrWindowsBuildEvidenceError::Unexpected {
                field: "RF-DETR bundle size",
                expected: "nonzero".to_owned(),
                found: "0".to_owned(),
            });
        }
        require_sha256("RF-DETR bundle SHA-256", &self.rf_bundle.sha256)?;

        if self.ggml_bundle.label != RFDETR_WINDOWS_GGML_BUNDLE_LABEL {
            return Err(RfdetrWindowsBuildEvidenceError::Unexpected {
                field: "GGML bundle label",
                expected: RFDETR_WINDOWS_GGML_BUNDLE_LABEL.to_owned(),
                found: self.ggml_bundle.label.clone(),
            });
        }
        if self.ggml_bundle.size == 0 {
            return Err(RfdetrWindowsBuildEvidenceError::Unexpected {
                field: "GGML bundle size",
                expected: "nonzero".to_owned(),
                found: "0".to_owned(),
            });
        }
        require_sha256("GGML bundle SHA-256", &self.ggml_bundle.sha256)?;

        provenance::require_repository(GGML_REPOSITORY, &self.ggml.repository)
            .map_err(|source| RfdetrWindowsBuildEvidenceError::Source { source })?;
        provenance::require_commit(GGML_COMMIT, &self.ggml.revision)
            .map_err(|source| RfdetrWindowsBuildEvidenceError::Source { source })?;
        require_sha256("GGML source SHA-256", &self.ggml.content_sha256)?;
        if self.ggml.content_sha256 != self.ggml_bundle.sha256 {
            return Err(RfdetrWindowsBuildEvidenceError::Unexpected {
                field: "GGML dependency content SHA-256 vs bundle SHA-256",
                expected: self.ggml_bundle.sha256.clone(),
                found: self.ggml.content_sha256.clone(),
            });
        }

        if self.cmake_archive.label != RFDETR_WINDOWS_CMAKE_ARCHIVE_LABEL {
            return Err(RfdetrWindowsBuildEvidenceError::Unexpected {
                field: "CMake archive label",
                expected: RFDETR_WINDOWS_CMAKE_ARCHIVE_LABEL.to_owned(),
                found: self.cmake_archive.label.clone(),
            });
        }
        if self.cmake_archive.size == 0 {
            return Err(RfdetrWindowsBuildEvidenceError::Unexpected {
                field: "CMake archive size",
                expected: "nonzero".to_owned(),
                found: "0".to_owned(),
            });
        }
        require_sha256("CMake archive SHA-256", &self.cmake_archive.sha256)?;

        if self.cmake_cache.label != RFDETR_WINDOWS_CMAKE_CACHE_LABEL {
            return Err(RfdetrWindowsBuildEvidenceError::Unexpected {
                field: "CMake cache label",
                expected: RFDETR_WINDOWS_CMAKE_CACHE_LABEL.to_owned(),
                found: self.cmake_cache.label.clone(),
            });
        }
        require_sha256("CMake cache SHA-256", &self.cmake_cache.sha256)?;

        if self.build_option_evidence.label != RFDETR_WINDOWS_VCXPROJ_LABEL {
            return Err(RfdetrWindowsBuildEvidenceError::Unexpected {
                field: "Build option evidence label",
                expected: RFDETR_WINDOWS_VCXPROJ_LABEL.to_owned(),
                found: self.build_option_evidence.label.clone(),
            });
        }
        require_sha256(
            "Build option evidence SHA-256",
            &self.build_option_evidence.sha256,
        )?;

        let mut labels = BTreeSet::new();
        for process in &self.subprocesses {
            if process.label.is_empty()
                || !process
                    .label
                    .bytes()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == b'-')
                || !labels.insert(process.label.as_str())
                || process.argv.is_empty()
                || process.argv.iter().any(|arg| arg.contains('\0'))
                || process.cwd.is_empty()
                || process.cwd.contains('\0')
                || process.stdout.label != format!("logs/{}.stdout", process.label)
                || process.stderr.label != format!("logs/{}.stderr", process.label)
            {
                return Err(RfdetrWindowsBuildEvidenceError::Unexpected {
                    field: "subprocess identity",
                    expected: "unique safe label, argv, cwd and named original streams".to_owned(),
                    found: process.label.clone(),
                });
            }
        }
        for required in ["cmake-configure", "cmake-build"] {
            if !labels.contains(required) {
                return Err(RfdetrWindowsBuildEvidenceError::Missing { field: required });
            }
        }
        for proc in &self.subprocesses {
            if proc.exit_code != 0 {
                return Err(RfdetrWindowsBuildEvidenceError::SubprocessFailed {
                    label: proc.label.clone(),
                    exit_code: proc.exit_code,
                });
            }
            require_sha256("Subprocess stdout SHA-256", &proc.stdout.sha256)?;
            require_sha256("Subprocess stderr SHA-256", &proc.stderr.sha256)?;
        }

        Ok(())
    }

    pub fn validate_against_source(
        &self,
        source: &SourceIdentity,
    ) -> Result<(), RfdetrWindowsBuildEvidenceError> {
        self.validate()?;
        verify_source_commits(source, &self.ggml.revision)
            .map_err(|source| RfdetrWindowsBuildEvidenceError::Source { source })?;
        require_sha256(
            "RF-DETR source archive SHA-256",
            &source.windows_dependency.content_sha256,
        )?;
        if source.windows_dependency.content_sha256 != self.rf_bundle.sha256 {
            return Err(RfdetrWindowsBuildEvidenceError::Unexpected {
                field: "RF-DETR source archive receipt binding",
                expected: self.rf_bundle.sha256.clone(),
                found: source.windows_dependency.content_sha256.clone(),
            });
        }
        Ok(())
    }

    pub fn digest(&self) -> Result<String, RfdetrWindowsBuildEvidenceError> {
        self.validate()?;
        Ok(sha256_hex(
            &serde_json::to_vec(self).expect("RF-DETR build evidence serialization"),
        ))
    }
}

pub fn encode_rfdetr_windows_build_evidence(
    evidence: &RfdetrWindowsBuildEvidence,
) -> Result<Vec<u8>, RfdetrWindowsBuildEvidenceCodecError> {
    evidence
        .validate()
        .map_err(|source| RfdetrWindowsBuildEvidenceCodecError::Validation { source })?;
    serde_json::to_vec(evidence)
        .map_err(|source| RfdetrWindowsBuildEvidenceCodecError::Encode { source })
}

pub fn decode_rfdetr_windows_build_evidence(
    bytes: &[u8],
) -> Result<RfdetrWindowsBuildEvidence, RfdetrWindowsBuildEvidenceCodecError> {
    let evidence = serde_json::from_slice::<RfdetrWindowsBuildEvidence>(bytes)
        .map_err(|source| RfdetrWindowsBuildEvidenceCodecError::Decode { source })?;
    evidence
        .validate()
        .map_err(|source| RfdetrWindowsBuildEvidenceCodecError::Validation { source })?;
    Ok(evidence)
}

pub fn verify_source_commits(
    identity: &SourceIdentity,
    ggml_revision: &str,
) -> Result<(), ProvenanceError> {
    provenance::require_repository(RF_DETR_REPOSITORY, &identity.windows_dependency.repository)?;
    provenance::require_commit(RF_DETR_COMMIT, &identity.windows_dependency.revision)?;
    provenance::require_commit(GGML_COMMIT, ggml_revision)?;
    Ok(())
}

pub fn rfdetr_windows_build_configuration() -> BuildConfiguration {
    BuildConfiguration {
        target_triple: TARGET_TRIPLE.to_owned(),
        profile: BUILD_PROFILE.to_owned(),
        flags: RFDETR_WINDOWS_BUILD_FLAGS
            .iter()
            .map(|flag| (*flag).to_owned())
            .collect(),
        network_access_denied: true,
    }
}

pub fn verify_cpu_only_imports(census: &PeInfo) -> Result<(), RfdetrWindowsError> {
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
        Err(RfdetrWindowsError::GpuImport { libraries })
    }
}

fn verify_rfdetr_output(
    outputs: &[OutputIdentityEntry],
) -> Result<(), RfdetrWindowsBuildEvidenceError> {
    let [output] = outputs else {
        return Err(RfdetrWindowsBuildEvidenceError::OutputCount {
            found: outputs.len(),
        });
    };
    if output.label != RFDETR_CLI_OUTPUT_LABEL {
        return Err(RfdetrWindowsBuildEvidenceError::Unexpected {
            field: "RF-DETR output label",
            expected: RFDETR_CLI_OUTPUT_LABEL.to_owned(),
            found: output.label.clone(),
        });
    }
    if output.census.machine != crate::pe::machine_amd64() {
        return Err(RfdetrWindowsBuildEvidenceError::Unexpected {
            field: "RF-DETR output machine",
            expected: format!("{:#x}", crate::pe::machine_amd64()),
            found: format!("{:#x}", output.census.machine),
        });
    }
    verify_cpu_only_imports(&output.census)
        .map_err(|source| RfdetrWindowsBuildEvidenceError::OutputCensus { source })
}

pub fn validate_cmake_cache(bytes: &[u8]) -> Result<(), RfdetrWindowsBuildEvidenceError> {
    let text = std::str::from_utf8(bytes).map_err(|_| {
        RfdetrWindowsBuildEvidenceError::CacheValidation {
            reason: "CMakeCache.txt is not valid UTF-8".to_owned(),
        }
    })?;

    let mut cache_entries = std::collections::BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.starts_with("//") || line.is_empty() {
            continue;
        }
        if let Some((key_part, val_part)) = line.split_once('=') {
            let key = if let Some((k, _type)) = key_part.split_once(':') {
                k.trim()
            } else {
                key_part.trim()
            };
            if cache_entries
                .insert(key.to_owned(), val_part.trim().to_owned())
                .is_some()
            {
                return Err(RfdetrWindowsBuildEvidenceError::CacheValidation {
                    reason: format!("duplicate CMake cache key {key}"),
                });
            }
        }
    }

    let required_flags = [
        ("RFDETR_SHARED", "OFF"),
        ("RFDETR_BUILD_CLI", "ON"),
        ("RFDETR_BUILD_TESTS", "OFF"),
        ("RFDETR_BUILD_EXAMPLES", "OFF"),
        ("RFDETR_GGML_CUDA", "OFF"),
        ("RFDETR_GGML_METAL", "OFF"),
        ("RFDETR_GGML_VULKAN", "OFF"),
        ("RFDETR_GGML_HIPBLAS", "OFF"),
        ("BUILD_SHARED_LIBS", "OFF"),
        ("GGML_NATIVE", "OFF"),
        ("GGML_AVX", "OFF"),
        ("GGML_AVX2", "OFF"),
        ("GGML_AVX512", "OFF"),
        ("GGML_FMA", "OFF"),
        ("GGML_F16C", "OFF"),
        ("GGML_SSE42", "OFF"),
        ("GGML_BMI2", "OFF"),
        ("GGML_OPENCL", "OFF"),
        ("GGML_BACKEND_DL", "OFF"),
        ("GGML_LLAMAFILE", "ON"),
        ("CMAKE_BUILD_TYPE", "Release"),
        ("GGML_OPENMP", "ON"),
        ("GGML_CPU", "ON"),
        ("GGML_CPU_ALL_VARIANTS", "OFF"),
        ("GGML_CCACHE", "OFF"),
        ("GGML_AVX512_BF16", "OFF"),
        ("GGML_AVX512_VBMI", "OFF"),
        ("GGML_AVX512_VNNI", "OFF"),
        ("GGML_AVX_VNNI", "OFF"),
        ("GGML_BLAS", "OFF"),
        ("GGML_CUDA", "OFF"),
        ("GGML_METAL", "OFF"),
        ("GGML_VULKAN", "OFF"),
        ("GGML_HIP", "OFF"),
        ("GGML_SYCL", "OFF"),
        ("GGML_RPC", "OFF"),
        ("GGML_OPENVINO", "OFF"),
        ("GGML_HEXAGON", "OFF"),
        ("GGML_MUSA", "OFF"),
        ("GGML_VIRTGPU", "OFF"),
        ("GGML_VIRTGPU_BACKEND", "OFF"),
        ("GGML_WEBGPU", "OFF"),
        ("GGML_ZDNN", "OFF"),
        ("GGML_ZENDNN", "OFF"),
    ];

    for (key, expected_val) in required_flags {
        match cache_entries.get(key) {
            Some(val) if val.eq_ignore_ascii_case(expected_val) => {}
            Some(other) => {
                return Err(RfdetrWindowsBuildEvidenceError::CacheValidation {
                    reason: format!(
                        "CMake cache entry {key} has unexpected value {other:?}, expected {expected_val:?}"
                    ),
                });
            }
            None => {
                return Err(RfdetrWindowsBuildEvidenceError::CacheValidation {
                    reason: format!("CMake cache entry {key} is missing"),
                });
            }
        }
    }

    if cache_entries
        .get("CMAKE_MSVC_RUNTIME_LIBRARY")
        .map(String::as_str)
        != Some("MultiThreadedDLL")
    {
        return Err(RfdetrWindowsBuildEvidenceError::CacheValidation {
            reason: "CMAKE_MSVC_RUNTIME_LIBRARY must be exactly MultiThreadedDLL (/MD)".to_owned(),
        });
    }

    for flag_var in [
        "CMAKE_CXX_FLAGS",
        "CMAKE_C_FLAGS",
        "CMAKE_CXX_FLAGS_RELEASE",
        "CMAKE_C_FLAGS_RELEASE",
    ] {
        if let Some(flags) = cache_entries.get(flag_var) {
            let tokens: Vec<&str> = flags.split_whitespace().collect();
            if tokens.iter().any(|token| {
                matches!(*token, "/MT" | "-MT" | "/MTd" | "-MTd" | "/MDd" | "-MDd")
                    || token.to_ascii_lowercase().contains("arch:avx")
            }) {
                return Err(RfdetrWindowsBuildEvidenceError::CacheValidation {
                    reason: format!(
                        "{flag_var} contains a static/debug CRT or non-baseline ISA override"
                    ),
                });
            }
        }
    }

    Ok(())
}

/// Validate the generated Release/x64 compiler settings, not textual mentions
/// elsewhere in the project. This deliberately accepts only CMake's emitted
/// condition form; it does not attempt to evaluate arbitrary MSBuild programs.
pub fn validate_build_option_evidence(bytes: &[u8]) -> Result<(), RfdetrWindowsBuildEvidenceError> {
    use quick_xml::{Reader, events::Event};
    let refuse = |reason: &str| RfdetrWindowsBuildEvidenceError::BuildOptionValidation {
        reason: reason.to_owned(),
    };
    if bytes.len() > 8 * 1024 * 1024 {
        return Err(refuse("vcxproj exceeds evidence limit"));
    }
    let text =
        std::str::from_utf8(bytes).map_err(|_| refuse("vcxproj evidence is not valid UTF-8"))?;
    let mut reader = Reader::from_str(text);
    reader.config_mut().expand_empty_elements = true;
    let mut stack = Vec::<String>::new();
    let mut group = None;
    let mut active_field: Option<(String, String)> = None;
    let mut runtime_count = 0;
    let mut root_count = 0;
    loop {
        match reader
            .read_event()
            .map_err(|_| refuse("malformed vcxproj XML"))?
        {
            Event::Start(tag) => {
                let name = std::str::from_utf8(tag.name().as_ref())
                    .map_err(|_| refuse("invalid vcxproj element name"))?
                    .to_owned();
                if stack.is_empty() {
                    root_count += 1;
                    if name != "Project" || root_count != 1 {
                        return Err(refuse("vcxproj must have one Project root"));
                    }
                }
                if active_field.is_some() {
                    return Err(refuse("nested compiler setting is not supported"));
                }
                let mut condition = None;
                for attribute in tag.attributes() {
                    let attribute = attribute.map_err(|_| refuse("invalid vcxproj attribute"))?;
                    if attribute.key.as_ref() == b"Condition" {
                        condition = Some(
                            attribute
                                .decoded_and_normalized_value(
                                    quick_xml::XmlVersion::Explicit1_0,
                                    reader.decoder(),
                                )
                                .map_err(|_| refuse("invalid vcxproj condition"))?
                                .into_owned(),
                        );
                    }
                }
                if name == "ItemDefinitionGroup" {
                    if group.is_some() || stack.as_slice() != ["Project"] {
                        return Err(refuse("unexpected compiler group location"));
                    }
                    let condition = condition
                        .as_ref()
                        .ok_or_else(|| refuse("unconditional compiler group is not supported"))?;
                    group = Some(match condition.as_str() {
                        "'$(Configuration)|$(Platform)'=='Release|x64'" => true,
                        "'$(Configuration)|$(Platform)'=='Debug|x64'"
                        | "'$(Configuration)|$(Platform)'=='RelWithDebInfo|x64'"
                        | "'$(Configuration)|$(Platform)'=='MinSizeRel|x64'" => false,
                        _ => return Err(refuse("unsupported compiler group condition")),
                    });
                } else if name == "ClCompile" && group == Some(true) && condition.is_some() {
                    return Err(refuse(
                        "conditional Release compiler settings are not supported",
                    ));
                }
                if matches!(
                    name.as_str(),
                    "RuntimeLibrary" | "EnableEnhancedInstructionSet" | "AdditionalOptions"
                ) && stack.last().is_some_and(|value| value == "ClCompile")
                {
                    if group.is_none() {
                        return Err(refuse("compiler override outside a configuration group"));
                    }
                    if group == Some(true) {
                        if condition.is_some() {
                            return Err(refuse(
                                "conditional Release compiler setting is not supported",
                            ));
                        }
                        active_field = Some((name.clone(), String::new()));
                    }
                }
                stack.push(name);
            }
            Event::Text(value) => {
                let value = value.decode().map_err(|_| refuse("invalid vcxproj text"))?;
                if let Some((_, content)) = active_field.as_mut() {
                    content.push_str(&value);
                } else if stack.is_empty() && !value.trim().is_empty() {
                    return Err(refuse("text outside vcxproj root"));
                }
            }
            Event::GeneralRef(_) | Event::CData(_) if active_field.is_some() => {
                return Err(refuse("encoded compiler setting is not supported"));
            }
            Event::End(tag) => {
                if let Some((name, value)) = active_field.take() {
                    let value = value.trim();
                    match name.as_str() {
                        "RuntimeLibrary" => {
                            runtime_count += 1;
                            if value != "MultiThreadedDLL" || runtime_count != 1 {
                                return Err(refuse(
                                    "Release must declare one dynamic CRT MultiThreadedDLL; static CRT or debug CRT refused",
                                ));
                            }
                        }
                        "EnableEnhancedInstructionSet" if value != "NotSet" => {
                            return Err(refuse("Release specifies a non-baseline ISA"));
                        }
                        "AdditionalOptions"
                            // Only CMake's inherited placeholder is admitted. Any
                            // extra switches need an explicit contract review.
                            if !value.is_empty() && value != "%(AdditionalOptions)" => {
                                return Err(refuse(
                                    "unreviewed Release AdditionalOptions override",
                                ));
                        }
                        _ => {}
                    }
                }
                if tag.name().as_ref() == b"ItemDefinitionGroup" {
                    group = None;
                }
                stack
                    .pop()
                    .ok_or_else(|| refuse("unbalanced vcxproj elements"))?;
            }
            Event::Decl(declaration) => {
                if declaration
                    .version()
                    .map_err(|_| refuse("invalid XML declaration"))?
                    .as_ref()
                    != b"1.0"
                {
                    return Err(refuse("unsupported XML version"));
                }
            }
            Event::DocType(_) => return Err(refuse("vcxproj DTD is not supported")),
            Event::Eof => break,
            _ => {}
        }
    }
    if root_count != 1 || !stack.is_empty() || runtime_count != 1 {
        return Err(refuse(
            "vcxproj lacks one complete Release/x64 dynamic CRT declaration",
        ));
    }
    Ok(())
}

pub fn assemble_receipt_draft(
    source: SourceIdentity,
    evidence: RfdetrWindowsBuildEvidence,
    builder: BuilderIdentity,
    outputs: Vec<OutputIdentityEntry>,
) -> Result<ControlledBuildReceiptDraft, RfdetrWindowsBuildEvidenceError> {
    evidence.validate_against_source(&source)?;
    verify_rfdetr_output(&outputs)?;
    let evidence_digest = evidence.digest()?;
    Ok(ControlledBuildReceiptDraft {
        source: Some(source),
        inputs: Some(vec![
            evidence.rf_bundle,
            evidence.ggml_bundle,
            evidence.cmake_archive,
        ]),
        builder: Some(builder),
        configuration: Some(rfdetr_windows_build_configuration()),
        outputs: Some(outputs),
        supporting: Some(vec![SupportingArtifactRef {
            label: RFDETR_WINDOWS_BUILD_EVIDENCE_LABEL.to_owned(),
            sha256: evidence_digest,
        }]),
        ..ControlledBuildReceiptDraft::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pe::{FixtureSpec, ImportSpec, PeSymbolSpec, fixture, parse_pe};
    use crate::provenance::Provenance;

    fn pinned_source() -> SourceIdentity {
        SourceIdentity {
            product: Provenance {
                commit: "journal-product".to_owned(),
                lock_sha256: "0".repeat(64),
            },
            windows_dependency: DependencySource {
                repository: RF_DETR_REPOSITORY.into(),
                revision: RF_DETR_COMMIT.to_owned(),
                content_sha256: "a".repeat(64),
            },
        }
    }

    fn dummy_builder() -> BuilderIdentity {
        BuilderIdentity {
            host: "builder-1".into(),
            toolchain: "msvc".into(),
        }
    }

    fn valid_evidence(source: &SourceIdentity) -> RfdetrWindowsBuildEvidence {
        RfdetrWindowsBuildEvidence {
            schema: RFDETR_WINDOWS_BUILD_EVIDENCE_SCHEMA_V1.to_owned(),
            rf_bundle: InputIdentityEntry {
                label: RFDETR_WINDOWS_RF_BUNDLE_LABEL.to_owned(),
                sha256: source.windows_dependency.content_sha256.clone(),
                size: 100,
            },
            ggml_bundle: InputIdentityEntry {
                label: RFDETR_WINDOWS_GGML_BUNDLE_LABEL.to_owned(),
                sha256: "b".repeat(64),
                size: 200,
            },
            ggml: DependencySource {
                repository: GGML_REPOSITORY.to_owned(),
                revision: GGML_COMMIT.to_owned(),
                content_sha256: "b".repeat(64),
            },
            cmake_archive: InputIdentityEntry {
                label: RFDETR_WINDOWS_CMAKE_ARCHIVE_LABEL.to_owned(),
                sha256: "c".repeat(64),
                size: 300,
            },
            cmake_cache: InputIdentityEntry {
                label: RFDETR_WINDOWS_CMAKE_CACHE_LABEL.to_owned(),
                sha256: "d".repeat(64),
                size: 400,
            },
            build_option_evidence: InputIdentityEntry {
                label: RFDETR_WINDOWS_VCXPROJ_LABEL.to_owned(),
                sha256: "e".repeat(64),
                size: 500,
            },
            subprocesses: ["cmake-configure", "cmake-build"]
                .into_iter()
                .map(|label| RfdetrWindowsSubprocessRecord {
                    label: label.to_owned(),
                    argv: vec!["cmake".to_owned()],
                    cwd: "C:\\build".to_owned(),
                    exit_code: 0,
                    stdout: InputIdentityEntry {
                        label: format!("logs/{label}.stdout"),
                        sha256: "1".repeat(64),
                        size: 10,
                    },
                    stderr: InputIdentityEntry {
                        label: format!("logs/{label}.stderr"),
                        sha256: "2".repeat(64),
                        size: 0,
                    },
                })
                .collect(),
        }
    }

    fn valid_cli_exe_census() -> PeInfo {
        let bytes = fixture(&FixtureSpec {
            dll: false,
            imports: &[ImportSpec {
                name: "kernel32.dll",
                symbols: &[PeSymbolSpec::Named("ExitProcess")],
            }],
            ..FixtureSpec::default()
        });
        parse_pe(&bytes).expect("parse valid pe")
    }

    #[test]
    fn test_receipt_builds_v1_with_rfdetr_windows_dependency_and_sidecar() {
        let source = pinned_source();
        let evidence = valid_evidence(&source);
        let builder = dummy_builder();
        let pe_info = valid_cli_exe_census();
        let outputs = vec![OutputIdentityEntry {
            label: RFDETR_CLI_OUTPUT_LABEL.to_owned(),
            pre_signing_sha256: "f".repeat(64),
            size: 1024,
            census: pe_info,
        }];

        let mut draft = assemble_receipt_draft(source.clone(), evidence.clone(), builder, outputs)
            .expect("draft assemble");
        assert_eq!(
            draft.configuration,
            Some(rfdetr_windows_build_configuration())
        );
        draft.schema = Some(crate::controlled_build::CONTROLLED_BUILD_RECEIPT_SCHEMA_V1.to_owned());
        draft.validation = Some(crate::controlled_build::ValidationReference {
            description: "RF-DETR validation".to_owned(),
            sha256: "0".repeat(64),
        });
        let receipt = draft.validate().expect("receipt validate");
        assert_eq!(receipt.source.windows_dependency.revision, RF_DETR_COMMIT);
        assert_eq!(receipt.inputs.len(), 3);
        assert_eq!(receipt.supporting.len(), 1);
        assert_eq!(
            receipt.supporting[0].label,
            RFDETR_WINDOWS_BUILD_EVIDENCE_LABEL
        );
    }

    #[test]
    fn test_receipt_refuses_wrong_rf_commit_or_ggml_commit() {
        let mut source = pinned_source();
        source.windows_dependency.revision = "deadbeef".repeat(5);
        let evidence = valid_evidence(&source);
        let err = evidence.validate_against_source(&source).unwrap_err();
        assert!(err.to_string().contains("source identity"));

        let source = pinned_source();
        let mut evidence = valid_evidence(&source);
        evidence.ggml.revision = "wrong-ggml-commit".to_owned();
        let err = evidence.validate().unwrap_err();
        assert!(err.to_string().contains("source identity"));
    }

    #[test]
    fn test_cache_parser_accepts_admitted_flags_and_llamafile_on() {
        let cache = r#"
# CMakeCache.txt
RFDETR_SHARED:BOOL=OFF
RFDETR_BUILD_CLI:BOOL=ON
RFDETR_BUILD_TESTS:BOOL=OFF
RFDETR_BUILD_EXAMPLES:BOOL=OFF
RFDETR_GGML_CUDA:BOOL=OFF
RFDETR_GGML_METAL:BOOL=OFF
RFDETR_GGML_VULKAN:BOOL=OFF
RFDETR_GGML_HIPBLAS:BOOL=OFF
BUILD_SHARED_LIBS:BOOL=OFF
GGML_NATIVE:BOOL=OFF
GGML_AVX:BOOL=OFF
GGML_AVX2:BOOL=OFF
GGML_AVX512:BOOL=OFF
GGML_FMA:BOOL=OFF
GGML_F16C:BOOL=OFF
GGML_SSE42:BOOL=OFF
GGML_BMI2:BOOL=OFF
GGML_OPENCL:BOOL=OFF
GGML_BACKEND_DL:BOOL=OFF
GGML_LLAMAFILE:BOOL=ON
CMAKE_BUILD_TYPE:STRING=Release
GGML_OPENMP:STRING=ON
GGML_CPU:STRING=ON
GGML_CPU_ALL_VARIANTS:STRING=OFF
GGML_CCACHE:STRING=OFF
GGML_AVX512_BF16:STRING=OFF
GGML_AVX512_VBMI:STRING=OFF
GGML_AVX512_VNNI:STRING=OFF
GGML_AVX_VNNI:STRING=OFF
GGML_BLAS:STRING=OFF
GGML_CUDA:STRING=OFF
GGML_METAL:STRING=OFF
GGML_VULKAN:STRING=OFF
GGML_HIP:STRING=OFF
GGML_SYCL:STRING=OFF
GGML_RPC:STRING=OFF
GGML_OPENVINO:STRING=OFF
GGML_HEXAGON:STRING=OFF
GGML_MUSA:STRING=OFF
GGML_VIRTGPU:STRING=OFF
GGML_VIRTGPU_BACKEND:STRING=OFF
GGML_WEBGPU:STRING=OFF
GGML_ZDNN:STRING=OFF
GGML_ZENDNN:STRING=OFF
CMAKE_MSVC_RUNTIME_LIBRARY:STRING=MultiThreadedDLL
CMAKE_CXX_FLAGS_RELEASE:STRING=/MD /O2 /Ob2 /DNDEBUG
"#;
        validate_cmake_cache(cache.as_bytes()).expect("cache should be valid");
    }

    #[test]
    fn test_cache_parser_refuses_avx2_and_dynamic_backend_overrides() {
        let cache = r#"
RFDETR_SHARED:BOOL=OFF
RFDETR_BUILD_CLI:BOOL=ON
RFDETR_BUILD_TESTS:BOOL=OFF
RFDETR_BUILD_EXAMPLES:BOOL=OFF
RFDETR_GGML_CUDA:BOOL=OFF
RFDETR_GGML_METAL:BOOL=OFF
RFDETR_GGML_VULKAN:BOOL=OFF
RFDETR_GGML_HIPBLAS:BOOL=OFF
BUILD_SHARED_LIBS:BOOL=OFF
GGML_NATIVE:BOOL=OFF
GGML_AVX:BOOL=OFF
GGML_AVX2:BOOL=ON
GGML_AVX512:BOOL=OFF
GGML_FMA:BOOL=OFF
GGML_F16C:BOOL=OFF
GGML_SSE42:BOOL=OFF
GGML_BMI2:BOOL=OFF
GGML_OPENCL:BOOL=OFF
GGML_BACKEND_DL:BOOL=OFF
GGML_LLAMAFILE:BOOL=ON
CMAKE_BUILD_TYPE:STRING=Release
GGML_OPENMP:STRING=ON
GGML_CPU:STRING=ON
GGML_CPU_ALL_VARIANTS:STRING=OFF
GGML_CCACHE:STRING=OFF
GGML_AVX512_BF16:STRING=OFF
GGML_AVX512_VBMI:STRING=OFF
GGML_AVX512_VNNI:STRING=OFF
GGML_AVX_VNNI:STRING=OFF
GGML_BLAS:STRING=OFF
GGML_CUDA:STRING=OFF
GGML_METAL:STRING=OFF
GGML_VULKAN:STRING=OFF
GGML_HIP:STRING=OFF
GGML_SYCL:STRING=OFF
GGML_RPC:STRING=OFF
GGML_OPENVINO:STRING=OFF
GGML_HEXAGON:STRING=OFF
GGML_MUSA:STRING=OFF
GGML_VIRTGPU:STRING=OFF
GGML_VIRTGPU_BACKEND:STRING=OFF
GGML_WEBGPU:STRING=OFF
GGML_ZDNN:STRING=OFF
GGML_ZENDNN:STRING=OFF
"#;
        let err = validate_cmake_cache(cache.as_bytes()).unwrap_err();
        assert!(err.to_string().contains("GGML_AVX2"));
    }

    #[test]
    fn test_build_option_evidence_proves_multithreaded_dll_and_refuses_mt() {
        let valid_vcxproj = r#"
<Project>
<ItemDefinitionGroup Condition="'$(Configuration)|$(Platform)'=='Release|x64'">
  <ClCompile>
    <RuntimeLibrary>MultiThreadedDLL</RuntimeLibrary>
    <EnableEnhancedInstructionSet>NotSet</EnableEnhancedInstructionSet>
  </ClCompile>
</ItemDefinitionGroup>
</Project>
"#;
        validate_build_option_evidence(valid_vcxproj.as_bytes()).expect("vcxproj is valid");

        let invalid_mt_vcxproj = r#"
<Project>
<ItemDefinitionGroup Condition="'$(Configuration)|$(Platform)'=='Release|x64'">
  <ClCompile>
    <RuntimeLibrary>MultiThreaded</RuntimeLibrary>
  </ClCompile>
</ItemDefinitionGroup>
</Project>
"#;
        let err = validate_build_option_evidence(invalid_mt_vcxproj.as_bytes()).unwrap_err();
        assert!(err.to_string().contains("static CRT"));

        let invalid_avx_vcxproj = r#"
<Project>
<ItemDefinitionGroup Condition="'$(Configuration)|$(Platform)'=='Release|x64'">
  <ClCompile>
    <RuntimeLibrary>MultiThreadedDLL</RuntimeLibrary>
    <EnableEnhancedInstructionSet>AdvancedVectorExtensions2</EnableEnhancedInstructionSet>
  </ClCompile>
</ItemDefinitionGroup>
</Project>
"#;
        let err = validate_build_option_evidence(invalid_avx_vcxproj.as_bytes()).unwrap_err();
        assert!(err.to_string().contains("non-baseline ISA"));
    }

    #[test]
    fn project_settings_cannot_be_satisfied_by_comments_or_other_configurations() {
        let valid = r#"<Project><ItemDefinitionGroup Condition="'$(Configuration)|$(Platform)'=='Release|x64'"><ClCompile><RuntimeLibrary>MultiThreadedDLL</RuntimeLibrary><EnableEnhancedInstructionSet>NotSet</EnableEnhancedInstructionSet></ClCompile></ItemDefinitionGroup></Project>"#;
        validate_build_option_evidence(valid.as_bytes()).unwrap();
        for invalid in [
            valid.replace("MultiThreadedDLL", "MultiThreadedDebugDLL"),
            valid.replace("Release|x64", "Debug|x64"),
            valid.replace("<RuntimeLibrary>MultiThreadedDLL</RuntimeLibrary>", "<!-- <RuntimeLibrary>MultiThreadedDLL</RuntimeLibrary> -->"),
            valid.replace("<RuntimeLibrary>", "<RuntimeLibrary Condition=\"false\">"),
            valid.replace("MultiThreadedDLL", "MultiThreadedD&#76;L"),
            valid.replace("</ClCompile>", "<AdditionalOptions>/MDd %(AdditionalOptions)</AdditionalOptions></ClCompile>"),
            valid.replace("</ClCompile>", "<RuntimeLibrary>MultiThreadedDLL</RuntimeLibrary></ClCompile>"),
            valid.replace("</Project>", "<ItemGroup><ClCompile Include=\"override.cpp\"><RuntimeLibrary>MultiThreaded</RuntimeLibrary></ClCompile></ItemGroup></Project>"),
            format!("{valid}{valid}"),
            valid.replace("</Project>", ""),
        ] {
            assert!(validate_build_option_evidence(invalid.as_bytes()).is_err(), "admitted {invalid}");
        }
        // Nonselected configurations are expected in a multi-config project.
        let debug = valid
            .replace("Release|x64", "Debug|x64")
            .replace("MultiThreadedDLL", "MultiThreadedDebugDLL");
        let joined = valid.replace("</Project>", &debug.replace("<Project>", ""));
        validate_build_option_evidence(joined.as_bytes()).unwrap();
    }

    #[test]
    fn test_pe_census_rejects_forbidden_accelerator_imports() {
        let bytes = fixture(&FixtureSpec {
            dll: false,
            imports: &[
                ImportSpec {
                    name: "kernel32.dll",
                    symbols: &[PeSymbolSpec::Named("ExitProcess")],
                },
                ImportSpec {
                    name: "nvcuda.dll",
                    symbols: &[PeSymbolSpec::Named("cuInit")],
                },
            ],
            ..FixtureSpec::default()
        });
        let pe_info = parse_pe(&bytes).expect("parse pe");
        let err = verify_cpu_only_imports(&pe_info).unwrap_err();
        assert!(err.to_string().contains("nvcuda.dll"));
    }
}
