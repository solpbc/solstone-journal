// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Receipt admission for the controlled native Windows FFmpeg build.
//!
//! The native slot builds the two final Rust PEs with MSVC. This module reads
//! the build script's canonical configure receipts after that compilation,
//! binds them to the exact source and tool archives, proves the resulting PE
//! import tables do not retain an FFmpeg DLL edge, and persists one normal
//! controlled-build receipt. It does not configure, compile, sign, package,
//! or execute an artifact.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use solstone_core_ffmpeg_build_support::{
    ConfigureReceipt, EVIDENCE_DIR, controlled_component_args_for_audio_remux,
    controlled_component_inventory_for_audio_remux, parse_ffmpeg_pin, read_configure_receipt,
    read_current_run_record, sha256_hex, validate_controlled_component_args,
    validate_controlled_component_inventory,
};

use crate::acquire;
use crate::artifact_verify::{
    ControlledBuildArtifactVerificationLimits, verify_persisted_controlled_build_artifacts,
};
use crate::controlled_build::{
    BuildConfiguration, BuilderIdentity, ControlledBuildReceipt, ControlledBuildReceiptPublication,
    DependencySource, InputIdentityEntry, SourceIdentity, SupportingArtifactRef,
    ValidationReference, census_outputs, write_controlled_build_receipt_exclusive,
};
use crate::pe::{self, PeInfo};
use crate::provenance::Provenance;

pub const FFMPEG_WINDOWS_TARGET_TRIPLE: &str = "x86_64-pc-windows-msvc";
pub const FFMPEG_WINDOWS_BUILD_PROFILE: &str = "release";
pub const FFMPEG_WINDOWS_CORE_OUTPUT_LABEL: &str = "bin/solstone-core.exe";
pub const FFMPEG_WINDOWS_DESCRIBE_OUTPUT_LABEL: &str = "bin/solstone-core-describe.exe";
pub const FFMPEG_WINDOWS_BUILD_EVIDENCE_LABEL: &str =
    "provenance/windows-x86_64/ffmpeg-build-evidence.json";
pub const FFMPEG_WINDOWS_BUILD_EVIDENCE_SCHEMA_V1: &str =
    "solstone.ffmpeg-windows-build-evidence.v1";

const FFMPEG_WINDOWS_OUTPUT_TREE_ENTRIES: usize = 3;
const FFMPEG_WINDOWS_OUTPUT_LIMIT: usize = 3 * 1024 * 1024 * 1024;
const FFMPEG_RETAINED_IMPORT_PREFIXES: &[&str] = &[
    "avcodec",
    "avdevice",
    "avfilter",
    "avformat",
    "avutil",
    "avresample",
    "postproc",
    "swresample",
    "swscale",
    "ffmpeg",
];

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FfmpegConfigureEvidence {
    pub content_sha256: String,
    pub fingerprint: String,
    pub components: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FfmpegWindowsBuildEvidence {
    pub schema: String,
    pub source_archive: InputIdentityEntry,
    pub msys2_base: InputIdentityEntry,
    pub make: InputIdentityEntry,
    pub nasm: InputIdentityEntry,
    pub llvm: InputIdentityEntry,
    pub audio_remux: FfmpegConfigureEvidence,
    pub video_decode: FfmpegConfigureEvidence,
}

pub struct FfmpegWindowsBuildRecordArgs<'a> {
    pub repo_root: &'a Path,
    pub source_archive: &'a Path,
    pub msys2_archive: &'a Path,
    pub make_archive: &'a Path,
    pub nasm_archive: &'a Path,
    pub llvm_archive: &'a Path,
    pub audio_evidence_dir: &'a Path,
    pub video_evidence_dir: &'a Path,
    pub output_root: &'a Path,
    pub evidence_path: &'a Path,
    pub receipt_path: &'a Path,
    pub validation_path: &'a Path,
    pub product: Provenance,
    pub builder: BuilderIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FfmpegWindowsBuildRecord {
    pub receipt: ControlledBuildReceipt,
    pub evidence_sha256: String,
    pub receipt_publication: FfmpegWindowsReceiptPublication,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfmpegWindowsReceiptPublication {
    Durable,
    PublishedButNotDurable,
}

impl FfmpegWindowsReceiptPublication {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Durable => "durable",
            Self::PublishedButNotDurable => "published-but-not-durable",
        }
    }
}

