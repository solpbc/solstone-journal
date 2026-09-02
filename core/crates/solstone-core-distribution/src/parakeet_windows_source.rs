// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Deterministic inputs and pre-signing receipt evidence for the controlled
//! Windows `parakeet.cpp` build slot.
//!
//! The driver archives a checked-out, already-applied upstream patch series and
//! verifies the bundled model before transfer. The network-denied slot only
//! extracts those inputs, builds the server, copies the verified model, and
//! records their identities. This does not start the provider or supply the
//! independent-provider facade required for a Windows capability claim.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::Command;

use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};
use tar::Archive;

use crate::artifact_verify::{
    ControlledBuildArtifactVerificationLimits, verify_persisted_controlled_build_artifacts,
};
use crate::ced_windows_source::inspect_cmake_windows_archive;
use crate::controlled_build::{
    BuildConfiguration, BuilderIdentity, ControlledBuildReceipt, ControlledBuildReceiptPublication,
    DependencySource, InputIdentityEntry, SourceIdentity, SupportingArtifactRef,
    ValidationReference, census_outputs, write_controlled_build_receipt_exclusive,
};
use crate::digest::sha256_hex;
use crate::parakeet_windows::{
    GGML_COMMIT, PARAKEET_CPP_COMMIT, PARAKEET_MODEL_OUTPUT_LABEL, PARAKEET_MODEL_SHA256,
    PARAKEET_MODEL_SIZE_BYTES, PARAKEET_SERVER_OUTPUT_LABEL,
};
use crate::provenance::Provenance;

const PARAKEET_CPP_REPOSITORY: &str = "https://github.com/mudler/parakeet.cpp.git";
const PARAKEET_GGML_REPOSITORY: &str = "https://github.com/ggml-org/ggml";
const PARAKEET_WINDOWS_SOURCE_ARCHIVE_SCHEMA_V1: &str =
    "solstone.parakeet-windows-source-archive.v1";
const PARAKEET_WINDOWS_BUILD_EVIDENCE_SCHEMA_V1: &str =
    "solstone.parakeet-windows-build-evidence.v1";
const PARAKEET_WINDOWS_SOURCE_ARCHIVE_MANIFEST: &str = "parakeet-windows-source.json";
const PARAKEET_WINDOWS_SOURCE_ARCHIVE_LABEL: &str = "sources/parakeet.cpp-patched-source.tar.gz";
const PARAKEET_WINDOWS_CMAKE_ARCHIVE_LABEL: &str = "tools/cmake-windows-x86_64.zip";
const PARAKEET_WINDOWS_MODEL_INPUT_LABEL: &str = "models/tdt-0.6b-v3-q8_0.gguf";
const PARAKEET_WINDOWS_BUILD_EVIDENCE_LABEL: &str =
    "provenance/windows-x86_64/parakeet-build-evidence.json";
// The receipt names one PE server, while the retained Windows root observes
// both output directories and the copied model as well. Keep this bound tied
// to the intentionally small tree rather than allowing receipt verification
// to scan an arbitrary build output.
const PARAKEET_WINDOWS_OUTPUT_TREE_ENTRIES: usize = 4;
const PARAKEET_WINDOWS_OUTPUT_TREE_MAXIMUM_BYTES: usize =
    PARAKEET_MODEL_SIZE_BYTES as usize + 128 * 1024 * 1024;
const PARAKEET_WINDOWS_GGML_PATCH_DIFF_SHA256: &str =
    "e62f5e880cde081d478927b62f304f60c93e92a8996b4e82f2e3b6a9205e9926";
const PARAKEET_WINDOWS_SERVER_PATCH_NAME: &str = "0005-controlled-local-server-boundary.patch";
const PARAKEET_WINDOWS_SERVER_PATCH_PATH: &str =
    "core/distribution/parakeet-windows-patches/0005-controlled-local-server-boundary.patch";
const PARAKEET_WINDOWS_SERVER_PATCH_SHA256: &str =
    "b2314ce9019d0d7020678c07b623adf3419e420f3fe5df0c4e7baa143a1d0412";
const PARAKEET_WINDOWS_SERVER_PATCH_SIZE: u64 = 12_337;
const PARAKEET_WINDOWS_PATCHES: &[(&str, &str)] = &[
    (
        "0001-ggml-cpu-fold-broadcast-iterations-in-llamafile_sgem.patch",
        "779bb7c37d38c7a007e9f6e874db7040fafe7e9d93d4152efcbdae8fa560963b",
    ),
    (
        "0002-metal-conv-2d-dw.patch",
        "55bad8241fd355fbecb63516a5778dd92b916fdd97b6e780ff2979b74f4a8fe3",
    ),
    (
        "0003-metal-pad-leading.patch",
        "61e7e7be0a2b22afa8aa05afb1d90d89e75f67a2ecdc0fa7d8f51bd705002609",
    ),
    (
        "0004-cuda-pad-grid-stride.patch",
        "6a57d875c16a0f9aee42f88b9f07ba44fdf05718f54009697299495d8fb02424",
    ),
];

#[derive(Debug)]
pub struct ParakeetWindowsSourceError {
    message: String,
}

impl ParakeetWindowsSourceError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ParakeetWindowsSourceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ParakeetWindowsSourceError {}

