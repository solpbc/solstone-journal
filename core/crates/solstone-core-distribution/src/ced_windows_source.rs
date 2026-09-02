// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Deterministic, source-bound inputs and receipt recording for the Windows
//! CED build slot.
//!
//! The driver materializes this archive from clean Git objects before anything
//! reaches Windows. The slot therefore never clones, initializes a submodule,
//! or downloads a build tool while it is network-denied.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::Command;

use flate2::read::GzDecoder;
use tar::Archive;

use crate::acquire;
use crate::artifact_verify::{
    ControlledBuildArtifactVerificationLimits, verify_persisted_controlled_build_artifacts,
};
use crate::ced_windows::{
    CED_CPP_COMMIT, CED_CPP_REPOSITORY, CED_GGML_REPOSITORY, CED_WINDOWS_BUILD_EVIDENCE_LABEL,
    CED_WINDOWS_BUILD_EVIDENCE_SCHEMA_V1, CED_WINDOWS_EXPORT_DEFINITION, GGML_COMMIT,
};
use crate::controlled_build::{
    BuilderIdentity, ControlledBuildReceipt, ControlledBuildReceiptPublication, DependencySource,
    InputIdentityEntry, SourceIdentity, SupportingArtifactRef, ValidationReference, census_outputs,
    write_controlled_build_receipt_exclusive,
};
use crate::digest::sha256_hex;
use crate::provenance::Provenance;

pub const CED_WINDOWS_SOURCE_ARCHIVE_SCHEMA_V1: &str = "solstone.ced-windows-source-archive.v1";
pub const CED_WINDOWS_SOURCE_ARCHIVE_MANIFEST_LABEL: &str = "ced-windows-source.json";

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CedWindowsSourceArchiveManifest {
    pub schema: String,
    pub ced: DependencySource,
    pub ggml: DependencySource,
    pub export_definition_sha256: String,
    pub files: Vec<CedWindowsSourceArchiveFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CedWindowsSourceArchiveFile {
    pub path: String,
    pub mode: u32,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CedWindowsSourceArchive {
    pub archive: InputIdentityEntry,
    pub ced: DependencySource,
    pub ggml: DependencySource,
    pub export_definition_sha256: String,
}

/// Inputs to the post-build CED recorder. Every path is expected to name a
/// byte sequence already created by the network-denied slot; this function
/// only verifies and persists those facts and never invokes a compiler.
pub struct CedWindowsBuildRecordArgs<'a> {
    pub repo_root: &'a Path,
    pub source_archive: &'a Path,
    pub cmake_archive: &'a Path,
    pub cmake_cache: &'a Path,
    pub output_root: &'a Path,
    pub evidence_path: &'a Path,
    pub receipt_path: &'a Path,
    pub validation_path: &'a Path,
    pub product: Provenance,
    pub builder: BuilderIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CedWindowsBuildRecord {
    pub receipt: ControlledBuildReceipt,
    pub receipt_publication: CedWindowsReceiptPublication,
    pub evidence_sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CedWindowsReceiptPublication {
    Durable,
    PublishedButNotDurable,
}

impl CedWindowsReceiptPublication {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Durable => "durable",
            Self::PublishedButNotDurable => "published-but-not-durable",
        }
    }
}

#[derive(Debug)]
pub struct CedWindowsSourceArchiveError {
    message: String,
}

impl CedWindowsSourceArchiveError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for CedWindowsSourceArchiveError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CedWindowsSourceArchiveError {}

impl From<io::Error> for CedWindowsSourceArchiveError {
    fn from(source: io::Error) -> Self {
        Self::new(source.to_string())
    }
}

pub fn usage() -> &'static str {
    "usage: solstone-distribution ced-windows <source-archive|verify-inputs|record|verify> [FLAG]"
}