#[derive(Debug)]
pub struct FfmpegWindowsError {
    message: String,
}

impl FfmpegWindowsError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for FfmpegWindowsError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for FfmpegWindowsError {}

impl From<std::io::Error> for FfmpegWindowsError {
    fn from(source: std::io::Error) -> Self {
        Self::new(source.to_string())
    }
}

pub fn usage() -> &'static str {
    "usage: solstone-distribution ffmpeg-windows <verify-inputs|record|verify> [FLAG]"
}

pub fn run_cli(args: &[String]) -> Result<String, FfmpegWindowsError> {
    let Some((operation, rest)) = args.split_first() else {
        return Err(FfmpegWindowsError::new(usage()));
    };
    let flags = parse_flags(rest)?;
    match operation.as_str() {
        "help" | "--help" | "-h" => {
            require_only(&flags, &[])?;
            Ok(usage().to_owned())
        }
        "verify-inputs" => {
            let repo_root = repository_root()?;
            let source_archive = required_path(&flags, "--source-archive")?;
            let msys2_archive = required_path(&flags, "--msys2-archive")?;
            let make_archive = required_path(&flags, "--make-archive")?;
            let nasm_archive = required_path(&flags, "--nasm-archive")?;
            let llvm_archive = required_path(&flags, "--llvm-archive")?;
            require_only(
                &flags,
                &[
                    "--source-archive",
                    "--msys2-archive",
                    "--make-archive",
                    "--nasm-archive",
                    "--llvm-archive",
                ],
            )?;
            let inputs = inspect_inputs(
                &repo_root,
                &source_archive,
                &msys2_archive,
                &make_archive,
                &nasm_archive,
                &llvm_archive,
            )?;
            Ok(format!(
                "FFMPEG_WINDOWS_INPUTS_OK source_sha256={} source_size={} llvm_sha256={}",
                inputs.source_archive.sha256, inputs.source_archive.size, inputs.llvm.sha256
            ))
        }
        "record" => {
            let repo_root = repository_root()?;
            let source_archive = required_path(&flags, "--source-archive")?;
            let msys2_archive = required_path(&flags, "--msys2-archive")?;
            let make_archive = required_path(&flags, "--make-archive")?;
            let nasm_archive = required_path(&flags, "--nasm-archive")?;
            let llvm_archive = required_path(&flags, "--llvm-archive")?;
            let audio_evidence_dir = required_path(&flags, "--audio-evidence-dir")?;
            let video_evidence_dir = required_path(&flags, "--video-evidence-dir")?;
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
                    "--msys2-archive",
                    "--make-archive",
                    "--nasm-archive",
                    "--llvm-archive",
                    "--audio-evidence-dir",
                    "--video-evidence-dir",
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
            let record = record_ffmpeg_windows_build(FfmpegWindowsBuildRecordArgs {
                repo_root: &repo_root,
                source_archive: &source_archive,
                msys2_archive: &msys2_archive,
                make_archive: &make_archive,
                nasm_archive: &nasm_archive,
                llvm_archive: &llvm_archive,
                audio_evidence_dir: &audio_evidence_dir,
                video_evidence_dir: &video_evidence_dir,
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
            let core = record
                .receipt
                .outputs
                .iter()
                .find(|output| output.label == FFMPEG_WINDOWS_CORE_OUTPUT_LABEL)
                .expect("FFmpeg receipt always contains core PE");
            Ok(format!(
                "FFMPEG_WINDOWS_RECORD_OK receipt={} evidence_sha256={} core_sha256={} receipt_publication={}",
                receipt_path.display(),
                record.evidence_sha256,
                core.pre_signing_sha256,
                record.receipt_publication.as_str()
            ))
        }
        "verify" => {
            let receipt = required_path(&flags, "--receipt")?;
            let output_root = required_path(&flags, "--output-root")?;
            require_only(&flags, &["--receipt", "--output-root"])?;
            let verified = verify_persisted_controlled_build_artifacts(
                &receipt,
                &output_root,
                ControlledBuildArtifactVerificationLimits::new(
                    FFMPEG_WINDOWS_OUTPUT_TREE_ENTRIES,
                    2,
                    FFMPEG_WINDOWS_OUTPUT_LIMIT,
                ),
            )
            .map_err(|source| FfmpegWindowsError::new(source.to_string()))?;
            for output in &verified.receipt().outputs {
                reject_ffmpeg_dynamic_imports(&output.census)?;
            }
            Ok(format!(
                "FFMPEG_WINDOWS_ARTIFACT_VERIFY_OK receipt={} core_sha256={}",
                receipt.display(),
                verified
                    .receipt()
                    .outputs
                    .iter()
                    .find(|output| output.label == FFMPEG_WINDOWS_CORE_OUTPUT_LABEL)
                    .expect("FFmpeg receipt always contains core PE")
                    .pre_signing_sha256
            ))
        }
        other => Err(FfmpegWindowsError::new(format!(
            "unknown FFmpeg Windows command {other:?}\n{}",
            usage()
        ))),
    }
}

pub fn record_ffmpeg_windows_build(
    args: FfmpegWindowsBuildRecordArgs<'_>,
) -> Result<FfmpegWindowsBuildRecord, FfmpegWindowsError> {
    let inputs = inspect_inputs(
        args.repo_root,
        args.source_archive,
        args.msys2_archive,
        args.make_archive,
        args.nasm_archive,
        args.llvm_archive,
    )?;
    let audio_remux = inspect_configure_evidence(args.audio_evidence_dir, true)?;
    let video_decode = inspect_configure_evidence(args.video_evidence_dir, false)?;
    let evidence = FfmpegWindowsBuildEvidence {
        schema: FFMPEG_WINDOWS_BUILD_EVIDENCE_SCHEMA_V1.to_owned(),
        source_archive: inputs.source_archive.clone(),
        msys2_base: inputs.msys2_base.clone(),
        make: inputs.make.clone(),
        nasm: inputs.nasm.clone(),
        llvm: inputs.llvm.clone(),
        audio_remux,
        video_decode,
    };
    validate_build_evidence(&evidence)?;
    let evidence_bytes = serde_json::to_vec(&evidence)
        .map_err(|source| FfmpegWindowsError::new(source.to_string()))?;
    let evidence_sha256 = sha256_hex(&evidence_bytes);
    publish_evidence_exclusive(args.evidence_path, &evidence_bytes)?;

    let core = fs::read(args.output_root.join(FFMPEG_WINDOWS_CORE_OUTPUT_LABEL))?;
    let describe = fs::read(args.output_root.join(FFMPEG_WINDOWS_DESCRIBE_OUTPUT_LABEL))?;
    let outputs = census_outputs(&[
        (FFMPEG_WINDOWS_CORE_OUTPUT_LABEL, core.as_slice()),
        (FFMPEG_WINDOWS_DESCRIBE_OUTPUT_LABEL, describe.as_slice()),
    ])
    .map_err(|source| FfmpegWindowsError::new(source.to_string()))?;
    for output in &outputs {
        if output.census.machine != pe::machine_amd64() {
            return Err(FfmpegWindowsError::new(format!(
                "FFmpeg Windows output {} is not AMD64",
                output.label
            )));
        }
        reject_ffmpeg_dynamic_imports(&output.census)?;
    }

    let validation = fs::read(args.validation_path)?;
    if validation.is_empty() {
        return Err(FfmpegWindowsError::new(
            "FFmpeg Windows validation reference is empty",
        ));
    }
    let receipt = ControlledBuildReceipt {
        schema: crate::controlled_build::CONTROLLED_BUILD_RECEIPT_SCHEMA_V1.to_owned(),
        source: SourceIdentity {
            product: args.product,
            windows_dependency: DependencySource {
                repository: "https://github.com/FFmpeg/FFmpeg".to_owned(),
                revision: ffmpeg_pin(args.repo_root)?.commit,
                content_sha256: evidence.source_archive.sha256.clone(),
            },
        },
        inputs: vec![
            evidence.source_archive.clone(),
            evidence.msys2_base.clone(),
            evidence.make.clone(),
            evidence.nasm.clone(),
            evidence.llvm.clone(),
        ],
        builder: args.builder,
        configuration: BuildConfiguration {
            target_triple: FFMPEG_WINDOWS_TARGET_TRIPLE.to_owned(),
            profile: FFMPEG_WINDOWS_BUILD_PROFILE.to_owned(),
            flags: vec![
                format!("audio-remux-configure={}", evidence.audio_remux.fingerprint),
                format!("video-decode-configure={}", evidence.video_decode.fingerprint),
                "--toolchain=msvc".to_owned(),
                "--disable-everything".to_owned(),
            ],
            network_access_denied: true,
        },
        outputs,
        supporting: vec![SupportingArtifactRef {
            label: FFMPEG_WINDOWS_BUILD_EVIDENCE_LABEL.to_owned(),
            sha256: evidence_sha256.clone(),
        }],
        validation: ValidationReference {
            description: "FFmpeg Windows network-denied MSVC build, configure receipt, and PE import admission log".to_owned(),
            sha256: sha256_hex(&validation),
        },
    };
    receipt
        .validate()
        .map_err(|source| FfmpegWindowsError::new(source.to_string()))?;
    let publication = write_controlled_build_receipt_exclusive(args.receipt_path, &receipt)
        .map_err(|source| FfmpegWindowsError::new(source.to_string()))?;
    let receipt_publication = match publication {
        ControlledBuildReceiptPublication::Durable { .. } => {
            FfmpegWindowsReceiptPublication::Durable
        }
        ControlledBuildReceiptPublication::PublishedButNotDurable { .. } => {
            FfmpegWindowsReceiptPublication::PublishedButNotDurable
        }
        ControlledBuildReceiptPublication::PublicationUnconfirmed { publication, .. } => {
            return Err(FfmpegWindowsError::new(format!(
                "FFmpeg receipt publication is unconfirmed: {publication:?}"
            )));
        }
    };
    Ok(FfmpegWindowsBuildRecord {
        receipt,
        evidence_sha256,
        receipt_publication,
    })
}

struct FfmpegWindowsInputs {
    source_archive: InputIdentityEntry,
    msys2_base: InputIdentityEntry,
    make: InputIdentityEntry,
    nasm: InputIdentityEntry,
    llvm: InputIdentityEntry,
}

fn inspect_inputs(
    repo_root: &Path,
    source_archive: &Path,
    msys2_archive: &Path,
    make_archive: &Path,
    nasm_archive: &Path,
    llvm_archive: &Path,
) -> Result<FfmpegWindowsInputs, FfmpegWindowsError> {
    let pin = ffmpeg_pin(repo_root)?;
    let source_bytes = fs::read(source_archive)?;
    if source_bytes.len() as u64 != pin.size || sha256_hex(&source_bytes) != pin.sha256 {
        return Err(FfmpegWindowsError::new(
            "FFmpeg Windows source archive does not match the admitted pin",
        ));
    }
    let tools = acquire::windows_ffmpeg_toolchain_inputs(repo_root)
        .map_err(|source| FfmpegWindowsError::new(source.to_string()))?;
    Ok(FfmpegWindowsInputs {
        source_archive: InputIdentityEntry {
            label: "sources/FFmpeg-9.0.tar.gz".to_owned(),
            sha256: sha256_hex(&source_bytes),
            size: source_bytes.len() as u64,
        },
        msys2_base: inspect_tool(msys2_archive, &tools.msys2_base, "tools/msys2-base.tar.xz")?,
        make: inspect_tool(make_archive, &tools.make, "tools/msys2-make.pkg.tar.zst")?,
        nasm: inspect_tool(nasm_archive, &tools.nasm, "tools/nasm-win64.zip")?,
        llvm: inspect_tool(llvm_archive, &tools.llvm, "tools/llvm-win64.tar.xz")?,
    })
}

fn ffmpeg_pin(
    repo_root: &Path,
) -> Result<solstone_core_ffmpeg_build_support::FfmpegPin, FfmpegWindowsError> {
    let text = fs::read_to_string(repo_root.join("core/distribution/builder-inputs.toml"))?;
    parse_ffmpeg_pin(&text).map_err(FfmpegWindowsError::new)
}

fn inspect_tool(
    path: &Path,
    expected: &acquire::WindowsFfmpegToolchainInput,
    label: &str,
) -> Result<InputIdentityEntry, FfmpegWindowsError> {
    let bytes = fs::read(path)?;
    if bytes.len() as u64 != expected.size || sha256_hex(&bytes) != expected.sha256 {
        return Err(FfmpegWindowsError::new(format!(
            "FFmpeg Windows tool {} does not match the admitted identity",
            expected.filename
        )));
    }
    Ok(InputIdentityEntry {
        label: label.to_owned(),
        sha256: expected.sha256.clone(),
        size: expected.size,
    })
}

fn inspect_configure_evidence(
    evidence_dir: &Path,
    audio_remux: bool,
) -> Result<FfmpegConfigureEvidence, FfmpegWindowsError> {
    let evidence_dir = evidence_dir.join(EVIDENCE_DIR);
    let record = read_current_run_record(&evidence_dir).map_err(FfmpegWindowsError::new)?;
    if !record.configure_executed
        || record.target != FFMPEG_WINDOWS_TARGET_TRIPLE
        || record.profile != FFMPEG_WINDOWS_BUILD_PROFILE
    {
        return Err(FfmpegWindowsError::new(
            "FFmpeg Windows configure record does not bind an executed release MSVC build",
        ));
    }
    let stored = read_configure_receipt(&evidence_dir, &record.receipt_filename)
        .map_err(FfmpegWindowsError::new)?;
    if stored.content_sha256 != record.receipt_sha256
        || stored.receipt.fingerprint != record.fingerprint
        || stored.receipt.source_sha256 != record.source_sha256
    {
        return Err(FfmpegWindowsError::new(
            "FFmpeg Windows configure receipt does not match its current-run record",
        ));
    }
    validate_configure_receipt(&stored.receipt, audio_remux)?;
    Ok(FfmpegConfigureEvidence {
        content_sha256: stored.content_sha256,
        fingerprint: stored.receipt.fingerprint,
        components: stored.receipt.components,
    })
}

fn validate_configure_receipt(
    receipt: &ConfigureReceipt,
    audio_remux: bool,
) -> Result<(), FfmpegWindowsError> {
    if receipt.target != FFMPEG_WINDOWS_TARGET_TRIPLE
        || receipt.profile != FFMPEG_WINDOWS_BUILD_PROFILE
    {
        return Err(FfmpegWindowsError::new(
            "FFmpeg Windows configure receipt has the wrong target or profile",
        ));
    }
    validate_controlled_component_args(&receipt.args).map_err(FfmpegWindowsError::new)?;
    validate_controlled_component_inventory(&receipt.components)
        .map_err(FfmpegWindowsError::new)?;
    if !controlled_component_args_for_audio_remux(audio_remux)
        .iter()
        .all(|required| receipt.args.iter().any(|arg| arg == required))
        || receipt.components.iter().map(String::as_str).ne(
            controlled_component_inventory_for_audio_remux(audio_remux)
                .iter()
                .copied(),
        )
    {
        return Err(FfmpegWindowsError::new(
            "FFmpeg Windows configure receipt has the wrong per-lane component profile",
        ));
    }
    Ok(())
}

fn validate_build_evidence(
    evidence: &FfmpegWindowsBuildEvidence,
) -> Result<(), FfmpegWindowsError> {
    if evidence.schema != FFMPEG_WINDOWS_BUILD_EVIDENCE_SCHEMA_V1 {
        return Err(FfmpegWindowsError::new(
            "unexpected FFmpeg Windows build evidence schema",
        ));
    }
    for input in [
        &evidence.source_archive,
        &evidence.msys2_base,
        &evidence.make,
        &evidence.nasm,
        &evidence.llvm,
    ] {
        if input.size == 0
            || input.sha256.len() != 64
            || !input.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(FfmpegWindowsError::new(
                "FFmpeg Windows build evidence has an invalid input identity",
            ));
        }
    }
    if evidence.audio_remux.content_sha256.len() != 64
        || evidence.video_decode.content_sha256.len() != 64
        || evidence.audio_remux.fingerprint.len() != 64
        || evidence.video_decode.fingerprint.len() != 64
        || evidence
            .audio_remux
            .components
            .iter()
            .map(String::as_str)
            .ne(controlled_component_inventory_for_audio_remux(true)
                .iter()
                .copied())
        || evidence
            .video_decode
            .components
            .iter()
            .map(String::as_str)
            .ne(controlled_component_inventory_for_audio_remux(false)
                .iter()
                .copied())
    {
        return Err(FfmpegWindowsError::new(
            "FFmpeg Windows build evidence has an invalid configure receipt binding",
        ));
    }
    Ok(())
}

fn reject_ffmpeg_dynamic_imports(info: &PeInfo) -> Result<(), FfmpegWindowsError> {
    let retained = info
        .imports
        .iter()
        .map(|library| library.name.to_ascii_lowercase())
        .filter(|name| {
            let stem = name.trim_end_matches(".dll");
            FFMPEG_RETAINED_IMPORT_PREFIXES
                .iter()
                .any(|prefix| stem.starts_with(prefix))
        })
        .collect::<Vec<_>>();
    if retained.is_empty() {
        Ok(())
    } else {
        Err(FfmpegWindowsError::new(format!(
            "FFmpeg Windows PE retains a forbidden dynamic FFmpeg import: {}",
            retained.join(", ")
        )))
    }
}

fn publish_evidence_exclusive(path: &Path, bytes: &[u8]) -> Result<(), FfmpegWindowsError> {
    let publication = solstone_core_journal_io::write_bytes_exclusive_detailed(
        path,
        bytes,
        solstone_core_journal_io::AtomicWriteOptions::default(),
    )
    .map_err(|source| FfmpegWindowsError::new(source.to_string()))?;
    if matches!(
        publication.final_name,
        solstone_core_journal_io::FinalNameConfirmation::Confirmed { .. }
    ) && matches!(
        publication.cleanup,
        solstone_core_journal_io::StageCleanup::Removed
    ) {
        Ok(())
    } else {
        Err(FfmpegWindowsError::new(format!(
            "FFmpeg Windows evidence publication is unconfirmed: {publication:?}"
        )))
    }
}

fn parse_flags(args: &[String]) -> Result<BTreeMap<String, String>, FfmpegWindowsError> {
    if !args.len().is_multiple_of(2) {
        return Err(FfmpegWindowsError::new(
            "FFmpeg Windows command flags must be name/value pairs",
        ));
    }
    let mut flags = BTreeMap::new();
    for pair in args.chunks_exact(2) {
        if !pair[0].starts_with("--") {
            return Err(FfmpegWindowsError::new(format!(
                "invalid FFmpeg Windows flag {:?}",
                pair[0]
            )));
        }
        if flags.insert(pair[0].clone(), pair[1].clone()).is_some() {
            return Err(FfmpegWindowsError::new(format!(
                "duplicate FFmpeg Windows flag {}",
                pair[0]
            )));
        }
    }
    Ok(flags)
}

fn required_text(
    flags: &BTreeMap<String, String>,
    name: &str,
) -> Result<String, FfmpegWindowsError> {
    let value = flags
        .get(name)
        .ok_or_else(|| FfmpegWindowsError::new(format!("missing required {name}")))?;
    if value.is_empty() {
        return Err(FfmpegWindowsError::new(format!(
            "required FFmpeg Windows flag {name} is empty"
        )));
    }
    Ok(value.clone())
}

fn required_path(
    flags: &BTreeMap<String, String>,
    name: &str,
) -> Result<PathBuf, FfmpegWindowsError> {
    Ok(PathBuf::from(required_text(flags, name)?))
}

fn required_lower_hex(
    flags: &BTreeMap<String, String>,
    name: &str,
    length: usize,
) -> Result<String, FfmpegWindowsError> {
    let value = required_text(flags, name)?;
    if value.len() != length
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte.is_ascii_lowercase())
    {
        return Err(FfmpegWindowsError::new(format!(
            "required FFmpeg Windows flag {name} must be {length} lowercase hexadecimal characters"
        )));
    }
    Ok(value)
}