impl From<io::Error> for ParakeetWindowsSourceError {
    fn from(source: io::Error) -> Self {
        Self::new(source.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParakeetWindowsSourceArchiveManifest {
    pub schema: String,
    pub parakeet: DependencySource,
    pub ggml: DependencySource,
    pub patches: Vec<InputIdentityEntry>,
    pub files: Vec<ParakeetWindowsSourceArchiveFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParakeetWindowsSourceArchiveFile {
    pub path: String,
    pub mode: u32,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParakeetWindowsSourceArchive {
    pub archive: InputIdentityEntry,
    pub parakeet: DependencySource,
    pub ggml: DependencySource,
    pub patches: Vec<InputIdentityEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParakeetWindowsBuildEvidence {
    pub schema: String,
    pub source_archive: InputIdentityEntry,
    pub cmake_archive: InputIdentityEntry,
    pub model: InputIdentityEntry,
    pub cmake_cache_sha256: String,
}

/// Inputs to the post-build recorder. It verifies already-produced bytes; it
/// never invokes CMake, the server, or a dynamic loader.
pub struct ParakeetWindowsBuildRecordArgs<'a> {
    pub repo_root: &'a Path,
    pub source_archive: &'a Path,
    pub cmake_archive: &'a Path,
    pub model_path: &'a Path,
    pub cmake_cache: &'a Path,
    pub output_root: &'a Path,
    pub evidence_path: &'a Path,
    pub receipt_path: &'a Path,
    pub validation_path: &'a Path,
    pub product: Provenance,
    pub builder: BuilderIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParakeetWindowsBuildRecord {
    pub receipt: ControlledBuildReceipt,
    pub receipt_publication: ParakeetWindowsReceiptPublication,
    pub evidence_sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParakeetWindowsReceiptPublication {
    Durable,
    PublishedButNotDurable,
}

impl ParakeetWindowsReceiptPublication {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Durable => "durable",
            Self::PublishedButNotDurable => "published-but-not-durable",
        }
    }
}

#[derive(Debug, Clone)]
struct SourceFile {
    path: String,
    mode: u32,
    bytes: Vec<u8>,
}

impl SourceFile {
    fn record(&self) -> ParakeetWindowsSourceArchiveFile {
        ParakeetWindowsSourceArchiveFile {
            path: self.path.clone(),
            mode: self.mode,
            sha256: sha256_hex(&self.bytes),
        }
    }
}

pub fn usage() -> &'static str {
    "usage: solstone-distribution parakeet-windows <source-archive|verify-inputs|record|verify|help> [FLAG]"
}

pub fn run_cli(args: &[String]) -> Result<String, ParakeetWindowsSourceError> {
    let Some((operation, rest)) = args.split_first() else {
        return Err(ParakeetWindowsSourceError::new(usage()));
    };
    let flags = parse_flags(rest)?;
    match operation.as_str() {
        "help" | "--help" | "-h" => {
            require_only(&flags, &[])?;
            Ok(usage().to_owned())
        }
        "source-archive" => {
            let source = required_path(&flags, "--source")?;
            let output = required_path(&flags, "--out")?;
            require_only(&flags, &["--source", "--out"])?;
            let archive = materialize_parakeet_windows_source_archive(&source, &output)?;
            Ok(format!(
                "PARAKEET_WINDOWS_SOURCE_ARCHIVE_OK path={} sha256={} size={}",
                output.display(),
                archive.archive.sha256,
                archive.archive.size
            ))
        }
        "verify-inputs" => {
            let repo_root = repository_root()?;
            let source_archive = required_path(&flags, "--source-archive")?;
            let cmake_archive = required_path(&flags, "--cmake-archive")?;
            let model_path = required_path(&flags, "--model")?;
            require_only(&flags, &["--source-archive", "--cmake-archive", "--model"])?;
            let source = inspect_parakeet_windows_source_archive(&source_archive)?;
            let (_, cmake) = inspect_cmake_windows_archive(&repo_root, &cmake_archive)
                .map_err(|source| ParakeetWindowsSourceError::new(source.to_string()))?;
            let model = inspect_model(&model_path)?;
            Ok(format!(
                "PARAKEET_WINDOWS_INPUTS_OK source_sha256={} source_size={} cmake_sha256={} cmake_size={} model_sha256={} model_size={}",
                source.archive.sha256,
                source.archive.size,
                cmake.sha256,
                cmake.size,
                model.sha256,
                model.size,
            ))
        }
        "record" => {
            let repo_root = repository_root()?;
            let source_archive = required_path(&flags, "--source-archive")?;
            let cmake_archive = required_path(&flags, "--cmake-archive")?;
            let model_path = required_path(&flags, "--model")?;
            let cmake_cache = required_path(&flags, "--cmake-cache")?;
            let output_root = required_path(&flags, "--output-root")?;
            let evidence_path = required_path(&flags, "--evidence")?;
            let receipt_path = required_path(&flags, "--receipt")?;
            let validation_path = required_path(&flags, "--validation")?;
            let product_commit = required_lower_hex(&flags, "--product-commit", 40)?;
            let cargo_lock_sha256 = required_lower_hex(&flags, "--cargo-lock-sha256", 64)?;
            let builder_host = required_text(&flags, "--builder-host")?;
            let toolchain = required_text(&flags, "--toolchain")?;
            require_only(
                &flags,
                &[
                    "--source-archive",
                    "--cmake-archive",
                    "--model",
                    "--cmake-cache",
                    "--output-root",
                    "--evidence",
                    "--receipt",
                    "--validation",
                    "--product-commit",
                    "--cargo-lock-sha256",
                    "--builder-host",
                    "--toolchain",
                ],
            )?;
            let record = record_parakeet_windows_build(ParakeetWindowsBuildRecordArgs {
                repo_root: &repo_root,
                source_archive: &source_archive,
                cmake_archive: &cmake_archive,
                model_path: &model_path,
                cmake_cache: &cmake_cache,
                output_root: &output_root,
                evidence_path: &evidence_path,
                receipt_path: &receipt_path,
                validation_path: &validation_path,
                product: Provenance {
                    commit: product_commit,
                    lock_sha256: cargo_lock_sha256,
                },
                builder: BuilderIdentity {
                    host: builder_host,
                    toolchain,
                },
            })?;
            let output = record
                .receipt
                .outputs
                .first()
                .expect("Parakeet record creates one server output");
            Ok(format!(
                "PARAKEET_WINDOWS_RECORD_OK receipt={} evidence_sha256={} server_sha256={} receipt_publication={}",
                receipt_path.display(),
                record.evidence_sha256,
                output.pre_signing_sha256,
                record.receipt_publication.as_str(),
            ))
        }
        "verify" => {
            let receipt_path = required_path(&flags, "--receipt")?;
            let output_root = required_path(&flags, "--output-root")?;
            require_only(&flags, &["--receipt", "--output-root"])?;
            let verified = verify_persisted_controlled_build_artifacts(
                &receipt_path,
                &output_root,
                ControlledBuildArtifactVerificationLimits::new(
                    PARAKEET_WINDOWS_OUTPUT_TREE_ENTRIES,
                    2,
                    PARAKEET_WINDOWS_OUTPUT_TREE_MAXIMUM_BYTES,
                ),
            )
            .map_err(|source| ParakeetWindowsSourceError::new(source.to_string()))?;
            let model = inspect_model(&output_root.join(PARAKEET_MODEL_OUTPUT_LABEL))?;
            if !verified
                .receipt()
                .inputs
                .iter()
                .any(|input| input == &model)
            {
                return Err(ParakeetWindowsSourceError::new(
                    "Parakeet receipt does not bind the rehashed packaged model",
                ));
            }
            let output = verified
                .receipt()
                .outputs
                .first()
                .expect("verified Parakeet receipt has one server output");
            Ok(format!(
                "PARAKEET_WINDOWS_ARTIFACT_VERIFY_OK receipt={} server_sha256={} model_sha256={}",
                receipt_path.display(),
                output.pre_signing_sha256,
                model.sha256,
            ))
        }
        other => Err(ParakeetWindowsSourceError::new(format!(
            "unknown Parakeet Windows command {other:?}\n{}",
            usage()
        ))),
    }
}

pub fn materialize_parakeet_windows_source_archive(
    source_root: &Path,
    destination: &Path,
) -> Result<ParakeetWindowsSourceArchive, ParakeetWindowsSourceError> {
    if destination.exists() {
        return Err(ParakeetWindowsSourceError::new(format!(
            "refusing to replace existing Parakeet source archive {}",
            destination.display()
        )));
    }
    verify_source_checkout(source_root)?;
    let ggml_root = source_root.join("third_party/ggml");
    let mut source_files = working_tree_files(source_root, "")?;
    source_files.push(SourceFile {
        path: format!("parakeet-windows-patches/{PARAKEET_WINDOWS_SERVER_PATCH_NAME}"),
        mode: 0o644,
        bytes: verified_server_patch_bytes()?,
    });
    source_files.sort_by(|left, right| left.path.cmp(&right.path));
    let parakeet_digest = digest_files(&source_files);
    let ggml_files = working_tree_files(&ggml_root, "third_party/ggml/")?;
    let ggml_digest = digest_files(&ggml_files);
    source_files.extend(ggml_files);
    source_files.sort_by(|left, right| left.path.cmp(&right.path));
    ensure_distinct_files(&source_files)?;
    let patches = verified_patch_inputs(source_root)?;
    let manifest = ParakeetWindowsSourceArchiveManifest {
        schema: PARAKEET_WINDOWS_SOURCE_ARCHIVE_SCHEMA_V1.to_owned(),
        parakeet: DependencySource {
            repository: PARAKEET_CPP_REPOSITORY.to_owned(),
            revision: PARAKEET_CPP_COMMIT.to_owned(),
            content_sha256: parakeet_digest,
        },
        ggml: DependencySource {
            repository: PARAKEET_GGML_REPOSITORY.to_owned(),
            revision: GGML_COMMIT.to_owned(),
            content_sha256: ggml_digest,
        },
        patches,
        files: source_files.iter().map(SourceFile::record).collect(),
    };
    let manifest_bytes = serde_json::to_vec(&manifest)
        .map_err(|source| ParakeetWindowsSourceError::new(source.to_string()))?;
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    write_archive(destination, &source_files, &manifest_bytes)?;
    inspect_parakeet_windows_source_archive(destination)
}

pub fn inspect_parakeet_windows_source_archive(
    path: &Path,
) -> Result<ParakeetWindowsSourceArchive, ParakeetWindowsSourceError> {
    let bytes = fs::read(path)?;
    let manifest = validate_archive_bytes(&bytes)?;
    Ok(ParakeetWindowsSourceArchive {
        archive: InputIdentityEntry {
            label: PARAKEET_WINDOWS_SOURCE_ARCHIVE_LABEL.to_owned(),
            sha256: sha256_hex(&bytes),
            size: bytes.len() as u64,
        },
        parakeet: manifest.parakeet,
        ggml: manifest.ggml,
        patches: manifest.patches,
    })
}

pub fn record_parakeet_windows_build(
    args: ParakeetWindowsBuildRecordArgs<'_>,
) -> Result<ParakeetWindowsBuildRecord, ParakeetWindowsSourceError> {
    let source = inspect_parakeet_windows_source_archive(args.source_archive)?;
    let (_, cmake_archive) = inspect_cmake_windows_archive(args.repo_root, args.cmake_archive)
        .map_err(|source| ParakeetWindowsSourceError::new(source.to_string()))?;
    if cmake_archive.label != PARAKEET_WINDOWS_CMAKE_ARCHIVE_LABEL {
        return Err(ParakeetWindowsSourceError::new(
            "Parakeet Windows CMake archive label differs from the reviewed tool label",
        ));
    }
    let model = inspect_model(args.model_path)?;
    let cmake_cache = fs::read(args.cmake_cache)?;
    if cmake_cache.is_empty() {
        return Err(ParakeetWindowsSourceError::new(
            "Parakeet CMake cache is empty",
        ));
    }
    let server = fs::read(args.output_root.join(PARAKEET_SERVER_OUTPUT_LABEL))?;
    let copied_model = inspect_model(&args.output_root.join(PARAKEET_MODEL_OUTPUT_LABEL))?;
    if copied_model != model {
        return Err(ParakeetWindowsSourceError::new(
            "Parakeet copied model differs from its verified build input",
        ));
    }
    let outputs = census_outputs(&[(PARAKEET_SERVER_OUTPUT_LABEL, server.as_slice())])
        .map_err(|source| ParakeetWindowsSourceError::new(source.to_string()))?;
    crate::parakeet_windows::verify_import_closure(
        &outputs
            .first()
            .expect("Parakeet server census is present")
            .census,
    )
    .map_err(|source| ParakeetWindowsSourceError::new(source.to_string()))?;
    let evidence = ParakeetWindowsBuildEvidence {
        schema: PARAKEET_WINDOWS_BUILD_EVIDENCE_SCHEMA_V1.to_owned(),
        source_archive: source.archive.clone(),
        cmake_archive,
        model: model.clone(),
        cmake_cache_sha256: sha256_hex(&cmake_cache),
    };
    validate_build_evidence(&evidence)?;
    let evidence_bytes = serde_json::to_vec(&evidence)
        .map_err(|source| ParakeetWindowsSourceError::new(source.to_string()))?;
    let evidence_sha256 = sha256_hex(&evidence_bytes);
    publish_evidence_exclusive(args.evidence_path, &evidence_bytes)?;
    let validation = fs::read(args.validation_path)?;
    if validation.is_empty() {
        return Err(ParakeetWindowsSourceError::new(
            "Parakeet validation reference is empty",
        ));
    }
    let receipt = ControlledBuildReceipt {
        schema: crate::controlled_build::CONTROLLED_BUILD_RECEIPT_SCHEMA_V1.to_owned(),
        source: SourceIdentity {
            product: args.product,
            windows_dependency: DependencySource {
                repository: source.parakeet.repository,
                revision: source.parakeet.revision,
                content_sha256: source.archive.sha256,
            },
        },
        inputs: vec![
            evidence.source_archive.clone(),
            evidence.cmake_archive.clone(),
            model,
        ],
        builder: args.builder,
        configuration: BuildConfiguration {
            target_triple: "x86_64-pc-windows-msvc".to_owned(),
            profile: "Release".to_owned(),
            flags: vec![
                "-DPARAKEET_BUILD_TESTS=OFF".to_owned(),
                "-DPARAKEET_BUILD_CLI=OFF".to_owned(),
                "-DPARAKEET_BUILD_SERVER=ON".to_owned(),
                "-DPARAKEET_SHARED=OFF".to_owned(),
                "-DBUILD_SHARED_LIBS=OFF".to_owned(),
                "-DPARAKEET_GGML_CUDA=OFF".to_owned(),
                "-DPARAKEET_GGML_METAL=OFF".to_owned(),
                "-DPARAKEET_GGML_VULKAN=OFF".to_owned(),
                "-DPARAKEET_GGML_HIP=OFF".to_owned(),
                "-DGGML_NATIVE=OFF".to_owned(),
                "-DGGML_LLAMAFILE=OFF".to_owned(),
                "-DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreadedDLL".to_owned(),
            ],
            network_access_denied: true,
        },
        outputs,
        supporting: vec![SupportingArtifactRef {
            label: PARAKEET_WINDOWS_BUILD_EVIDENCE_LABEL.to_owned(),
            sha256: evidence_sha256.clone(),
        }],
        validation: ValidationReference {
            description: "Parakeet Windows network-denied CMake build and PE admission log"
                .to_owned(),
            sha256: sha256_hex(&validation),
        },
    };
    receipt
        .validate()
        .map_err(|source| ParakeetWindowsSourceError::new(source.to_string()))?;
    let receipt_publication =
        match write_controlled_build_receipt_exclusive(args.receipt_path, &receipt)
            .map_err(|source| ParakeetWindowsSourceError::new(source.to_string()))?
        {
            ControlledBuildReceiptPublication::Durable { .. } => {
                ParakeetWindowsReceiptPublication::Durable
            }
            ControlledBuildReceiptPublication::PublishedButNotDurable { .. } => {
                ParakeetWindowsReceiptPublication::PublishedButNotDurable
            }
            ControlledBuildReceiptPublication::PublicationUnconfirmed { publication, .. } => {
                return Err(ParakeetWindowsSourceError::new(format!(
                    "Parakeet receipt publication is unconfirmed: {publication:?}"
                )));
            }
        };
    Ok(ParakeetWindowsBuildRecord {
        receipt,
        receipt_publication,
        evidence_sha256,
    })
}

fn parse_flags(args: &[String]) -> Result<BTreeMap<String, String>, ParakeetWindowsSourceError> {
    if !args.len().is_multiple_of(2) {
        return Err(ParakeetWindowsSourceError::new(
            "Parakeet Windows command flags must be name/value pairs",
        ));
    }
    let mut flags = BTreeMap::new();
    for pair in args.chunks_exact(2) {
        if !pair[0].starts_with("--") {
            return Err(ParakeetWindowsSourceError::new(format!(
                "invalid Parakeet Windows flag {:?}",
                pair[0]
            )));
        }
        if flags.insert(pair[0].clone(), pair[1].clone()).is_some() {
            return Err(ParakeetWindowsSourceError::new(format!(
                "duplicate Parakeet Windows flag {}",
                pair[0]
            )));
        }
    }
    Ok(flags)
}

fn required_text(
    flags: &BTreeMap<String, String>,
    name: &str,
) -> Result<String, ParakeetWindowsSourceError> {
    let value = flags
        .get(name)
        .ok_or_else(|| ParakeetWindowsSourceError::new(format!("missing required {name}")))?;
    if value.is_empty() {
        Err(ParakeetWindowsSourceError::new(format!(
            "required Parakeet Windows flag {name} is empty"
        )))
    } else {
        Ok(value.clone())
    }
}

fn required_path(
    flags: &BTreeMap<String, String>,
    name: &str,
) -> Result<PathBuf, ParakeetWindowsSourceError> {
    Ok(PathBuf::from(required_text(flags, name)?))
}

fn required_lower_hex(
    flags: &BTreeMap<String, String>,
    name: &str,
    length: usize,
) -> Result<String, ParakeetWindowsSourceError> {
    let value = required_text(flags, name)?;
    if value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(value)
    } else {
        Err(ParakeetWindowsSourceError::new(format!(
            "required Parakeet Windows flag {name} must be {length} lowercase hexadecimal characters"
        )))
    }
}

fn require_only(
    flags: &BTreeMap<String, String>,
    admitted: &[&str],
) -> Result<(), ParakeetWindowsSourceError> {
    if let Some(unexpected) = flags.keys().find(|name| !admitted.contains(&name.as_str())) {
        Err(ParakeetWindowsSourceError::new(format!(
            "unknown Parakeet Windows flag {unexpected}"
        )))
    } else {
        Ok(())
    }
}

fn repository_root() -> Result<PathBuf, ParakeetWindowsSourceError> {
    let mut cursor = std::env::current_dir()?;
    loop {
        if cursor
            .join("core/distribution/builder-inputs.toml")
            .is_file()
        {
            return Ok(cursor);
        }
        cursor = cursor.parent().map(Path::to_path_buf).ok_or_else(|| {
            ParakeetWindowsSourceError::new("could not find core/distribution/builder-inputs.toml")
        })?;
    }
}

fn verify_source_checkout(source_root: &Path) -> Result<(), ParakeetWindowsSourceError> {
    verify_git_identity(
        source_root,
        PARAKEET_CPP_REPOSITORY,
        PARAKEET_CPP_COMMIT,
        "parakeet.cpp",
    )?;
    let root_status = git_output(
        source_root,
        &[
            "status",
            "--porcelain=v1",
            "--untracked-files=all",
            "--ignore-submodules=none",
        ],
    )?;
    if root_status != b" M examples/server/main.cpp\n M third_party/ggml\n" {
        return Err(ParakeetWindowsSourceError::new(
            "Parakeet source must contain only the reviewed server patch and dirty ggml submodule",
        ));
    }
    let ggml_root = source_root.join("third_party/ggml");
    verify_git_identity(&ggml_root, PARAKEET_GGML_REPOSITORY, GGML_COMMIT, "ggml")?;
    let gitlink = git_output(source_root, &["ls-tree", "HEAD", "third_party/ggml"])?;
    let expected_gitlink = format!("160000 commit {GGML_COMMIT}\tthird_party/ggml\n");
    if gitlink != expected_gitlink.as_bytes() {
        return Err(ParakeetWindowsSourceError::new(
            "Parakeet source has an unexpected ggml gitlink",
        ));
    }
    let patch_diff = git_output(&ggml_root, &["diff", "--binary", "HEAD"])?;
    if sha256_hex(&patch_diff) != PARAKEET_WINDOWS_GGML_PATCH_DIFF_SHA256 {
        return Err(ParakeetWindowsSourceError::new(
            "ggml working tree does not match the reviewed patch series",
        ));
    }
    verified_patch_inputs(source_root)?;
    Ok(())
}

fn verify_git_identity(
    root: &Path,
    expected_repository: &str,
    expected_commit: &str,
    label: &str,
) -> Result<(), ParakeetWindowsSourceError> {
    let commit = git_text(root, &["rev-parse", "HEAD"])?;
    if commit != expected_commit {
        return Err(ParakeetWindowsSourceError::new(format!(
            "{label} checkout has unexpected commit\n  expected: {expected_commit}\n  found: {commit}"
        )));
    }
    let repository = git_text(root, &["remote", "get-url", "origin"])?;
    if repository != expected_repository {
        return Err(ParakeetWindowsSourceError::new(format!(
            "{label} checkout has unexpected origin\n  expected: {expected_repository}\n  found: {repository}"
        )));
    }
    Ok(())
}

fn verified_patch_inputs(
    source_root: &Path,
) -> Result<Vec<InputIdentityEntry>, ParakeetWindowsSourceError> {
    let mut patches = Vec::with_capacity(PARAKEET_WINDOWS_PATCHES.len() + 1);
    for (name, expected_sha256) in PARAKEET_WINDOWS_PATCHES {
        let path = source_root.join("third_party/ggml-patches").join(name);
        let bytes = fs::read(&path)?;
        let sha256 = sha256_hex(&bytes);
        if sha256 != *expected_sha256 || bytes.is_empty() {
            return Err(ParakeetWindowsSourceError::new(format!(
                "Parakeet ggml patch {name} has an unexpected identity"
            )));
        }
        patches.push(InputIdentityEntry {
            label: format!("patches/{name}"),
            sha256,
            size: bytes.len() as u64,
        });
    }
    let server_patch = verified_server_patch_bytes()?;
    let server_patch_sha256 = sha256_hex(&server_patch);
    let server_diff = git_output(
        source_root,
        &["diff", "--binary", "HEAD", "--", "examples/server/main.cpp"],
    )?;
    if server_diff != server_patch {
        return Err(ParakeetWindowsSourceError::new(
            "Parakeet server working tree does not match the reviewed controlled-server patch",
        ));
    }
    patches.push(InputIdentityEntry {
        label: format!("patches/{PARAKEET_WINDOWS_SERVER_PATCH_NAME}"),
        sha256: server_patch_sha256,
        size: server_patch.len() as u64,
    });
    Ok(patches)
}

fn verified_server_patch_bytes() -> Result<Vec<u8>, ParakeetWindowsSourceError> {
    let server_patch_path = repository_root()?.join(PARAKEET_WINDOWS_SERVER_PATCH_PATH);
    let server_patch = fs::read(&server_patch_path)?;
    if sha256_hex(&server_patch) != PARAKEET_WINDOWS_SERVER_PATCH_SHA256
        || server_patch.len() as u64 != PARAKEET_WINDOWS_SERVER_PATCH_SIZE
    {
        return Err(ParakeetWindowsSourceError::new(
            "Parakeet controlled-server patch has an unexpected identity",
        ));
    }
    Ok(server_patch)
}

fn working_tree_files(
    root: &Path,
    prefix: &str,
) -> Result<Vec<SourceFile>, ParakeetWindowsSourceError> {
    let listed = git_output(root, &["ls-files", "-s", "-z"])?;
    let mut files = Vec::new();
    for record in listed
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
    {
        let tab = record
            .iter()
            .position(|byte| *byte == b'\t')
            .ok_or_else(|| {
                ParakeetWindowsSourceError::new("Git ls-files record has no tab separator")
            })?;
        let metadata = std::str::from_utf8(&record[..tab])
            .map_err(|source| ParakeetWindowsSourceError::new(source.to_string()))?;
        let path = std::str::from_utf8(&record[tab + 1..])
            .map_err(|source| ParakeetWindowsSourceError::new(source.to_string()))?;
        let mode = metadata
            .split_whitespace()
            .next()
            .ok_or_else(|| ParakeetWindowsSourceError::new("Git ls-files record has no mode"))?;
        if mode == "160000" {
            continue;
        }
        if mode != "100644" && mode != "100755" {
            return Err(ParakeetWindowsSourceError::new(format!(
                "Parakeet source has non-regular tracked mode {mode} at {path}"
            )));
        }
        crate::archive::refuse_escape(path)
            .map_err(|source| ParakeetWindowsSourceError::new(source.as_str()))?;
        let source_path = root.join(path);
        let metadata = fs::symlink_metadata(&source_path)?;
        if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
            return Err(ParakeetWindowsSourceError::new(format!(
                "Parakeet source path is not a regular file: {}",
                source_path.display()
            )));
        }
        files.push(SourceFile {
            path: format!("{prefix}{path}"),
            mode: if mode == "100755" { 0o755 } else { 0o644 },
            bytes: fs::read(source_path)?,
        });
    }
    Ok(files)
}

fn inspect_model(path: &Path) -> Result<InputIdentityEntry, ParakeetWindowsSourceError> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(ParakeetWindowsSourceError::new(format!(
            "Parakeet model path is not a regular file: {}",
            path.display()
        )));
    }
    if metadata.len() != PARAKEET_MODEL_SIZE_BYTES {
        return Err(ParakeetWindowsSourceError::new(format!(
            "Parakeet model has unexpected size at {}",
            path.display()
        )));
    }
    let mut hasher = Sha256::new();
    let mut reader = BufReader::new(fs::File::open(path)?);
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let sha256 = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    if sha256 != PARAKEET_MODEL_SHA256 {
        return Err(ParakeetWindowsSourceError::new(format!(
            "Parakeet model has unexpected SHA-256 at {}",
            path.display()
        )));
    }
    Ok(InputIdentityEntry {
        label: PARAKEET_WINDOWS_MODEL_INPUT_LABEL.to_owned(),
        sha256,
        size: metadata.len(),
    })
}