/// Execute the deliberately small CED controlled-build command surface.
///
/// `source-archive` runs only on the driver, while `record` and `verify` run
/// in the native slot after CMake has already produced an output. None of the
/// three commands downloads, configures, or builds CED itself.
pub fn run_cli(args: &[String]) -> Result<String, CedWindowsSourceArchiveError> {
    let Some((operation, rest)) = args.split_first() else {
        return Err(CedWindowsSourceArchiveError::new(usage()));
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
            let archive = materialize_ced_windows_source_archive(&source, &output)?;
            Ok(format!(
                "CED_WINDOWS_SOURCE_ARCHIVE_OK path={} sha256={} size={}",
                output.display(),
                archive.archive.sha256,
                archive.archive.size
            ))
        }
        "verify-inputs" => {
            let repo_root = repository_root()?;
            let source_archive = required_path(&flags, "--source-archive")?;
            let cmake_archive = required_path(&flags, "--cmake-archive")?;
            require_only(&flags, &["--source-archive", "--cmake-archive"])?;
            let source = inspect_ced_windows_source_archive(&source_archive)?;
            let (cmake_version, cmake) = inspect_cmake_windows_archive(&repo_root, &cmake_archive)?;
            Ok(format!(
                "CED_WINDOWS_INPUTS_OK source_sha256={} source_size={} cmake_version={} cmake_sha256={} cmake_size={}",
                source.archive.sha256, source.archive.size, cmake_version, cmake.sha256, cmake.size
            ))
        }
        "record" => {
            let repo_root = repository_root()?;
            let source_archive = required_path(&flags, "--source-archive")?;
            let cmake_archive = required_path(&flags, "--cmake-archive")?;
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
            let record = record_ced_windows_build(CedWindowsBuildRecordArgs {
                repo_root: &repo_root,
                source_archive: &source_archive,
                cmake_archive: &cmake_archive,
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
                .expect("CED record creates exactly one output");
            Ok(format!(
                "CED_WINDOWS_RECORD_OK receipt={} evidence_sha256={} output_sha256={} receipt_publication={}",
                receipt_path.display(),
                record.evidence_sha256,
                output.pre_signing_sha256,
                record.receipt_publication.as_str()
            ))
        }
        "verify" => {
            let receipt_path = required_path(&flags, "--receipt")?;
            let output_root = required_path(&flags, "--output-root")?;
            require_only(&flags, &["--receipt", "--output-root"])?;
            let verified = verify_persisted_controlled_build_artifacts(
                &receipt_path,
                &output_root,
                ControlledBuildArtifactVerificationLimits::new(1, 2, 128 * 1024 * 1024),
            )
            .map_err(|source| CedWindowsSourceArchiveError::new(source.to_string()))?;
            let output = verified
                .receipt()
                .outputs
                .first()
                .expect("verified CED receipt has one output");
            Ok(format!(
                "CED_WINDOWS_ARTIFACT_VERIFY_OK receipt={} output_sha256={}",
                receipt_path.display(),
                output.pre_signing_sha256
            ))
        }
        other => Err(CedWindowsSourceArchiveError::new(format!(
            "unknown CED Windows command {other:?}\n{}",
            usage()
        ))),
    }
}

fn parse_flags(args: &[String]) -> Result<BTreeMap<String, String>, CedWindowsSourceArchiveError> {
    if !args.len().is_multiple_of(2) {
        return Err(CedWindowsSourceArchiveError::new(
            "CED Windows command flags must be name/value pairs",
        ));
    }
    let mut flags = BTreeMap::new();
    for pair in args.chunks_exact(2) {
        let name = &pair[0];
        if !name.starts_with("--") {
            return Err(CedWindowsSourceArchiveError::new(format!(
                "invalid CED Windows flag {name:?}"
            )));
        }
        if flags.insert(name.clone(), pair[1].clone()).is_some() {
            return Err(CedWindowsSourceArchiveError::new(format!(
                "duplicate CED Windows flag {name}"
            )));
        }
    }
    Ok(flags)
}

fn required_text(
    flags: &BTreeMap<String, String>,
    name: &str,
) -> Result<String, CedWindowsSourceArchiveError> {
    let value = flags
        .get(name)
        .ok_or_else(|| CedWindowsSourceArchiveError::new(format!("missing required {name}")))?;
    if value.is_empty() {
        return Err(CedWindowsSourceArchiveError::new(format!(
            "required CED Windows flag {name} is empty"
        )));
    }
    Ok(value.clone())
}

fn required_path(
    flags: &BTreeMap<String, String>,
    name: &str,
) -> Result<PathBuf, CedWindowsSourceArchiveError> {
    Ok(PathBuf::from(required_text(flags, name)?))
}

fn required_lower_hex(
    flags: &BTreeMap<String, String>,
    name: &str,
    length: usize,
) -> Result<String, CedWindowsSourceArchiveError> {
    let value = required_text(flags, name)?;
    if value.len() != length
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(CedWindowsSourceArchiveError::new(format!(
            "required CED Windows flag {name} must be {length} lowercase hexadecimal characters"
        )));
    }
    Ok(value)
}

fn require_only(
    flags: &BTreeMap<String, String>,
    admitted: &[&str],
) -> Result<(), CedWindowsSourceArchiveError> {
    if let Some(unexpected) = flags.keys().find(|name| !admitted.contains(&name.as_str())) {
        return Err(CedWindowsSourceArchiveError::new(format!(
            "unknown CED Windows flag {unexpected}"
        )));
    }
    Ok(())
}

