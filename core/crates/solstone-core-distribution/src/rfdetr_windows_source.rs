// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Deterministic, source-bound bundle verification, recorder, and assembly-facing
//! validator for the Windows RF-DETR build slot.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::artifact_verify::{
    ControlledBuildArtifactVerificationLimits, VerifiedControlledBuildArtifacts,
    verify_persisted_controlled_build_artifacts,
};
use crate::ced_windows_source::inspect_cmake_windows_archive;
use crate::controlled_build::{
    BuilderIdentity, ControlledBuildReceipt, ControlledBuildReceiptPublication, DependencySource,
    InputIdentityEntry, SourceIdentity, SupportingArtifactRef, ValidationReference, census_outputs,
    decode_controlled_build_receipt, write_controlled_build_receipt_exclusive,
};
use crate::digest::sha256_hex;
use crate::provenance::Provenance;
use crate::rfdetr_windows::{
    GGML_COMMIT, GGML_REPOSITORY, RF_DETR_COMMIT, RF_DETR_REPOSITORY, RFDETR_CLI_OUTPUT_LABEL,
    RFDETR_WINDOWS_BUILD_EVIDENCE_LABEL, RFDETR_WINDOWS_BUILD_EVIDENCE_SCHEMA_V1,
    RFDETR_WINDOWS_CMAKE_CACHE_LABEL, RFDETR_WINDOWS_GGML_BUNDLE_LABEL,
    RFDETR_WINDOWS_RF_BUNDLE_LABEL, RFDETR_WINDOWS_VCXPROJ_LABEL, RfdetrWindowsBuildEvidence,
    RfdetrWindowsSubprocessRecord, decode_rfdetr_windows_build_evidence,
    encode_rfdetr_windows_build_evidence, rfdetr_windows_build_configuration,
    validate_build_option_evidence, validate_cmake_cache,
};

const RFDETR_WINDOWS_OUTPUT_TREE_ENTRIES: usize = 2;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubprocessEvidenceFileEntry {
    pub label: String,
    pub argv: Vec<String>,
    pub cwd: String,
    pub exit_code: i32,
    pub stdout_path: PathBuf,
    pub stderr_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RfdetrWindowsBuildRecord {
    pub receipt: ControlledBuildReceipt,
    pub receipt_publication: RfdetrWindowsReceiptPublication,
    pub evidence_sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RfdetrWindowsReceiptPublication {
    Durable,
    PublishedButNotDurable,
}

impl RfdetrWindowsReceiptPublication {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Durable => "durable",
            Self::PublishedButNotDurable => "published-but-not-durable",
        }
    }
}

#[derive(Debug)]
pub struct RfdetrWindowsSourceError {
    message: String,
}

impl RfdetrWindowsSourceError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for RfdetrWindowsSourceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for RfdetrWindowsSourceError {}

impl From<io::Error> for RfdetrWindowsSourceError {
    fn from(source: io::Error) -> Self {
        Self::new(source.to_string())
    }
}

pub fn usage() -> &'static str {
    "usage: solstone-distribution rfdetr-windows <plan|verify-inputs|record|verify> [FLAG]"
}

pub fn run_cli(args: &[String]) -> Result<String, RfdetrWindowsSourceError> {
    let Some((operation, rest)) = args.split_first() else {
        return Err(RfdetrWindowsSourceError::new(usage()));
    };
    let flags = parse_flags(rest)?;
    match operation.as_str() {
        "plan" => {
            require_only(&flags, &[])?;
            let repo = repository_root()?;
            let cmake = crate::acquire::windows_cmake_archive_input(&repo)
                .map_err(|error| RfdetrWindowsSourceError::new(error.to_string()))?;
            let model_source = fs::read_to_string(
                repo.join("core/crates/solstone-core-local/src/install/rfdetr_install.rs"),
            )?;
            let model_sha256 =
                crate::inventory::digest_const_hex(&model_source, "RFDETR_MODEL_SHA256")
                    .ok_or_else(|| {
                        RfdetrWindowsSourceError::new("RF-DETR model digest authority is missing")
                    })?;
            serde_json::to_string_pretty(&serde_json::json!({
                "rf_commit": RF_DETR_COMMIT,
                "ggml_commit": GGML_COMMIT,
                "configuration": rfdetr_windows_build_configuration(),
                "cmake": {"version": cmake.version, "size": cmake.size, "sha256": cmake.sha256},
                "model_sha256": model_sha256,
            }))
            .map_err(|error| RfdetrWindowsSourceError::new(error.to_string()))
        }
        "help" | "--help" | "-h" => {
            require_only(&flags, &[])?;
            Ok(usage().to_owned())
        }
        "verify-inputs" => {
            let repo_root = repository_root()?;
            let rf_bundle = required_path(&flags, "--rf-bundle")?;
            let ggml_bundle = required_path(&flags, "--ggml-bundle")?;
            let cmake_archive = required_path(&flags, "--cmake-archive")?;
            require_only(&flags, &["--rf-bundle", "--ggml-bundle", "--cmake-archive"])?;
            let rf_id = inspect_git_bundle(&rf_bundle, RFDETR_WINDOWS_RF_BUNDLE_LABEL)?;
            let ggml_id = inspect_git_bundle(&ggml_bundle, RFDETR_WINDOWS_GGML_BUNDLE_LABEL)?;
            let (cmake_version, cmake_id) =
                inspect_cmake_windows_archive(&repo_root, &cmake_archive)
                    .map_err(|e| RfdetrWindowsSourceError::new(e.to_string()))?;
            Ok(format!(
                "RFDETR_WINDOWS_INPUTS_OK rf_sha256={} rf_size={} ggml_sha256={} ggml_size={} cmake_version={} cmake_sha256={} cmake_size={}",
                rf_id.sha256,
                rf_id.size,
                ggml_id.sha256,
                ggml_id.size,
                cmake_version,
                cmake_id.sha256,
                cmake_id.size
            ))
        }
        "record" => {
            let repo_root = repository_root()?;
            let rf_bundle = required_path(&flags, "--rf-bundle")?;
            let ggml_bundle = required_path(&flags, "--ggml-bundle")?;
            let cmake_archive = required_path(&flags, "--cmake-archive")?;
            let cmake_cache = required_path(&flags, "--cmake-cache")?;
            let build_option_evidence = required_path(&flags, "--build-option-evidence")?;
            let subprocess_evidence = required_path(&flags, "--subprocess-evidence")?;
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
                    "--rf-bundle",
                    "--ggml-bundle",
                    "--cmake-archive",
                    "--cmake-cache",
                    "--build-option-evidence",
                    "--subprocess-evidence",
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
            let rf_id = inspect_git_bundle(&rf_bundle, RFDETR_WINDOWS_RF_BUNDLE_LABEL)?;
            let ggml_id = inspect_git_bundle(&ggml_bundle, RFDETR_WINDOWS_GGML_BUNDLE_LABEL)?;
            let (_, cmake_id) = inspect_cmake_windows_archive(&repo_root, &cmake_archive)
                .map_err(|e| RfdetrWindowsSourceError::new(e.to_string()))?;

            let product = Provenance {
                commit: product_commit,
                lock_sha256: cargo_lock_sha256,
            };
            verify_product_provenance(&repo_root, &product)?;
            let record = record_from_verified_inputs(
                rf_id,
                ggml_id,
                cmake_id,
                &cmake_cache,
                &build_option_evidence,
                &subprocess_evidence,
                &output_root,
                &evidence_path,
                &receipt_path,
                &validation_path,
                product,
                BuilderIdentity {
                    host: builder_host,
                    toolchain,
                },
            )?;
            let output = record
                .receipt
                .outputs
                .first()
                .expect("RF-DETR record creates exactly one output");
            Ok(format!(
                "RFDETR_WINDOWS_RECORD_OK receipt={} evidence_sha256={} output_sha256={} receipt_publication={}",
                receipt_path.display(),
                record.evidence_sha256,
                output.pre_signing_sha256,
                record.receipt_publication.as_str()
            ))
        }
        "verify" => {
            let receipt_path = required_path(&flags, "--receipt")?;
            let output_root = required_path(&flags, "--output-root")?;
            let evidence_path = required_path(&flags, "--evidence")?;
            let cmake_cache_path = required_path(&flags, "--cmake-cache")?;
            let build_option_path = required_path(&flags, "--build-option-evidence")?;
            let subprocess_evidence_path = required_path(&flags, "--subprocess-evidence")?;
            require_only(
                &flags,
                &[
                    "--receipt",
                    "--output-root",
                    "--evidence",
                    "--cmake-cache",
                    "--build-option-evidence",
                    "--subprocess-evidence",
                ],
            )?;
            let verified = verify_rfdetr_windows_assembly(
                &receipt_path,
                &output_root,
                &evidence_path,
                &cmake_cache_path,
                &build_option_path,
                &subprocess_evidence_path,
            )?;
            let output = verified
                .receipt()
                .outputs
                .first()
                .expect("verified RF-DETR receipt has one output");
            Ok(format!(
                "RFDETR_WINDOWS_ARTIFACT_VERIFY_OK receipt={} output_sha256={}",
                receipt_path.display(),
                output.pre_signing_sha256
            ))
        }
        other => Err(RfdetrWindowsSourceError::new(format!(
            "unknown RF-DETR Windows command {other:?}\n{}",
            usage()
        ))),
    }
}