fn write_archive(
    destination: &Path,
    files: &[SourceFile],
    manifest: &[u8],
) -> Result<(), ParakeetWindowsSourceError> {
    if files
        .iter()
        .any(|file| file.path == PARAKEET_WINDOWS_SOURCE_ARCHIVE_MANIFEST)
    {
        return Err(ParakeetWindowsSourceError::new(
            "Parakeet source collides with its archive manifest member",
        ));
    }
    let temporary = destination.with_extension("partial");
    if temporary.exists() {
        return Err(ParakeetWindowsSourceError::new(format!(
            "refusing to replace interrupted Parakeet archive {}",
            temporary.display()
        )));
    }
    let result = (|| {
        let file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        let encoder = crate::tar::deterministic_gzip(file);
        let mut builder = tar::Builder::new(encoder);
        let mut members = BTreeMap::new();
        for source in files {
            members.insert(source.path.as_str(), (source.mode, source.bytes.as_slice()));
        }
        members.insert(PARAKEET_WINDOWS_SOURCE_ARCHIVE_MANIFEST, (0o644, manifest));
        for (path, (mode, bytes)) in members {
            crate::tar::append_regular(&mut builder, path, bytes, mode)?;
        }
        builder.finish()?;
        builder.into_inner()?.finish()?;
        Ok::<(), ParakeetWindowsSourceError>(())
    })();
    match result {
        Ok(()) => match fs::rename(&temporary, destination) {
            Ok(()) => Ok(()),
            Err(error) => {
                let _ = fs::remove_file(&temporary);
                Err(ParakeetWindowsSourceError::new(format!(
                    "could not publish Parakeet source archive {}: {error}",
                    destination.display()
                )))
            }
        },
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            Err(error)
        }
    }
}