fn require_only(
    flags: &BTreeMap<String, String>,
    admitted: &[&str],
) -> Result<(), FfmpegWindowsError> {
    if let Some(unexpected) = flags.keys().find(|name| !admitted.contains(&name.as_str())) {
        return Err(FfmpegWindowsError::new(format!(
            "unknown FFmpeg Windows flag {unexpected}"
        )));
    }
    Ok(())
}

fn repository_root() -> Result<PathBuf, FfmpegWindowsError> {
    let mut cursor = std::env::current_dir()?;
    loop {
        if cursor
            .join("core/distribution/builder-inputs.toml")
            .is_file()
        {
            return Ok(cursor);
        }
        cursor = cursor
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| FfmpegWindowsError::new("could not find builder-inputs.toml"))?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pe::{FixtureSpec, ImportSpec, PeSymbolSpec, fixture};

    #[test]
    fn dynamic_ffmpeg_imports_are_refused_but_system_imports_pass() {
        let named = [PeSymbolSpec::Named("Main")];
        let retained = [ImportSpec {
            name: "avcodec-61.dll",
            symbols: &named,
        }];
        let retained = fixture(&FixtureSpec {
            imports: &retained,
            ..FixtureSpec::default()
        });
        let info = pe::parse_pe(&retained).expect("fixture PE");
        assert!(reject_ffmpeg_dynamic_imports(&info).is_err());

        let system = [ImportSpec {
            name: "KERNEL32.dll",
            symbols: &named,
        }];
        let system = fixture(&FixtureSpec {
            imports: &system,
            ..FixtureSpec::default()
        });
        let info = pe::parse_pe(&system).expect("fixture PE");
        reject_ffmpeg_dynamic_imports(&info).expect("system import");
    }

    #[test]
    fn evidence_requires_the_two_distinct_component_profiles() {
        let input = InputIdentityEntry {
            label: "fixture".to_owned(),
            sha256: "a".repeat(64),
            size: 1,
        };
        let audio = FfmpegConfigureEvidence {
            content_sha256: "b".repeat(64),
            fingerprint: "c".repeat(64),
            components: controlled_component_inventory_for_audio_remux(true)
                .iter()
                .map(|item| (*item).to_owned())
                .collect(),
        };
        let video = FfmpegConfigureEvidence {
            content_sha256: "d".repeat(64),
            fingerprint: "e".repeat(64),
            components: controlled_component_inventory_for_audio_remux(false)
                .iter()
                .map(|item| (*item).to_owned())
                .collect(),
        };
        let evidence = FfmpegWindowsBuildEvidence {
            schema: FFMPEG_WINDOWS_BUILD_EVIDENCE_SCHEMA_V1.to_owned(),
            source_archive: input.clone(),
            msys2_base: input.clone(),
            make: input.clone(),
            nasm: input.clone(),
            llvm: input,
            audio_remux: audio,
            video_decode: video,
        };
        validate_build_evidence(&evidence).expect("profile evidence");
        let mut swapped = evidence;
        swapped.video_decode.components = swapped.audio_remux.components.clone();
        assert!(validate_build_evidence(&swapped).is_err());
    }
}