pub fn inspect_git_bundle(
    path: &Path,
    label: &str,
) -> Result<InputIdentityEntry, RfdetrWindowsSourceError> {
    if !path.is_file() {
        return Err(RfdetrWindowsSourceError::new(format!(
            "Git bundle is not a regular file: {}",
            path.display()
        )));
    }
    let metadata = fs::metadata(path)?;
    if metadata.len() == 0 {
        return Err(RfdetrWindowsSourceError::new(format!(
            "Git bundle is empty: {}",
            path.display()
        )));
    }
    let expected = match label {
        RFDETR_WINDOWS_RF_BUNDLE_LABEL => RF_DETR_COMMIT,
        RFDETR_WINDOWS_GGML_BUNDLE_LABEL => GGML_COMMIT,
        _ => {
            return Err(RfdetrWindowsSourceError::new(
                "unrecognized RF-DETR bundle role",
            ));
        }
    };
    verify_bundle_commit(path, expected)?;
    let bytes = fs::read(path)?;
    Ok(InputIdentityEntry {
        label: label.to_owned(),
        sha256: sha256_hex(&bytes),
        size: bytes.len() as u64,
    })
}

/// Import into an empty object database: a bundle that requires hidden local
/// prerequisites cannot satisfy the pinned-source admission check.
fn verify_bundle_commit(path: &Path, expected: &str) -> Result<(), RfdetrWindowsSourceError> {
    let path = fs::canonicalize(path)?;
    let scratch = tempfile::Builder::new()
        .prefix("rfdetr-bundle-verify-")
        .tempdir()?;
    let outcome = (|| {
        git_checked(scratch.path(), &["init", "--bare", "."])?;
        let path = path
            .to_str()
            .ok_or_else(|| RfdetrWindowsSourceError::new("bundle path is not UTF-8"))?;
        git_checked(scratch.path(), &["bundle", "verify", path])?;
        git_checked(scratch.path(), &["bundle", "unbundle", path])?;
        let kind = git_checked(scratch.path(), &["cat-file", "-t", expected])?;
        if kind.trim() != "commit" {
            return Err(RfdetrWindowsSourceError::new(
                "bundle does not contain the exact pinned commit",
            ));
        }
        git_checked(
            scratch.path(),
            &["fsck", "--full", "--strict", "--no-reflogs", expected],
        )?;
        Ok(())
    })();
    // Cleanup failures remain failures even when object verification succeeded.
    scratch.close()?;
    outcome
}