fn repository_root() -> Result<PathBuf, CedWindowsSourceArchiveError> {
    let mut cursor = std::env::current_dir()?;
    loop {
        if cursor
            .join("core/distribution/builder-inputs.toml")
            .is_file()
        {
            return Ok(cursor);
        }
        cursor = cursor.parent().map(Path::to_path_buf).ok_or_else(|| {
            CedWindowsSourceArchiveError::new(
                "could not find core/distribution/builder-inputs.toml",
            )
        })?;
    }
}

/// Materialize a fresh, deterministic CED archive from exact clean Git object
/// bytes. Existing destination files are refused: a slot must never inherit an
/// unexamined archive from a prior build.
pub fn materialize_ced_windows_source_archive(
    source_root: &Path,
    destination: &Path,
) -> Result<CedWindowsSourceArchive, CedWindowsSourceArchiveError> {
    if destination.exists() {
        return Err(CedWindowsSourceArchiveError::new(format!(
            "refusing to replace existing CED source archive {}",
            destination.display()
        )));
    }
    verify_source_checkout(source_root)?;

    let ggml_root = source_root.join("third_party/ggml");
    let mut ced_files = git_files(source_root, "")?;
    let raw_ced_digest = digest_files(&ced_files);
    let cmake = ced_files
        .iter_mut()
        .find(|file| file.path == "CMakeLists.txt")
        .ok_or_else(|| CedWindowsSourceArchiveError::new("CED source lacks CMakeLists.txt"))?;
    cmake.bytes = overlay_cmake_lists(&cmake.bytes)?;

    let mut source_files = ced_files;
    source_files.extend(git_files(&ggml_root, "third_party/ggml/")?);
    source_files.push(SourceFile {
        path: "ced.exports.def".to_owned(),
        mode: 0o644,
        bytes: CED_WINDOWS_EXPORT_DEFINITION.as_bytes().to_vec(),
    });
    source_files.sort_by(|left, right| left.path.cmp(&right.path));
    ensure_distinct_files(&source_files)?;

    let manifest = CedWindowsSourceArchiveManifest {
        schema: CED_WINDOWS_SOURCE_ARCHIVE_SCHEMA_V1.to_owned(),
        ced: DependencySource {
            repository: CED_CPP_REPOSITORY.to_owned(),
            revision: CED_CPP_COMMIT.to_owned(),
            content_sha256: raw_ced_digest,
        },
        ggml: DependencySource {
            repository: CED_GGML_REPOSITORY.to_owned(),
            revision: GGML_COMMIT.to_owned(),
            content_sha256: digest_files(
                &source_files
                    .iter()
                    .filter(|file| file.path.starts_with("third_party/ggml/"))
                    .cloned()
                    .collect::<Vec<_>>(),
            ),
        },
        export_definition_sha256: sha256_hex(CED_WINDOWS_EXPORT_DEFINITION.as_bytes()),
        files: source_files
            .iter()
            .map(SourceFile::record)
            .collect::<Vec<_>>(),
    };
    let manifest_bytes = serde_json::to_vec(&manifest)
        .map_err(|source| CedWindowsSourceArchiveError::new(source.to_string()))?;

    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    write_archive(destination, &source_files, &manifest_bytes)?;
    inspect_ced_windows_source_archive(destination)
}

/// Parse and verify the source archive at a boundary that has no Git checkout.
pub fn inspect_ced_windows_source_archive(
    path: &Path,
) -> Result<CedWindowsSourceArchive, CedWindowsSourceArchiveError> {
    let bytes = fs::read(path)?;
    let manifest = validate_archive_bytes(&bytes)?;
    Ok(CedWindowsSourceArchive {
        archive: InputIdentityEntry {
            label: crate::ced_windows::CED_WINDOWS_SOURCE_ARCHIVE_LABEL.to_owned(),
            sha256: sha256_hex(&bytes),
            size: bytes.len() as u64,
        },
        ced: manifest.ced,
        ggml: manifest.ggml,
        export_definition_sha256: manifest.export_definition_sha256,
    })
}

/// Require the exact CMake archive pin declared beside the driver acquisition
/// command, then report it in the CED receipt's input vocabulary.
pub fn inspect_cmake_windows_archive(
    repo_root: &Path,
    path: &Path,
) -> Result<(String, InputIdentityEntry), CedWindowsSourceArchiveError> {
    let expected = acquire::windows_cmake_archive_input(repo_root)
        .map_err(|source| CedWindowsSourceArchiveError::new(source.to_string()))?;
    let bytes = fs::read(path)?;
    let actual = sha256_hex(&bytes);
    if bytes.len() as u64 != expected.size || actual != expected.sha256 {
        return Err(CedWindowsSourceArchiveError::new(format!(
            "CMake Windows archive identity mismatch\n  expected: {} {} bytes {}\n  found: {} {} bytes {}",
            expected.filename,
            expected.size,
            expected.sha256,
            path.display(),
            bytes.len(),
            actual
        )));
    }
    Ok((
        expected.version,
        InputIdentityEntry {
            label: crate::ced_windows::CED_WINDOWS_CMAKE_ARCHIVE_LABEL.to_owned(),
            sha256: actual,
            size: bytes.len() as u64,
        },
    ))
}