fn validate_archive_bytes(
    bytes: &[u8],
) -> Result<ParakeetWindowsSourceArchiveManifest, ParakeetWindowsSourceError> {
    let mut archive = Archive::new(GzDecoder::new(bytes));
    let mut members = BTreeMap::new();
    for entry in archive.entries()? {
        let mut entry = entry?;
        if !entry.header().entry_type().is_file() {
            return Err(ParakeetWindowsSourceError::new(
                "Parakeet source archive contains a non-file member",
            ));
        }
        let path = entry.path()?.to_string_lossy().replace('\\', "/");
        crate::archive::refuse_escape(&path)
            .map_err(|source| ParakeetWindowsSourceError::new(source.as_str()))?;
        let mode = entry.header().mode()? & 0o777;
        let mut contents = Vec::new();
        entry.read_to_end(&mut contents)?;
        if members.insert(path.clone(), (mode, contents)).is_some() {
            return Err(ParakeetWindowsSourceError::new(format!(
                "Parakeet source archive contains duplicate member {path}"
            )));
        }
    }
    let (_, manifest_bytes) = members
        .remove(PARAKEET_WINDOWS_SOURCE_ARCHIVE_MANIFEST)
        .ok_or_else(|| ParakeetWindowsSourceError::new("Parakeet source archive lacks manifest"))?;
    let manifest = serde_json::from_slice::<ParakeetWindowsSourceArchiveManifest>(&manifest_bytes)
        .map_err(|source| ParakeetWindowsSourceError::new(source.to_string()))?;
    validate_manifest(&manifest, &members)?;
    Ok(manifest)
}