fn git_checked(root: &Path, args: &[&str]) -> Result<String, RfdetrWindowsSourceError> {
    let mut command = Command::new("git");
    command
        .arg("--no-replace-objects")
        .arg("-C")
        .arg(root)
        .args(args);
    // Explicit local objects/configuration only. An inherited Git environment
    // must not turn the empty verification database into a different checkout.
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("GIT_") {
            command.env_remove(name);
        }
    }
    command.env("GIT_CONFIG_NOSYSTEM", "1").env(
        "GIT_CONFIG_GLOBAL",
        if cfg!(windows) { "NUL" } else { "/dev/null" },
    );
    let output = command.output()?;
    if !output.status.success() {
        return Err(RfdetrWindowsSourceError::new(format!(
            "git {} failed ({}): {}",
            args.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    String::from_utf8(output.stdout)
        .map_err(|error| RfdetrWindowsSourceError::new(error.to_string()))
}

fn verify_product_provenance(
    root: &Path,
    product: &Provenance,
) -> Result<(), RfdetrWindowsSourceError> {
    let commit = git_checked(root, &["rev-parse", "HEAD"])?;
    crate::provenance::require_commit(&product.commit, commit.trim())
        .map_err(|error| RfdetrWindowsSourceError::new(error.to_string()))?;
    let status = git_checked(root, &["status", "--porcelain", "--untracked-files=normal"])?;
    crate::provenance::require_clean(!status.is_empty())
        .map_err(|error| RfdetrWindowsSourceError::new(error.to_string()))?;
    let digest = crate::provenance::lock_digest(&root.join("core/Cargo.lock"))
        .map_err(|error| RfdetrWindowsSourceError::new(error.to_string()))?;
    crate::provenance::require_lock(&product.lock_sha256, &digest)
        .map_err(|error| RfdetrWindowsSourceError::new(error.to_string()))
}

pub(crate) fn read_subprocess_stream(
    metadata_path: &Path,
    stream_path: &Path,
    label: &str,
    stream: &str,
) -> Result<Vec<u8>, RfdetrWindowsSourceError> {
    use std::io::Read;
    let expected = format!("logs/{label}.{stream}");
    if label.is_empty()
        || !label
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == b'-')
        || stream_path.to_str() != Some(expected.as_str())
    {
        return Err(RfdetrWindowsSourceError::new(
            "subprocess stream must use its declared relative log member",
        ));
    }
    let root = fs::canonicalize(metadata_path.parent().unwrap_or_else(|| Path::new(".")))?;
    let path = root.join("logs").join(format!("{label}.{stream}"));
    if fs::symlink_metadata(&path)?.file_type().is_symlink()
        || !fs::canonicalize(&path)?.starts_with(&root)
    {
        return Err(RfdetrWindowsSourceError::new(
            "subprocess stream escapes its evidence root",
        ));
    }
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(64 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 64 * 1024 * 1024 {
        return Err(RfdetrWindowsSourceError::new(
            "subprocess stream exceeds evidence limit",
        ));
    }
    Ok(bytes)
}

fn validate_configured_processes(
    evidence: &RfdetrWindowsBuildEvidence,
    cache: &[u8],
) -> Result<(), RfdetrWindowsSourceError> {
    let text = std::str::from_utf8(cache)
        .map_err(|error| RfdetrWindowsSourceError::new(error.to_string()))?;
    let setting = |key: &str| -> Result<String, RfdetrWindowsSourceError> {
        text.lines()
            .find_map(|line| {
                let (name, value) = line.split_once('=')?;
                (name.split_once(':')?.0 == key).then(|| value.trim().to_owned())
            })
            .ok_or_else(|| {
                RfdetrWindowsSourceError::new(format!("missing configured path/setting {key}"))
            })
    };
    if setting("CMAKE_GENERATOR")? != "Visual Studio 17 2022"
        || setting("CMAKE_GENERATOR_PLATFORM")? != "x64"
    {
        return Err(RfdetrWindowsSourceError::new(
            "unreviewed CMake generator/platform",
        ));
    }
    let source = setting("CMAKE_HOME_DIRECTORY")?;
    let build = setting("CMAKE_CACHEFILE_DIR")?;
    let normalized = |value: &str| value.replace('\\', "/").to_ascii_lowercase();
    let get = |label: &str| {
        evidence
            .subprocesses
            .iter()
            .find(|process| process.label == label)
            .ok_or_else(|| RfdetrWindowsSourceError::new(format!("missing subprocess {label}")))
    };
    let configure = get("cmake-configure")?;
    let compile = get("cmake-build")?;
    if configure
        .argv
        .first()
        .is_none_or(|program| program.is_empty())
        || configure.argv.first() != compile.argv.first()
        || normalized(&configure.cwd) != normalized(&source)
        || normalized(&compile.cwd) != normalized(&source)
    {
        return Err(RfdetrWindowsSourceError::new(
            "CMake process program/cwd does not match configured source",
        ));
    }
    let mut expected_configure: Vec<String> = [
        "-S",
        &source,
        "-B",
        &build,
        "-G",
        "Visual Studio 17 2022",
        "-A",
        "x64",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    expected_configure.extend(
        crate::rfdetr_windows::RFDETR_WINDOWS_BUILD_FLAGS
            .iter()
            .map(|value| (*value).to_owned()),
    );
    let expected_compile: Vec<String> = [
        "--build",
        &build,
        "--config",
        "Release",
        "--target",
        "rfdetr-cli",
        "--parallel",
        "2",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    for (process, expected, paths) in [
        (configure, expected_configure, vec![1usize, 3]),
        (compile, expected_compile, vec![1usize]),
    ] {
        if process.exit_code != 0 || process.argv.len() != expected.len() + 1 {
            return Err(RfdetrWindowsSourceError::new(
                "CMake subprocess exit/argument census does not match reviewed invocation",
            ));
        }
        for (index, (actual, expected)) in process.argv[1..].iter().zip(expected).enumerate() {
            let equal = if paths.contains(&index) {
                normalized(actual) == normalized(&expected)
            } else {
                actual == &expected
            };
            if !equal {
                return Err(RfdetrWindowsSourceError::new(format!(
                    "{} argument {index} differs from reviewed configuration",
                    process.label
                )));
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn record_from_verified_inputs(
    rf_bundle: InputIdentityEntry,
    ggml_bundle: InputIdentityEntry,
    cmake_archive: InputIdentityEntry,
    cmake_cache_path: &Path,
    build_option_path: &Path,
    subprocess_evidence_path: &Path,
    output_root: &Path,
    evidence_path: &Path,
    receipt_path: &Path,
    validation_path: &Path,
    product: Provenance,
    builder: BuilderIdentity,
) -> Result<RfdetrWindowsBuildRecord, RfdetrWindowsSourceError> {
    let cmake_cache_bytes = fs::read(cmake_cache_path)?;
    if cmake_cache_bytes.is_empty() {
        return Err(RfdetrWindowsSourceError::new(
            "RF-DETR CMake cache is empty",
        ));
    }
    validate_cmake_cache(&cmake_cache_bytes)
        .map_err(|e| RfdetrWindowsSourceError::new(e.to_string()))?;

    let build_option_bytes = fs::read(build_option_path)?;
    if build_option_bytes.is_empty() {
        return Err(RfdetrWindowsSourceError::new(
            "RF-DETR build option evidence is empty",
        ));
    }
    validate_build_option_evidence(&build_option_bytes)
        .map_err(|e| RfdetrWindowsSourceError::new(e.to_string()))?;

    let subprocess_raw = fs::read(subprocess_evidence_path)?;
    let subprocess_entries: Vec<SubprocessEvidenceFileEntry> =
        serde_json::from_slice(&subprocess_raw).map_err(|e| {
            RfdetrWindowsSourceError::new(format!(
                "could not decode subprocess evidence JSON {}: {e}",
                subprocess_evidence_path.display()
            ))
        })?;

    let mut subprocess_records = Vec::new();
    for entry in subprocess_entries {
        if entry.exit_code != 0 {
            return Err(RfdetrWindowsSourceError::new(format!(
                "subprocess {} failed with exit code {}",
                entry.label, entry.exit_code
            )));
        }
        let stdout_bytes = read_subprocess_stream(
            subprocess_evidence_path,
            &entry.stdout_path,
            &entry.label,
            "stdout",
        )?;
        let stderr_bytes = read_subprocess_stream(
            subprocess_evidence_path,
            &entry.stderr_path,
            &entry.label,
            "stderr",
        )?;
        subprocess_records.push(RfdetrWindowsSubprocessRecord {
            label: entry.label.clone(),
            argv: entry.argv,
            cwd: entry.cwd,
            exit_code: entry.exit_code,
            stdout: InputIdentityEntry {
                label: format!("logs/{}.stdout", entry.label),
                sha256: sha256_hex(&stdout_bytes),
                size: stdout_bytes.len() as u64,
            },
            stderr: InputIdentityEntry {
                label: format!("logs/{}.stderr", entry.label),
                sha256: sha256_hex(&stderr_bytes),
                size: stderr_bytes.len() as u64,
            },
        });
    }

    let output_path = output_root.join(RFDETR_CLI_OUTPUT_LABEL);
    let output = fs::read(&output_path).map_err(|e| {
        RfdetrWindowsSourceError::new(format!(
            "could not read RF-DETR output {}: {e}",
            output_path.display()
        ))
    })?;
    let outputs = census_outputs(&[(RFDETR_CLI_OUTPUT_LABEL, output.as_slice())])
        .map_err(|source| RfdetrWindowsSourceError::new(source.to_string()))?;

    let evidence = RfdetrWindowsBuildEvidence {
        schema: RFDETR_WINDOWS_BUILD_EVIDENCE_SCHEMA_V1.to_owned(),
        rf_bundle: rf_bundle.clone(),
        ggml_bundle: ggml_bundle.clone(),
        ggml: DependencySource {
            repository: GGML_REPOSITORY.to_owned(),
            revision: GGML_COMMIT.to_owned(),
            content_sha256: ggml_bundle.sha256.clone(),
        },
        cmake_archive,
        cmake_cache: InputIdentityEntry {
            label: RFDETR_WINDOWS_CMAKE_CACHE_LABEL.to_owned(),
            sha256: sha256_hex(&cmake_cache_bytes),
            size: cmake_cache_bytes.len() as u64,
        },
        build_option_evidence: InputIdentityEntry {
            label: RFDETR_WINDOWS_VCXPROJ_LABEL.to_owned(),
            sha256: sha256_hex(&build_option_bytes),
            size: build_option_bytes.len() as u64,
        },
        subprocesses: subprocess_records,
    };

    validate_configured_processes(&evidence, &cmake_cache_bytes)?;
    let evidence_bytes = encode_rfdetr_windows_build_evidence(&evidence)
        .map_err(|source| RfdetrWindowsSourceError::new(source.to_string()))?;
    let evidence_sha256 = sha256_hex(&evidence_bytes);
    publish_evidence_exclusive(evidence_path, &evidence_bytes)?;

    let validation = fs::read(validation_path)?;
    if validation.is_empty() {
        return Err(RfdetrWindowsSourceError::new(
            "RF-DETR validation reference is empty",
        ));
    }

    let source_identity = SourceIdentity {
        product,
        windows_dependency: DependencySource {
            repository: RF_DETR_REPOSITORY.to_owned(),
            revision: RF_DETR_COMMIT.to_owned(),
            content_sha256: rf_bundle.sha256,
        },
    };

    let mut draft =
        crate::rfdetr_windows::assemble_receipt_draft(source_identity, evidence, builder, outputs)
            .map_err(|source| RfdetrWindowsSourceError::new(source.to_string()))?;
    draft.schema = Some(crate::controlled_build::CONTROLLED_BUILD_RECEIPT_SCHEMA_V1.to_owned());
    draft.validation = Some(ValidationReference {
        description: "RF-DETR Windows network-denied CMake build and PE admission log".to_owned(),
        sha256: sha256_hex(&validation),
    });

    let receipt = draft
        .validate()
        .map_err(|source| RfdetrWindowsSourceError::new(source.to_string()))?;
    let publication = write_controlled_build_receipt_exclusive(receipt_path, &receipt)
        .map_err(|source| RfdetrWindowsSourceError::new(source.to_string()))?;

    let receipt_publication = match publication {
        ControlledBuildReceiptPublication::Durable { .. } => {
            RfdetrWindowsReceiptPublication::Durable
        }
        ControlledBuildReceiptPublication::PublishedButNotDurable { .. } => {
            RfdetrWindowsReceiptPublication::PublishedButNotDurable
        }
        ControlledBuildReceiptPublication::PublicationUnconfirmed { publication, .. } => {
            return Err(RfdetrWindowsSourceError::new(format!(
                "RF-DETR receipt publication is unconfirmed: {publication:?}"
            )));
        }
    };

    if receipt.supporting.as_slice()
        != [SupportingArtifactRef {
            label: RFDETR_WINDOWS_BUILD_EVIDENCE_LABEL.to_owned(),
            sha256: evidence_sha256.clone(),
        }]
    {
        return Err(RfdetrWindowsSourceError::new(
            "RF-DETR receipt does not bind its generated evidence sidecar",
        ));
    }

    Ok(RfdetrWindowsBuildRecord {
        receipt,
        receipt_publication,
        evidence_sha256,
    })
}

pub fn verify_rfdetr_windows_assembly(
    receipt_path: &Path,
    output_root: &Path,
    evidence_path: &Path,
    cmake_cache_path: &Path,
    build_option_path: &Path,
    subprocess_evidence_path: &Path,
) -> Result<VerifiedControlledBuildArtifacts, RfdetrWindowsSourceError> {
    let receipt_bytes = fs::read(receipt_path)?;
    let receipt = decode_controlled_build_receipt(&receipt_bytes)
        .map_err(|e| RfdetrWindowsSourceError::new(e.to_string()))?;

    let supporting_ref = receipt
        .supporting
        .iter()
        .find(|s| s.label == RFDETR_WINDOWS_BUILD_EVIDENCE_LABEL)
        .ok_or_else(|| {
            RfdetrWindowsSourceError::new("receipt does not contain RF-DETR build evidence label")
        })?;

    let evidence_bytes = fs::read(evidence_path)?;
    let actual_evidence_sha256 = sha256_hex(&evidence_bytes);
    if actual_evidence_sha256 != supporting_ref.sha256 {
        return Err(RfdetrWindowsSourceError::new(format!(
            "evidence digest mismatch: expected {}, found {}",
            supporting_ref.sha256, actual_evidence_sha256
        )));
    }

    let evidence = decode_rfdetr_windows_build_evidence(&evidence_bytes)
        .map_err(|e| RfdetrWindowsSourceError::new(e.to_string()))?;
    evidence
        .validate_against_source(&receipt.source)
        .map_err(|e| RfdetrWindowsSourceError::new(e.to_string()))?;

    let cache_bytes = fs::read(cmake_cache_path)?;
    let actual_cache_sha256 = sha256_hex(&cache_bytes);
    if actual_cache_sha256 != evidence.cmake_cache.sha256
        || cache_bytes.len() as u64 != evidence.cmake_cache.size
    {
        return Err(RfdetrWindowsSourceError::new(format!(
            "CMake cache digest mismatch: expected {}, found {}",
            evidence.cmake_cache.sha256, actual_cache_sha256
        )));
    }
    validate_cmake_cache(&cache_bytes).map_err(|e| RfdetrWindowsSourceError::new(e.to_string()))?;
    validate_configured_processes(&evidence, &cache_bytes)?;

    let build_option_bytes = fs::read(build_option_path)?;
    let actual_build_option_sha256 = sha256_hex(&build_option_bytes);
    if actual_build_option_sha256 != evidence.build_option_evidence.sha256
        || build_option_bytes.len() as u64 != evidence.build_option_evidence.size
    {
        return Err(RfdetrWindowsSourceError::new(format!(
            "Build option evidence digest mismatch: expected {}, found {}",
            evidence.build_option_evidence.sha256, actual_build_option_sha256
        )));
    }
    validate_build_option_evidence(&build_option_bytes)
        .map_err(|e| RfdetrWindowsSourceError::new(e.to_string()))?;

    let subprocess_raw = fs::read(subprocess_evidence_path)?;
    let subprocess_entries: Vec<SubprocessEvidenceFileEntry> =
        serde_json::from_slice(&subprocess_raw).map_err(|e| {
            RfdetrWindowsSourceError::new(format!("could not decode subprocess evidence JSON: {e}"))
        })?;

    if subprocess_entries.len() != evidence.subprocesses.len() {
        return Err(RfdetrWindowsSourceError::new(format!(
            "subprocess count mismatch: expected {}, found {}",
            evidence.subprocesses.len(),
            subprocess_entries.len()
        )));
    }

    for (entry, recorded) in subprocess_entries.iter().zip(&evidence.subprocesses) {
        if entry.label != recorded.label
            || entry.argv != recorded.argv
            || entry.cwd != recorded.cwd
            || entry.exit_code != recorded.exit_code
        {
            return Err(RfdetrWindowsSourceError::new(format!(
                "subprocess label mismatch: expected {}, found {}",
                recorded.label, entry.label
            )));
        }
        if entry.exit_code != 0 || recorded.exit_code != 0 {
            return Err(RfdetrWindowsSourceError::new(format!(
                "subprocess {} has nonzero exit code",
                entry.label
            )));
        }
        let stdout_bytes = read_subprocess_stream(
            subprocess_evidence_path,
            &entry.stdout_path,
            &entry.label,
            "stdout",
        )?;
        let actual_stdout_sha256 = sha256_hex(&stdout_bytes);
        if actual_stdout_sha256 != recorded.stdout.sha256
            || stdout_bytes.len() as u64 != recorded.stdout.size
        {
            return Err(RfdetrWindowsSourceError::new(format!(
                "subprocess {} stdout digest mismatch: expected {}, found {}",
                entry.label, recorded.stdout.sha256, actual_stdout_sha256
            )));
        }
        let stderr_bytes = read_subprocess_stream(
            subprocess_evidence_path,
            &entry.stderr_path,
            &entry.label,
            "stderr",
        )?;
        let actual_stderr_sha256 = sha256_hex(&stderr_bytes);
        if actual_stderr_sha256 != recorded.stderr.sha256
            || stderr_bytes.len() as u64 != recorded.stderr.size
        {
            return Err(RfdetrWindowsSourceError::new(format!(
                "subprocess {} stderr digest mismatch: expected {}, found {}",
                entry.label, recorded.stderr.sha256, actual_stderr_sha256
            )));
        }
    }

    let expected_config = rfdetr_windows_build_configuration();
    if receipt.configuration != expected_config {
        return Err(RfdetrWindowsSourceError::new(format!(
            "configuration mismatch: expected {expected_config:?}, found {:?}",
            receipt.configuration
        )));
    }

    let [output_entry] = receipt.outputs.as_slice() else {
        return Err(RfdetrWindowsSourceError::new(format!(
            "output count mismatch: expected 1, found {}",
            receipt.outputs.len()
        )));
    };
    if output_entry.label != RFDETR_CLI_OUTPUT_LABEL {
        return Err(RfdetrWindowsSourceError::new(format!(
            "output label mismatch: expected {RFDETR_CLI_OUTPUT_LABEL}, found {}",
            output_entry.label
        )));
    }

    verify_persisted_controlled_build_artifacts(
        receipt_path,
        output_root,
        ControlledBuildArtifactVerificationLimits::new(
            RFDETR_WINDOWS_OUTPUT_TREE_ENTRIES,
            2,
            128 * 1024 * 1024,
        ),
    )
    .map_err(|e| RfdetrWindowsSourceError::new(e.to_string()))
}

fn publish_evidence_exclusive(
    destination: &Path,
    bytes: &[u8],
) -> Result<(), RfdetrWindowsSourceError> {
    if destination.exists() {
        return Err(RfdetrWindowsSourceError::new(format!(
            "refusing to overwrite existing RF-DETR evidence {}",
            destination.display()
        )));
    }
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    let publication = solstone_core_journal_io::write_bytes_exclusive_detailed(
        destination,
        bytes,
        solstone_core_journal_io::AtomicWriteOptions::default(),
    )
    .map_err(|source| RfdetrWindowsSourceError::new(source.to_string()))?;
    if matches!(
        publication.final_name,
        solstone_core_journal_io::FinalNameConfirmation::Confirmed { .. }
    ) && matches!(
        publication.cleanup,
        solstone_core_journal_io::StageCleanup::Removed
    ) {
        Ok(())
    } else {
        Err(RfdetrWindowsSourceError::new(format!(
            "RF-DETR evidence publication is unconfirmed: {publication:?}"
        )))
    }
}

fn parse_flags(args: &[String]) -> Result<BTreeMap<String, String>, RfdetrWindowsSourceError> {
    if !args.len().is_multiple_of(2) {
        return Err(RfdetrWindowsSourceError::new(
            "RF-DETR Windows command flags must be name/value pairs",
        ));
    }
    let mut flags = BTreeMap::new();
    for pair in args.chunks_exact(2) {
        let name = &pair[0];
        if !name.starts_with("--") {
            return Err(RfdetrWindowsSourceError::new(format!(
                "invalid RF-DETR Windows flag {name:?}"
            )));
        }
        if flags.insert(name.clone(), pair[1].clone()).is_some() {
            return Err(RfdetrWindowsSourceError::new(format!(
                "duplicate RF-DETR Windows flag {name}"
            )));
        }
    }
    Ok(flags)
}

fn required_text(
    flags: &BTreeMap<String, String>,
    name: &str,
) -> Result<String, RfdetrWindowsSourceError> {
    let value = flags
        .get(name)
        .ok_or_else(|| RfdetrWindowsSourceError::new(format!("missing required {name}")))?;
    if value.is_empty() {
        return Err(RfdetrWindowsSourceError::new(format!(
            "required RF-DETR Windows flag {name} is empty"
        )));
    }
    Ok(value.clone())
}

fn required_path(
    flags: &BTreeMap<String, String>,
    name: &str,
) -> Result<PathBuf, RfdetrWindowsSourceError> {
    Ok(PathBuf::from(required_text(flags, name)?))
}

fn required_lower_hex(
    flags: &BTreeMap<String, String>,
    name: &str,
    length: usize,
) -> Result<String, RfdetrWindowsSourceError> {
    let value = required_text(flags, name)?;
    if value.len() != length
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(RfdetrWindowsSourceError::new(format!(
            "required RF-DETR Windows flag {name} must be {length} lowercase hexadecimal characters"
        )));
    }
    Ok(value)
}

fn require_only(
    flags: &BTreeMap<String, String>,
    admitted: &[&str],
) -> Result<(), RfdetrWindowsSourceError> {
    if let Some(unexpected) = flags.keys().find(|name| !admitted.contains(&name.as_str())) {
        return Err(RfdetrWindowsSourceError::new(format!(
            "unknown RF-DETR Windows flag {unexpected}"
        )));
    }
    Ok(())
}

fn repository_root() -> Result<PathBuf, RfdetrWindowsSourceError> {
    let mut cursor = std::env::current_dir()?;
    loop {
        if cursor
            .join("core/distribution/builder-inputs.toml")
            .is_file()
        {
            return Ok(cursor);
        }
        cursor = cursor.parent().map(Path::to_path_buf).ok_or_else(|| {
            RfdetrWindowsSourceError::new("could not find core/distribution/builder-inputs.toml")
        })?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pe::{FixtureSpec, ImportSpec, PeSymbolSpec, fixture};

    fn sample_valid_cmake_cache() -> &'static str {
        r#"
CMAKE_HOME_DIRECTORY:INTERNAL=/work/source
CMAKE_CACHEFILE_DIR:INTERNAL=/work/build
CMAKE_GENERATOR:INTERNAL=Visual Studio 17 2022
CMAKE_GENERATOR_PLATFORM:INTERNAL=x64
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
"#
    }

    fn fixture_argv(label: &str) -> Vec<String> {
        let args = if label == "cmake-configure" {
            let mut args = vec![
                "/tools/cmake",
                "-S",
                "/work/source",
                "-B",
                "/work/build",
                "-G",
                "Visual Studio 17 2022",
                "-A",
                "x64",
            ];
            args.extend(crate::rfdetr_windows::RFDETR_WINDOWS_BUILD_FLAGS);
            args
        } else {
            vec![
                "/tools/cmake",
                "--build",
                "/work/build",
                "--config",
                "Release",
                "--target",
                "rfdetr-cli",
                "--parallel",
                "2",
            ]
        };
        args.into_iter().map(str::to_owned).collect()
    }

    fn sample_valid_vcxproj() -> &'static str {
        r#"
<Project>
<ItemDefinitionGroup Condition="'$(Configuration)|$(Platform)'=='Release|x64'">
  <ClCompile>
    <RuntimeLibrary>MultiThreadedDLL</RuntimeLibrary>
    <EnableEnhancedInstructionSet>NotSet</EnableEnhancedInstructionSet>
  </ClCompile>
</ItemDefinitionGroup>
</Project>
"#
    }

    fn plant_pe_fixture(output_root: &Path) {
        let bin_dir = output_root.join("bin");
        fs::create_dir_all(&bin_dir).unwrap();
        let bytes = fixture(&FixtureSpec {
            dll: false,
            imports: &[ImportSpec {
                name: "kernel32.dll",
                symbols: &[PeSymbolSpec::Named("ExitProcess")],
            }],
            ..FixtureSpec::default()
        });
        fs::write(bin_dir.join("rfdetr-cli.exe"), bytes).unwrap();
    }

    fn commit_fixture(root: &Path, content: &str) -> String {
        fs::create_dir_all(root.join("core")).unwrap();
        fs::write(root.join("core/Cargo.lock"), content).unwrap();
        git_checked(root, &["add", "core/Cargo.lock"]).unwrap();
        git_checked(
            root,
            &[
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "-m",
                "fixture",
            ],
        )
        .unwrap();
        git_checked(root, &["rev-parse", "HEAD"])
            .unwrap()
            .trim()
            .to_owned()
    }

    #[test]
    fn source_bundle_requires_pinned_commit_and_complete_object_closure() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("source");
        fs::create_dir(&repo).unwrap();
        git_checked(&repo, &["init", "."]).unwrap();
        let first = commit_fixture(&repo, "first");
        let second = commit_fixture(&repo, "second");
        let complete = temp.path().join("complete.bundle");
        git_checked(
            &repo,
            &["bundle", "create", complete.to_str().unwrap(), "HEAD"],
        )
        .unwrap();
        verify_bundle_commit(&complete, &second).unwrap();
        assert!(verify_bundle_commit(&complete, &"0".repeat(40)).is_err());
        let incremental = temp.path().join("incremental.bundle");
        git_checked(
            &repo,
            &[
                "bundle",
                "create",
                incremental.to_str().unwrap(),
                &format!("{first}..HEAD"),
            ],
        )
        .unwrap();
        assert!(verify_bundle_commit(&incremental, &second).is_err());
    }

    #[test]
    fn product_identity_cannot_be_restamped_over_dirty_or_other_source() {
        let temp = tempfile::tempdir().unwrap();
        git_checked(temp.path(), &["init", "."]).unwrap();
        let commit = commit_fixture(temp.path(), "locked");
        let product = Provenance {
            commit,
            lock_sha256: sha256_hex(b"locked"),
        };
        verify_product_provenance(temp.path(), &product).unwrap();
        let wrong = Provenance {
            commit: "0".repeat(40),
            ..product.clone()
        };
        assert!(verify_product_provenance(temp.path(), &wrong).is_err());
        let wrong_lock = Provenance {
            lock_sha256: "0".repeat(64),
            ..product.clone()
        };
        assert!(verify_product_provenance(temp.path(), &wrong_lock).is_err());
        fs::write(temp.path().join("untracked.rs"), b"changed").unwrap();
        assert!(verify_product_provenance(temp.path(), &product).is_err());
        fs::remove_file(temp.path().join("untracked.rs")).unwrap();
        fs::write(temp.path().join("core/Cargo.lock"), b"other").unwrap();
        assert!(verify_product_provenance(temp.path(), &product).is_err());
    }

    #[test]
    fn subprocess_logs_cannot_resolve_arbitrary_paths() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir(temp.path().join("logs")).unwrap();
        fs::write(
            temp.path().join("logs/cmake-build.stdout"),
            b"original bytes\r\n",
        )
        .unwrap();
        let metadata = temp.path().join("subprocess.json");
        assert_eq!(
            read_subprocess_stream(
                &metadata,
                Path::new("logs/cmake-build.stdout"),
                "cmake-build",
                "stdout"
            )
            .unwrap(),
            b"original bytes\r\n"
        );
        for invalid in [
            "../outside",
            "/absolute",
            "logs/other.stdout",
            "logs/../cmake-build.stdout",
        ] {
            assert!(
                read_subprocess_stream(&metadata, Path::new(invalid), "cmake-build", "stdout")
                    .is_err()
            );
        }
    }

    #[test]
    fn test_git_bundle_inspection_rejects_missing_and_empty_file() {
        let temp = tempfile::tempdir().unwrap();
        let missing = temp.path().join("missing.bundle");
        let err = inspect_git_bundle(&missing, RFDETR_WINDOWS_RF_BUNDLE_LABEL).unwrap_err();
        assert!(err.to_string().contains("is not a regular file"));

        let empty = temp.path().join("empty.bundle");
        fs::write(&empty, b"").unwrap();
        let err = inspect_git_bundle(&empty, RFDETR_WINDOWS_RF_BUNDLE_LABEL).unwrap_err();
        assert!(err.to_string().contains("is empty"));
    }

    #[test]
    fn test_nonzero_subprocess_exit_fails_record_even_with_planted_binary() {
        let temp = tempfile::tempdir().unwrap();
        let output_root = temp.path().join("output");
        plant_pe_fixture(&output_root);

        let cache_file = temp.path().join("CMakeCache.txt");
        fs::write(&cache_file, sample_valid_cmake_cache()).unwrap();

        let vcxproj_file = temp.path().join("rfdetr-cli.Release.vcxproj");
        fs::write(&vcxproj_file, sample_valid_vcxproj()).unwrap();

        let stdout_file = temp.path().join("proc.stdout");
        let stderr_file = temp.path().join("proc.stderr");
        fs::write(&stdout_file, b"out").unwrap();
        fs::write(&stderr_file, b"err").unwrap();

        let subprocess_file = temp.path().join("subprocess.json");
        let subproc_entries = vec![SubprocessEvidenceFileEntry {
            label: "failed-proc".to_owned(),
            argv: vec!["failing-command".to_owned()],
            cwd: "/work".to_owned(),
            exit_code: 1,
            stdout_path: stdout_file.clone(),
            stderr_path: stderr_file.clone(),
        }];
        fs::write(
            &subprocess_file,
            serde_json::to_vec(&subproc_entries).unwrap(),
        )
        .unwrap();

        let val_file = temp.path().join("validation.log");
        fs::write(&val_file, b"val").unwrap();

        let rf_id = InputIdentityEntry {
            label: RFDETR_WINDOWS_RF_BUNDLE_LABEL.to_owned(),
            sha256: "a".repeat(64),
            size: 100,
        };
        let ggml_id = InputIdentityEntry {
            label: RFDETR_WINDOWS_GGML_BUNDLE_LABEL.to_owned(),
            sha256: "b".repeat(64),
            size: 200,
        };
        let cmake_id = InputIdentityEntry {
            label: crate::ced_windows::CED_WINDOWS_CMAKE_ARCHIVE_LABEL.to_owned(),
            sha256: "c".repeat(64),
            size: 300,
        };

        let err = record_from_verified_inputs(
            rf_id,
            ggml_id,
            cmake_id,
            &cache_file,
            &vcxproj_file,
            &subprocess_file,
            &output_root,
            &temp.path().join("evidence.json"),
            &temp.path().join("receipt.json"),
            &val_file,
            Provenance {
                commit: "journal-product".to_owned(),
                lock_sha256: "0".repeat(64),
            },
            BuilderIdentity {
                host: "host".to_owned(),
                toolchain: "msvc".to_owned(),
            },
        )
        .unwrap_err();

        assert!(err.to_string().contains("failed with exit code 1"));
    }

    #[test]
    fn test_assembly_validator_accepts_complete_evidence_set() {
        let temp = tempfile::tempdir().unwrap();
        let output_root = temp.path().join("output");
        plant_pe_fixture(&output_root);

        let cache_file = temp.path().join("CMakeCache.txt");
        fs::write(&cache_file, sample_valid_cmake_cache()).unwrap();

        let vcxproj_file = temp.path().join("rfdetr-cli.Release.vcxproj");
        fs::write(&vcxproj_file, sample_valid_vcxproj()).unwrap();

        let stdout_file = temp.path().join("proc.stdout");
        let stderr_file = temp.path().join("proc.stderr");
        fs::write(&stdout_file, b"out").unwrap();
        fs::write(&stderr_file, b"err").unwrap();

        let subprocess_file = temp.path().join("subprocess.json");
        fs::create_dir(temp.path().join("logs")).unwrap();
        for label in ["cmake-configure", "cmake-build"] {
            fs::write(temp.path().join(format!("logs/{label}.stdout")), b"out").unwrap();
            fs::write(temp.path().join(format!("logs/{label}.stderr")), b"err").unwrap();
        }
        let subproc_entries: Vec<_> = ["cmake-configure", "cmake-build"]
            .into_iter()
            .map(|label| SubprocessEvidenceFileEntry {
                label: label.to_owned(),
                argv: fixture_argv(label),
                cwd: "/work/source".to_owned(),
                exit_code: 0,
                stdout_path: PathBuf::from(format!("logs/{label}.stdout")),
                stderr_path: PathBuf::from(format!("logs/{label}.stderr")),
            })
            .collect();
        fs::write(
            &subprocess_file,
            serde_json::to_vec(&subproc_entries).unwrap(),
        )
        .unwrap();

        let val_file = temp.path().join("validation.log");
        fs::write(&val_file, b"val").unwrap();

        let receipt_file = temp.path().join("receipt.json");
        let evidence_file = temp.path().join("evidence.json");

        let rf_id = InputIdentityEntry {
            label: RFDETR_WINDOWS_RF_BUNDLE_LABEL.to_owned(),
            sha256: "a".repeat(64),
            size: 100,
        };
        let ggml_id = InputIdentityEntry {
            label: RFDETR_WINDOWS_GGML_BUNDLE_LABEL.to_owned(),
            sha256: "b".repeat(64),
            size: 200,
        };
        let cmake_id = InputIdentityEntry {
            label: crate::ced_windows::CED_WINDOWS_CMAKE_ARCHIVE_LABEL.to_owned(),
            sha256: "c".repeat(64),
            size: 300,
        };

        let record = record_from_verified_inputs(
            rf_id,
            ggml_id,
            cmake_id,
            &cache_file,
            &vcxproj_file,
            &subprocess_file,
            &output_root,
            &evidence_file,
            &receipt_file,
            &val_file,
            Provenance {
                commit: "journal-product".to_owned(),
                lock_sha256: "0".repeat(64),
            },
            BuilderIdentity {
                host: "host".to_owned(),
                toolchain: "msvc".to_owned(),
            },
        )
        .unwrap();

        let verified = verify_rfdetr_windows_assembly(
            &receipt_file,
            &output_root,
            &evidence_file,
            &cache_file,
            &vcxproj_file,
            &subprocess_file,
        )
        .unwrap();

        assert_eq!(verified.receipt().source, record.receipt.source);
        let evidence =
            decode_rfdetr_windows_build_evidence(&fs::read(&evidence_file).unwrap()).unwrap();
        for (process, arg, value) in [
            (0, 2, "/different-source"),
            (0, 9, "-DRFDETR_SHARED=ON"),
            (1, 6, "other-target"),
            (1, 8, "8"),
        ] {
            let mut altered = evidence.clone();
            altered.subprocesses[process].argv[arg] = value.to_owned();
            assert!(
                validate_configured_processes(&altered, sample_valid_cmake_cache().as_bytes())
                    .is_err()
            );
        }
        let mut altered = evidence.clone();
        altered.subprocesses[0]
            .argv
            .push("-DGGML_AVX2=ON".to_owned());
        assert!(
            validate_configured_processes(&altered, sample_valid_cmake_cache().as_bytes()).is_err()
        );
        let mut altered = evidence;
        altered.subprocesses[0].cwd = "/different-source".to_owned();
        assert!(
            validate_configured_processes(&altered, sample_valid_cmake_cache().as_bytes()).is_err()
        );
    }

    #[test]
    fn test_assembly_validator_fails_if_any_member_missing_or_tampered() {
        let temp = tempfile::tempdir().unwrap();
        let output_root = temp.path().join("output");
        plant_pe_fixture(&output_root);

        let cache_file = temp.path().join("CMakeCache.txt");
        fs::write(&cache_file, sample_valid_cmake_cache()).unwrap();

        let vcxproj_file = temp.path().join("rfdetr-cli.Release.vcxproj");
        fs::write(&vcxproj_file, sample_valid_vcxproj()).unwrap();

        let stdout_file = temp.path().join("proc.stdout");
        let stderr_file = temp.path().join("proc.stderr");
        fs::write(&stdout_file, b"out").unwrap();
        fs::write(&stderr_file, b"err").unwrap();

        let subprocess_file = temp.path().join("subprocess.json");
        fs::create_dir(temp.path().join("logs")).unwrap();
        for label in ["cmake-configure", "cmake-build"] {
            fs::write(temp.path().join(format!("logs/{label}.stdout")), b"out").unwrap();
            fs::write(temp.path().join(format!("logs/{label}.stderr")), b"err").unwrap();
        }
        let subproc_entries: Vec<_> = ["cmake-configure", "cmake-build"]
            .into_iter()
            .map(|label| SubprocessEvidenceFileEntry {
                label: label.to_owned(),
                argv: fixture_argv(label),
                cwd: "/work/source".to_owned(),
                exit_code: 0,
                stdout_path: PathBuf::from(format!("logs/{label}.stdout")),
                stderr_path: PathBuf::from(format!("logs/{label}.stderr")),
            })
            .collect();
        fs::write(
            &subprocess_file,
            serde_json::to_vec(&subproc_entries).unwrap(),
        )
        .unwrap();

        let val_file = temp.path().join("validation.log");
        fs::write(&val_file, b"val").unwrap();

        let receipt_file = temp.path().join("receipt.json");
        let evidence_file = temp.path().join("evidence.json");

        let rf_id = InputIdentityEntry {
            label: RFDETR_WINDOWS_RF_BUNDLE_LABEL.to_owned(),
            sha256: "a".repeat(64),
            size: 100,
        };
        let ggml_id = InputIdentityEntry {
            label: RFDETR_WINDOWS_GGML_BUNDLE_LABEL.to_owned(),
            sha256: "b".repeat(64),
            size: 200,
        };
        let cmake_id = InputIdentityEntry {
            label: crate::ced_windows::CED_WINDOWS_CMAKE_ARCHIVE_LABEL.to_owned(),
            sha256: "c".repeat(64),
            size: 300,
        };

        record_from_verified_inputs(
            rf_id,
            ggml_id,
            cmake_id,
            &cache_file,
            &vcxproj_file,
            &subprocess_file,
            &output_root,
            &evidence_file,
            &receipt_file,
            &val_file,
            Provenance {
                commit: "journal-product".to_owned(),
                lock_sha256: "0".repeat(64),
            },
            BuilderIdentity {
                host: "host".to_owned(),
                toolchain: "msvc".to_owned(),
            },
        )
        .unwrap();

        // 1. Tamper cache
        fs::write(&cache_file, b"tampered cache").unwrap();
        let err = verify_rfdetr_windows_assembly(
            &receipt_file,
            &output_root,
            &evidence_file,
            &cache_file,
            &vcxproj_file,
            &subprocess_file,
        )
        .unwrap_err();
        assert!(err.to_string().contains("CMake cache digest mismatch"));
        fs::write(&cache_file, sample_valid_cmake_cache()).unwrap();

        // 2. Tamper vcxproj
        fs::write(&vcxproj_file, b"tampered vcxproj").unwrap();
        let err = verify_rfdetr_windows_assembly(
            &receipt_file,
            &output_root,
            &evidence_file,
            &cache_file,
            &vcxproj_file,
            &subprocess_file,
        )
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("Build option evidence digest mismatch")
        );
        fs::write(&vcxproj_file, sample_valid_vcxproj()).unwrap();

        // 3. Tamper log
        fs::write(
            temp.path().join("logs/cmake-configure.stdout"),
            b"tampered stdout",
        )
        .unwrap();
        let err = verify_rfdetr_windows_assembly(
            &receipt_file,
            &output_root,
            &evidence_file,
            &cache_file,
            &vcxproj_file,
            &subprocess_file,
        )
        .unwrap_err();
        assert!(err.to_string().contains("stdout digest mismatch"));
    }
}