/// Build a receipt from an already-produced DLL, bind its evidence sidecar,
/// and publish both names exclusively. A Windows receipt is deliberately
/// returned as `published-but-not-durable` rather than being inflated into a
/// signing claim; the later artifact verifier must rehash this exact output.
pub fn record_ced_windows_build(
    args: CedWindowsBuildRecordArgs<'_>,
) -> Result<CedWindowsBuildRecord, CedWindowsSourceArchiveError> {
    let source = inspect_ced_windows_source_archive(args.source_archive)?;
    let (_, cmake_archive) = inspect_cmake_windows_archive(args.repo_root, args.cmake_archive)?;
    let cmake_cache = fs::read(args.cmake_cache)?;
    if cmake_cache.is_empty() {
        return Err(CedWindowsSourceArchiveError::new(
            "CED CMake cache is empty",
        ));
    }
    let output_path = args.output_root.join("bin/ced.dll");
    let output = fs::read(&output_path)?;
    let outputs = census_outputs(&[("bin/ced.dll", output.as_slice())])
        .map_err(|source| CedWindowsSourceArchiveError::new(source.to_string()))?;
    let evidence = crate::ced_windows::CedWindowsBuildEvidence {
        schema: CED_WINDOWS_BUILD_EVIDENCE_SCHEMA_V1.to_owned(),
        source_archive: source.archive.clone(),
        cmake_archive,
        ggml: source.ggml,
        export_definition_sha256: source.export_definition_sha256,
        cmake_cache_sha256: sha256_hex(&cmake_cache),
    };
    let evidence_bytes = crate::ced_windows::encode_ced_windows_build_evidence(&evidence)
        .map_err(|source| CedWindowsSourceArchiveError::new(source.to_string()))?;
    let evidence_sha256 = sha256_hex(&evidence_bytes);
    publish_evidence_exclusive(args.evidence_path, &evidence_bytes)?;
    let validation = fs::read(args.validation_path)?;
    if validation.is_empty() {
        return Err(CedWindowsSourceArchiveError::new(
            "CED validation reference is empty",
        ));
    }
    let source_identity = SourceIdentity {
        product: args.product,
        windows_dependency: DependencySource {
            repository: source.ced.repository,
            revision: source.ced.revision,
            content_sha256: source.archive.sha256,
        },
    };
    let mut draft = crate::ced_windows::assemble_receipt_draft(
        source_identity,
        evidence,
        args.builder,
        outputs,
    )
    .map_err(|source| CedWindowsSourceArchiveError::new(source.to_string()))?;
    draft.schema = Some(crate::controlled_build::CONTROLLED_BUILD_RECEIPT_SCHEMA_V1.to_owned());
    draft.validation = Some(ValidationReference {
        description: "CED Windows network-denied CMake build and PE admission log".to_owned(),
        sha256: sha256_hex(&validation),
    });
    let receipt = draft
        .validate()
        .map_err(|source| CedWindowsSourceArchiveError::new(source.to_string()))?;
    let publication = write_controlled_build_receipt_exclusive(args.receipt_path, &receipt)
        .map_err(|source| CedWindowsSourceArchiveError::new(source.to_string()))?;
    let receipt_publication = match publication {
        ControlledBuildReceiptPublication::Durable { .. } => CedWindowsReceiptPublication::Durable,
        ControlledBuildReceiptPublication::PublishedButNotDurable { .. } => {
            CedWindowsReceiptPublication::PublishedButNotDurable
        }
        ControlledBuildReceiptPublication::PublicationUnconfirmed { publication, .. } => {
            return Err(CedWindowsSourceArchiveError::new(format!(
                "CED receipt publication is unconfirmed: {publication:?}"
            )));
        }
    };
    if receipt.supporting.as_slice()
        != [SupportingArtifactRef {
            label: CED_WINDOWS_BUILD_EVIDENCE_LABEL.to_owned(),
            sha256: evidence_sha256.clone(),
        }]
    {
        return Err(CedWindowsSourceArchiveError::new(
            "CED receipt does not bind its generated evidence sidecar",
        ));
    }
    Ok(CedWindowsBuildRecord {
        receipt,
        receipt_publication,
        evidence_sha256,
    })
}