fn validate_manifest(
    manifest: &ParakeetWindowsSourceArchiveManifest,
    members: &BTreeMap<String, (u32, Vec<u8>)>,
) -> Result<(), ParakeetWindowsSourceError> {
    if manifest.schema != PARAKEET_WINDOWS_SOURCE_ARCHIVE_SCHEMA_V1 {
        return Err(ParakeetWindowsSourceError::new(
            "Parakeet source archive has an unexpected schema",
        ));
    }
    require_dependency(
        &manifest.parakeet,
        PARAKEET_CPP_REPOSITORY,
        PARAKEET_CPP_COMMIT,
        "parakeet.cpp",
    )?;
    require_dependency(
        &manifest.ggml,
        PARAKEET_GGML_REPOSITORY,
        GGML_COMMIT,
        "ggml",
    )?;
    let mut expected_patches = PARAKEET_WINDOWS_PATCHES
        .iter()
        .map(|(name, sha256)| (format!("patches/{name}"), (*sha256).to_owned()))
        .collect::<BTreeMap<_, _>>();
    expected_patches.insert(
        format!("patches/{PARAKEET_WINDOWS_SERVER_PATCH_NAME}"),
        PARAKEET_WINDOWS_SERVER_PATCH_SHA256.to_owned(),
    );
    let actual_patches = manifest
        .patches
        .iter()
        .map(|patch| (patch.label.clone(), patch.sha256.clone()))
        .collect::<BTreeMap<_, _>>();
    if actual_patches != expected_patches || actual_patches.len() != manifest.patches.len() {
        return Err(ParakeetWindowsSourceError::new(
            "Parakeet source archive has an unexpected patch-set identity",
        ));
    }
    for patch in &manifest.patches {
        if patch.size == 0 {
            return Err(ParakeetWindowsSourceError::new(
                "Parakeet source archive has an empty patch",
            ));
        }
    }
    let expected = manifest
        .files
        .iter()
        .map(|file| (file.path.clone(), (file.mode, file.sha256.clone())))
        .collect::<BTreeMap<_, _>>();
    if expected.len() != manifest.files.len() {
        return Err(ParakeetWindowsSourceError::new(
            "Parakeet source archive manifest has duplicate file paths",
        ));
    }
    let actual = members
        .iter()
        .map(|(path, (mode, bytes))| (path.clone(), (*mode, sha256_hex(bytes))))
        .collect::<BTreeMap<_, _>>();
    if expected != actual {
        return Err(ParakeetWindowsSourceError::new(
            "Parakeet source archive members differ from the manifest",
        ));
    }
    let source_files = members
        .iter()
        .map(|(path, (mode, bytes))| SourceFile {
            path: path.clone(),
            mode: *mode,
            bytes: bytes.clone(),
        })
        .collect::<Vec<_>>();
    let parakeet_files = source_files
        .iter()
        .filter(|file| !file.path.starts_with("third_party/ggml/"))
        .cloned()
        .collect::<Vec<_>>();
    let ggml_files = source_files
        .iter()
        .filter(|file| file.path.starts_with("third_party/ggml/"))
        .cloned()
        .collect::<Vec<_>>();
    if parakeet_files.is_empty()
        || ggml_files.is_empty()
        || digest_files(&parakeet_files) != manifest.parakeet.content_sha256
        || digest_files(&ggml_files) != manifest.ggml.content_sha256
    {
        return Err(ParakeetWindowsSourceError::new(
            "Parakeet source archive content digests do not bind its members",
        ));
    }
    for patch in &manifest.patches {
        let name = patch.label.strip_prefix("patches/").ok_or_else(|| {
            ParakeetWindowsSourceError::new("Parakeet source archive has invalid patch label")
        })?;
        let path = if name == PARAKEET_WINDOWS_SERVER_PATCH_NAME {
            format!("parakeet-windows-patches/{name}")
        } else {
            format!("third_party/ggml-patches/{name}")
        };
        let (_, bytes) = members.get(&path).ok_or_else(|| {
            ParakeetWindowsSourceError::new("Parakeet source archive lacks a reviewed patch")
        })?;
        if sha256_hex(bytes) != patch.sha256 || bytes.len() as u64 != patch.size {
            return Err(ParakeetWindowsSourceError::new(
                "Parakeet source archive patch bytes differ from their manifest identity",
            ));
        }
    }
    Ok(())
}