fn publish_evidence_exclusive(
    path: &Path,
    bytes: &[u8],
) -> Result<(), CedWindowsSourceArchiveError> {
    let publication = solstone_core_journal_io::write_bytes_exclusive_detailed(
        path,
        bytes,
        solstone_core_journal_io::AtomicWriteOptions::default(),
    )
    .map_err(|source| CedWindowsSourceArchiveError::new(source.to_string()))?;
    if matches!(
        publication.final_name,
        solstone_core_journal_io::FinalNameConfirmation::Confirmed { .. }
    ) && matches!(
        publication.cleanup,
        solstone_core_journal_io::StageCleanup::Removed
    ) {
        Ok(())
    } else {
        Err(CedWindowsSourceArchiveError::new(format!(
            "CED evidence publication is unconfirmed: {publication:?}"
        )))
    }
}

fn verify_source_checkout(source_root: &Path) -> Result<(), CedWindowsSourceArchiveError> {
    verify_git_checkout(source_root, CED_CPP_REPOSITORY, CED_CPP_COMMIT)?;
    let submodule = source_root.join("third_party/ggml");
    verify_git_checkout(&submodule, CED_GGML_REPOSITORY, GGML_COMMIT)?;
    let gitlink = git_output(source_root, &["ls-tree", "HEAD", "third_party/ggml"])?;
    let expected = format!("160000 commit {GGML_COMMIT}\tthird_party/ggml\n");
    if String::from_utf8_lossy(&gitlink) != expected {
        return Err(CedWindowsSourceArchiveError::new(format!(
            "CED source has an unexpected ggml gitlink\n  expected: {}\n  found: {}",
            expected.trim_end(),
            String::from_utf8_lossy(&gitlink).trim_end()
        )));
    }
    Ok(())
}

fn verify_git_checkout(
    root: &Path,
    expected_repository: &str,
    expected_commit: &str,
) -> Result<(), CedWindowsSourceArchiveError> {
    let status = git_output(
        root,
        &[
            "status",
            "--porcelain=v1",
            "--untracked-files=all",
            "--ignore-submodules=none",
        ],
    )?;
    if !status.is_empty() {
        return Err(CedWindowsSourceArchiveError::new(format!(
            "source checkout is dirty: {}",
            root.display()
        )));
    }
    let commit = git_text(root, &["rev-parse", "HEAD"])?;
    if commit != expected_commit {
        return Err(CedWindowsSourceArchiveError::new(format!(
            "source checkout has unexpected commit\n  expected: {expected_commit}\n  found: {commit}"
        )));
    }
    let repository = git_text(root, &["remote", "get-url", "origin"])?;
    if repository != expected_repository {
        return Err(CedWindowsSourceArchiveError::new(format!(
            "source checkout has unexpected origin\n  expected: {expected_repository}\n  found: {repository}"
        )));
    }
    Ok(())
}

fn git_files(root: &Path, prefix: &str) -> Result<Vec<SourceFile>, CedWindowsSourceArchiveError> {
    let listed = git_output(root, &["ls-tree", "-rz", "HEAD"])?;
    let mut files = Vec::new();
    for record in listed
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
    {
        let tab = record
            .iter()
            .position(|byte| *byte == b'\t')
            .ok_or_else(|| {
                CedWindowsSourceArchiveError::new("Git ls-tree record has no tab separator")
            })?;
        let (metadata, path) = record.split_at(tab);
        let path = &path[1..];
        let mut fields = metadata
            .split(|byte| *byte == b' ')
            .filter(|field| !field.is_empty());
        let mode = fields
            .next()
            .ok_or_else(|| CedWindowsSourceArchiveError::new("Git ls-tree record has no mode"))?;
        let kind = fields
            .next()
            .ok_or_else(|| CedWindowsSourceArchiveError::new("Git ls-tree record has no kind"))?;
        let mode = std::str::from_utf8(mode)
            .map_err(|source| CedWindowsSourceArchiveError::new(source.to_string()))?;
        let kind = std::str::from_utf8(kind)
            .map_err(|source| CedWindowsSourceArchiveError::new(source.to_string()))?;
        let path = std::str::from_utf8(path)
            .map_err(|source| CedWindowsSourceArchiveError::new(source.to_string()))?;
        if mode == "160000" && kind == "commit" && path == "third_party/ggml" && prefix.is_empty() {
            continue;
        }
        if mode == "160000" || kind != "blob" {
            return Err(CedWindowsSourceArchiveError::new(format!(
                "unsupported non-file Git entry {mode} {kind} {path}"
            )));
        }
        crate::archive::refuse_escape(path)
            .map_err(|source| CedWindowsSourceArchiveError::new(source.as_str()))?;
        let source_mode = u32::from_str_radix(mode, 8)
            .map_err(|source| CedWindowsSourceArchiveError::new(source.to_string()))?;
        if !matches!(source_mode, 0o100644 | 0o100755) {
            return Err(CedWindowsSourceArchiveError::new(format!(
                "unsupported Git file mode {mode} for {path}"
            )));
        }
        let spec = format!("HEAD:{path}");
        let bytes = git_output(root, &["show", &spec])?;
        files.push(SourceFile {
            path: format!("{prefix}{path}"),
            mode: source_mode & 0o777,
            bytes,
        });
    }
    Ok(files)
}

fn git_text(root: &Path, args: &[&str]) -> Result<String, CedWindowsSourceArchiveError> {
    let value = git_output(root, args)?;
    let text = std::str::from_utf8(&value)
        .map_err(|source| CedWindowsSourceArchiveError::new(source.to_string()))?;
    Ok(text.trim_end_matches(['\r', '\n']).to_owned())
}

fn git_output(root: &Path, args: &[&str]) -> Result<Vec<u8>, CedWindowsSourceArchiveError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|source| CedWindowsSourceArchiveError::new(source.to_string()))?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(CedWindowsSourceArchiveError::new(format!(
            "git -C {} {} failed: {}",
            root.display(),
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim_end()
        )))
    }
}

fn overlay_cmake_lists(bytes: &[u8]) -> Result<Vec<u8>, CedWindowsSourceArchiveError> {
    let original = std::str::from_utf8(bytes)
        .map_err(|source| CedWindowsSourceArchiveError::new(source.to_string()))?;
    const BROAD_EXPORT: &str =
        "  set_target_properties(ced PROPERTIES WINDOWS_EXPORT_ALL_SYMBOLS ON)";
    const REVIEWED_EXPORT: &str = "  set_target_properties(ced PROPERTIES WINDOWS_EXPORT_ALL_SYMBOLS OFF)\n  target_sources(ced PRIVATE ${CMAKE_CURRENT_SOURCE_DIR}/ced.exports.def)";
    if original.matches(BROAD_EXPORT).count() != 1 {
        return Err(CedWindowsSourceArchiveError::new(
            "CED CMake source no longer has exactly one broad Windows export setting",
        ));
    }
    Ok(original.replace(BROAD_EXPORT, REVIEWED_EXPORT).into_bytes())
}

#[derive(Debug, Clone)]
struct SourceFile {
    path: String,
    mode: u32,
    bytes: Vec<u8>,
}

impl SourceFile {
    fn record(&self) -> CedWindowsSourceArchiveFile {
        CedWindowsSourceArchiveFile {
            path: self.path.clone(),
            mode: self.mode,
            sha256: sha256_hex(&self.bytes),
        }
    }
}

fn ensure_distinct_files(files: &[SourceFile]) -> Result<(), CedWindowsSourceArchiveError> {
    let paths = files
        .iter()
        .map(|file| file.path.as_str())
        .collect::<BTreeSet<_>>();
    if paths.len() != files.len() {
        return Err(CedWindowsSourceArchiveError::new(
            "CED source archive has duplicate member paths",
        ));
    }
    Ok(())
}

fn digest_files(files: &[SourceFile]) -> String {
    let records = files.iter().map(SourceFile::record).collect::<Vec<_>>();
    sha256_hex(&serde_json::to_vec(&records).expect("source records serialize"))
}

fn write_archive(
    destination: &Path,
    files: &[SourceFile],
    manifest: &[u8],
) -> Result<(), CedWindowsSourceArchiveError> {
    let file = fs::File::create(destination)?;
    let encoder = crate::tar::deterministic_gzip(file);
    let mut builder = tar::Builder::new(encoder);
    let mut all = BTreeMap::new();
    for source in files {
        all.insert(source.path.as_str(), (source.mode, source.bytes.as_slice()));
    }
    all.insert(CED_WINDOWS_SOURCE_ARCHIVE_MANIFEST_LABEL, (0o644, manifest));
    for (path, (mode, bytes)) in all {
        crate::tar::append_regular(&mut builder, path, bytes, mode)?;
    }
    builder.finish()?;
    builder.into_inner()?.finish()?;
    Ok(())
}