fn require_dependency(
    dependency: &DependencySource,
    repository: &str,
    revision: &str,
    label: &str,
) -> Result<(), ParakeetWindowsSourceError> {
    if dependency.repository != repository || dependency.revision != revision {
        return Err(ParakeetWindowsSourceError::new(format!(
            "Parakeet source archive has unexpected {label} identity"
        )));
    }
    if dependency.content_sha256.len() != 64
        || !dependency
            .content_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(ParakeetWindowsSourceError::new(format!(
            "Parakeet source archive has invalid {label} content SHA-256"
        )));
    }
    Ok(())
}

fn validate_build_evidence(
    evidence: &ParakeetWindowsBuildEvidence,
) -> Result<(), ParakeetWindowsSourceError> {
    if evidence.schema != PARAKEET_WINDOWS_BUILD_EVIDENCE_SCHEMA_V1 {
        return Err(ParakeetWindowsSourceError::new(
            "Parakeet build evidence has an unexpected schema",
        ));
    }
    for (label, input) in [
        (
            PARAKEET_WINDOWS_SOURCE_ARCHIVE_LABEL,
            &evidence.source_archive,
        ),
        (
            PARAKEET_WINDOWS_CMAKE_ARCHIVE_LABEL,
            &evidence.cmake_archive,
        ),
        (PARAKEET_WINDOWS_MODEL_INPUT_LABEL, &evidence.model),
    ] {
        if input.label != label || input.size == 0 || input.sha256.len() != 64 {
            return Err(ParakeetWindowsSourceError::new(format!(
                "Parakeet build evidence has invalid {label} identity"
            )));
        }
    }
    if evidence.model.size != PARAKEET_MODEL_SIZE_BYTES
        || evidence.model.sha256 != PARAKEET_MODEL_SHA256
    {
        return Err(ParakeetWindowsSourceError::new(
            "Parakeet build evidence has an invalid model identity",
        ));
    }
    if evidence.cmake_cache_sha256.len() != 64
        || !evidence
            .cmake_cache_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(ParakeetWindowsSourceError::new(
            "Parakeet build evidence has an invalid CMake cache SHA-256",
        ));
    }
    Ok(())
}