fn validate_archive_bytes(
    bytes: &[u8],
) -> Result<CedWindowsSourceArchiveManifest, CedWindowsSourceArchiveError> {
    let mut archive = Archive::new(GzDecoder::new(bytes));
    let mut members = BTreeMap::new();
    for entry in archive.entries()? {
        let mut entry = entry?;
        if !entry.header().entry_type().is_file() {
            return Err(CedWindowsSourceArchiveError::new(
                "CED source archive contains a non-file member",
            ));
        }
        let path = entry.path()?.to_string_lossy().replace('\\', "/");
        crate::archive::refuse_escape(&path)
            .map_err(|source| CedWindowsSourceArchiveError::new(source.as_str()))?;
        let mode = entry.header().mode()? & 0o777;
        let mut contents = Vec::new();
        entry.read_to_end(&mut contents)?;
        if members.insert(path.clone(), (mode, contents)).is_some() {
            return Err(CedWindowsSourceArchiveError::new(format!(
                "CED source archive contains duplicate member {path}"
            )));
        }
    }
    let (_, manifest_bytes) = members
        .remove(CED_WINDOWS_SOURCE_ARCHIVE_MANIFEST_LABEL)
        .ok_or_else(|| CedWindowsSourceArchiveError::new("CED source archive lacks manifest"))?;
    let manifest = serde_json::from_slice::<CedWindowsSourceArchiveManifest>(&manifest_bytes)
        .map_err(|source| CedWindowsSourceArchiveError::new(source.to_string()))?;
    validate_manifest(&manifest, &members)?;
    Ok(manifest)
}

fn validate_manifest(
    manifest: &CedWindowsSourceArchiveManifest,
    members: &BTreeMap<String, (u32, Vec<u8>)>,
) -> Result<(), CedWindowsSourceArchiveError> {
    if manifest.schema != CED_WINDOWS_SOURCE_ARCHIVE_SCHEMA_V1 {
        return Err(CedWindowsSourceArchiveError::new(format!(
            "unexpected CED source archive schema {}",
            manifest.schema
        )));
    }
    require_dependency(&manifest.ced, CED_CPP_REPOSITORY, CED_CPP_COMMIT, "ced.cpp")?;
    require_dependency(&manifest.ggml, CED_GGML_REPOSITORY, GGML_COMMIT, "ggml")?;
    if manifest.export_definition_sha256 != sha256_hex(CED_WINDOWS_EXPORT_DEFINITION.as_bytes()) {
        return Err(CedWindowsSourceArchiveError::new(
            "CED source archive has an unexpected export-definition digest",
        ));
    }
    let expected = manifest
        .files
        .iter()
        .map(|file| (file.path.clone(), (file.mode, file.sha256.clone())))
        .collect::<BTreeMap<_, _>>();
    if expected.len() != manifest.files.len() {
        return Err(CedWindowsSourceArchiveError::new(
            "CED source archive manifest has duplicate file paths",
        ));
    }
    let actual = members
        .iter()
        .map(|(path, (mode, bytes))| (path.clone(), (*mode, sha256_hex(bytes))))
        .collect::<BTreeMap<_, _>>();
    if actual != expected {
        return Err(CedWindowsSourceArchiveError::new(
            "CED source archive members differ from the manifest",
        ));
    }
    let definition = members.get("ced.exports.def").ok_or_else(|| {
        CedWindowsSourceArchiveError::new("CED source archive lacks export definition")
    })?;
    if definition.1.as_slice() != CED_WINDOWS_EXPORT_DEFINITION.as_bytes() {
        return Err(CedWindowsSourceArchiveError::new(
            "CED source archive has an unexpected export definition",
        ));
    }
    let cmake = members.get("CMakeLists.txt").ok_or_else(|| {
        CedWindowsSourceArchiveError::new("CED source archive lacks CMakeLists.txt")
    })?;
    let cmake = std::str::from_utf8(&cmake.1)
        .map_err(|source| CedWindowsSourceArchiveError::new(source.to_string()))?;
    if !cmake.contains("WINDOWS_EXPORT_ALL_SYMBOLS OFF")
        || !cmake
            .contains("target_sources(ced PRIVATE ${CMAKE_CURRENT_SOURCE_DIR}/ced.exports.def)")
    {
        return Err(CedWindowsSourceArchiveError::new(
            "CED source archive lacks the reviewed Windows export overlay",
        ));
    }
    Ok(())
}