fn publish_evidence_exclusive(path: &Path, bytes: &[u8]) -> Result<(), ParakeetWindowsSourceError> {
    let publication = solstone_core_journal_io::write_bytes_exclusive_detailed(
        path,
        bytes,
        solstone_core_journal_io::AtomicWriteOptions::default(),
    )
    .map_err(|source| ParakeetWindowsSourceError::new(source.to_string()))?;
    if matches!(
        publication.final_name,
        solstone_core_journal_io::FinalNameConfirmation::Confirmed { .. }
    ) && matches!(
        publication.cleanup,
        solstone_core_journal_io::StageCleanup::Removed
    ) {
        Ok(())
    } else {
        Err(ParakeetWindowsSourceError::new(format!(
            "Parakeet build evidence publication is unconfirmed: {publication:?}"
        )))
    }
}

fn ensure_distinct_files(files: &[SourceFile]) -> Result<(), ParakeetWindowsSourceError> {
    if files
        .iter()
        .map(|file| file.path.as_str())
        .collect::<BTreeSet<_>>()
        .len()
        != files.len()
    {
        Err(ParakeetWindowsSourceError::new(
            "Parakeet source archive has duplicate member paths",
        ))
    } else {
        Ok(())
    }
}

fn digest_files(files: &[SourceFile]) -> String {
    let records = files.iter().map(SourceFile::record).collect::<Vec<_>>();
    sha256_hex(&serde_json::to_vec(&records).expect("source archive records serialize"))
}

fn git_text(root: &Path, args: &[&str]) -> Result<String, ParakeetWindowsSourceError> {
    let value = git_output(root, args)?;
    let text = std::str::from_utf8(&value)
        .map_err(|source| ParakeetWindowsSourceError::new(source.to_string()))?;
    Ok(text.trim_end_matches(['\r', '\n']).to_owned())
}

fn git_output(root: &Path, args: &[&str]) -> Result<Vec<u8>, ParakeetWindowsSourceError> {
    let output = Command::new("git")
        // The source boundary hashes `git diff --binary`; its index headers are
        // otherwise affected by the caller's core.abbrev configuration.
        .arg("-c")
        .arg("core.abbrev=8")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|source| ParakeetWindowsSourceError::new(source.to_string()))?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(ParakeetWindowsSourceError::new(format!(
            "git -C {} {} failed: {}",
            root.display(),
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim_end()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(label: &str, sha256: &str, size: u64) -> InputIdentityEntry {
        InputIdentityEntry {
            label: label.to_owned(),
            sha256: sha256.to_owned(),
            size,
        }
    }

    #[test]
    fn source_manifest_refuses_an_unreviewed_patch_set() {
        let manifest = ParakeetWindowsSourceArchiveManifest {
            schema: PARAKEET_WINDOWS_SOURCE_ARCHIVE_SCHEMA_V1.to_owned(),
            parakeet: DependencySource {
                repository: PARAKEET_CPP_REPOSITORY.to_owned(),
                revision: PARAKEET_CPP_COMMIT.to_owned(),
                content_sha256: "0".repeat(64),
            },
            ggml: DependencySource {
                repository: PARAKEET_GGML_REPOSITORY.to_owned(),
                revision: GGML_COMMIT.to_owned(),
                content_sha256: "1".repeat(64),
            },
            patches: vec![input("patches/unreviewed.patch", &"2".repeat(64), 1)],
            files: vec![],
        };
        let error = validate_manifest(&manifest, &BTreeMap::new())
            .expect_err("unreviewed patch set must fail closed");
        assert!(error.to_string().contains("patch-set identity"));
    }

    #[test]
    fn source_manifest_requires_the_controlled_server_patch() {
        let manifest = ParakeetWindowsSourceArchiveManifest {
            schema: PARAKEET_WINDOWS_SOURCE_ARCHIVE_SCHEMA_V1.to_owned(),
            parakeet: DependencySource {
                repository: PARAKEET_CPP_REPOSITORY.to_owned(),
                revision: PARAKEET_CPP_COMMIT.to_owned(),
                content_sha256: "0".repeat(64),
            },
            ggml: DependencySource {
                repository: PARAKEET_GGML_REPOSITORY.to_owned(),
                revision: GGML_COMMIT.to_owned(),
                content_sha256: "1".repeat(64),
            },
            patches: PARAKEET_WINDOWS_PATCHES
                .iter()
                .map(|(name, sha256)| input(&format!("patches/{name}"), sha256, 1))
                .collect(),
            files: vec![],
        };
        let error = validate_manifest(&manifest, &BTreeMap::new())
            .expect_err("missing controlled server patch must fail closed");
        assert!(error.to_string().contains("patch-set identity"));
    }

    #[test]
    fn build_evidence_requires_the_pinned_model_identity() {
        let evidence = ParakeetWindowsBuildEvidence {
            schema: PARAKEET_WINDOWS_BUILD_EVIDENCE_SCHEMA_V1.to_owned(),
            source_archive: input(PARAKEET_WINDOWS_SOURCE_ARCHIVE_LABEL, &"0".repeat(64), 1),
            cmake_archive: input(PARAKEET_WINDOWS_CMAKE_ARCHIVE_LABEL, &"1".repeat(64), 1),
            model: input(PARAKEET_WINDOWS_MODEL_INPUT_LABEL, &"2".repeat(64), 1),
            cmake_cache_sha256: "3".repeat(64),
        };
        let error =
            validate_build_evidence(&evidence).expect_err("wrong model identity must fail closed");
        assert!(error.to_string().contains("model identity"));
    }
}