fn require_dependency(
    dependency: &DependencySource,
    repository: &str,
    revision: &str,
    label: &str,
) -> Result<(), CedWindowsSourceArchiveError> {
    if dependency.repository != repository || dependency.revision != revision {
        return Err(CedWindowsSourceArchiveError::new(format!(
            "CED source archive has unexpected {label} identity"
        )));
    }
    if dependency.content_sha256.len() != 64
        || !dependency
            .content_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(CedWindowsSourceArchiveError::new(format!(
            "CED source archive has invalid {label} content SHA-256"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_archive() -> Vec<u8> {
        let files = vec![
            SourceFile {
                path: "CMakeLists.txt".into(),
                mode: 0o644,
                bytes: b"set_target_properties(ced PROPERTIES WINDOWS_EXPORT_ALL_SYMBOLS OFF)\ntarget_sources(ced PRIVATE ${CMAKE_CURRENT_SOURCE_DIR}/ced.exports.def)\n".to_vec(),
            },
            SourceFile {
                path: "ced.exports.def".into(),
                mode: 0o644,
                bytes: CED_WINDOWS_EXPORT_DEFINITION.as_bytes().to_vec(),
            },
            SourceFile {
                path: "third_party/ggml/CMakeLists.txt".into(),
                mode: 0o644,
                bytes: b"project(ggml)\n".to_vec(),
            },
        ];
        let manifest = CedWindowsSourceArchiveManifest {
            schema: CED_WINDOWS_SOURCE_ARCHIVE_SCHEMA_V1.into(),
            ced: DependencySource {
                repository: CED_CPP_REPOSITORY.into(),
                revision: CED_CPP_COMMIT.into(),
                content_sha256: "a".repeat(64),
            },
            ggml: DependencySource {
                repository: CED_GGML_REPOSITORY.into(),
                revision: GGML_COMMIT.into(),
                content_sha256: "b".repeat(64),
            },
            export_definition_sha256: sha256_hex(CED_WINDOWS_EXPORT_DEFINITION.as_bytes()),
            files: files.iter().map(SourceFile::record).collect(),
        };
        let manifest = serde_json::to_vec(&manifest).expect("manifest serializes");
        let temporary = tempfile::NamedTempFile::new().expect("temporary archive");
        write_archive(temporary.path(), &files, &manifest).expect("write archive");
        fs::read(temporary.path()).expect("read archive")
    }

    #[test]
    fn source_archive_is_deterministic_and_strictly_inspectable() {
        let first = fixture_archive();
        let second = fixture_archive();
        assert_eq!(first, second);
        let manifest = validate_archive_bytes(&first).expect("fixture archive validates");
        assert_eq!(manifest.ced.revision, CED_CPP_COMMIT);
        assert_eq!(manifest.ggml.revision, GGML_COMMIT);
    }

    #[test]
    fn source_archive_refuses_an_unreviewed_export_definition() {
        let mut files = vec![
            SourceFile {
                path: "CMakeLists.txt".into(),
                mode: 0o644,
                bytes: b"set_target_properties(ced PROPERTIES WINDOWS_EXPORT_ALL_SYMBOLS OFF)\ntarget_sources(ced PRIVATE ${CMAKE_CURRENT_SOURCE_DIR}/ced.exports.def)\n".to_vec(),
            },
            SourceFile {
                path: "ced.exports.def".into(),
                mode: 0o644,
                bytes: b"LIBRARY ced\nEXPORTS\n    ced_capi_unreviewed\n".to_vec(),
            },
        ];
        files.sort_by(|left, right| left.path.cmp(&right.path));
        let manifest = CedWindowsSourceArchiveManifest {
            schema: CED_WINDOWS_SOURCE_ARCHIVE_SCHEMA_V1.into(),
            ced: DependencySource {
                repository: CED_CPP_REPOSITORY.into(),
                revision: CED_CPP_COMMIT.into(),
                content_sha256: "a".repeat(64),
            },
            ggml: DependencySource {
                repository: CED_GGML_REPOSITORY.into(),
                revision: GGML_COMMIT.into(),
                content_sha256: "b".repeat(64),
            },
            export_definition_sha256: sha256_hex(CED_WINDOWS_EXPORT_DEFINITION.as_bytes()),
            files: files.iter().map(SourceFile::record).collect(),
        };
        let manifest = serde_json::to_vec(&manifest).expect("manifest serializes");
        let temporary = tempfile::NamedTempFile::new().expect("temporary archive");
        write_archive(temporary.path(), &files, &manifest).expect("write archive");
        let bytes = fs::read(temporary.path()).expect("read archive");
        let error = validate_archive_bytes(&bytes).expect_err("unreviewed export refuses");
        assert!(error.to_string().contains("unexpected export definition"));
    }

    #[test]
    fn overlay_replaces_only_the_pinned_broad_export_line() {
        let source = b"if(CED_SHARED)\n  set_target_properties(ced PROPERTIES WINDOWS_EXPORT_ALL_SYMBOLS ON)\nendif()\n";
        let overlay =
            String::from_utf8(overlay_cmake_lists(source).expect("overlay")).expect("utf8 overlay");
        assert!(overlay.contains("WINDOWS_EXPORT_ALL_SYMBOLS OFF"));
        assert!(overlay.contains("ced.exports.def"));
        assert!(!overlay.contains("WINDOWS_EXPORT_ALL_SYMBOLS ON"));
    }
}
