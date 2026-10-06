// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Source-bound controlled recording and verification CLI for the Windows
//! NVIDIA GPU attestation verifier.
//!
//! `record` refuses a dirty checkout or one off the driver's expected commit
//! and lock, then builds the evidence and receipt in memory and runs the same
//! pure admission that `produce windows-x86_64` runs, under the committed
//! pins, before either name is created. Both are published exclusively.

use std::fs;
use std::path::{Path, PathBuf};

use crate::artifact_verify::{
    ControlledBuildArtifactVerificationLimits, verify_persisted_controlled_build_artifacts,
};
use crate::controlled_build::{
    BuildConfiguration, BuilderIdentity, CONTROLLED_BUILD_RECEIPT_SCHEMA_V1,
    ControlledBuildReceipt, ControlledBuildReceiptPublication, DependencySource,
    InputIdentityEntry, SourceIdentity, SupportingArtifactRef, ValidationReference, census_outputs,
    decode_controlled_build_receipt, encode_controlled_build_receipt,
    write_controlled_build_receipt_exclusive,
};
use crate::digest::sha256_hex;
use crate::nvattest_windows::{
    AdmissionBytes, ArchivePin, MsvcRuntimeBytes, NVATTEST_BUILD_EVIDENCE_LABEL,
    NVATTEST_BUILD_EVIDENCE_SCHEMA_V1, NVATTEST_BUILD_PROFILE, NVATTEST_BUNDLE_INPUT_LABEL,
    NVATTEST_DRIVER_CONTROLS_SCHEMA_V1, NVATTEST_EXE_OUTPUT_LABEL, NVATTEST_LICENSE_LABEL,
    NVATTEST_NOTICES_BODY_PATH, NVATTEST_NOTICES_INDEX_PATH, NVATTEST_RUNTIME_OUTPUT_LABELS,
    NVATTEST_SOURCE_INPUT_LABEL, NVATTEST_TARGET_TRIPLE, NVATTEST_VALIDATION_DESCRIPTION,
    NvattestDriverControls, NvattestDumpbinDependents, NvattestToolCensus,
    NvattestWindowsBuildEvidence, Pins, admit, base64_encode, production_pins,
};
use crate::provenance::{Provenance, lock_digest, require_clean, require_commit, require_lock};

/// Output tree bound for re-hashing the persisted executable. The deepest
/// member is `share/ca/ca-bundle.pem`; the Windows inventory counts the file
/// itself as a level, so that tree is three deep there.
pub(crate) const OUTPUT_LIMITS: ControlledBuildArtifactVerificationLimits =
    ControlledBuildArtifactVerificationLimits::new(16, 3, 128 * 1024 * 1024);

/// The dumpbin `/dependents` captures the driver writes, by file name.
pub const NVATTEST_DUMPBIN_FILES: [&str; 4] = [
    "nvattest.exe.dependents.txt",
    "msvcp140.dll.dependents.txt",
    "vcruntime140.dll.dependents.txt",
    "vcruntime140_1.dll.dependents.txt",
];

#[derive(Debug)]
pub struct NvattestWindowsSourceError {
    pub message: String,
}

impl NvattestWindowsSourceError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for NvattestWindowsSourceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for NvattestWindowsSourceError {}

impl From<String> for NvattestWindowsSourceError {
    fn from(message: String) -> Self {
        Self::new(message)
    }
}

pub fn usage() -> &'static str {
    "usage: solstone-distribution nvattest-windows <verify-inputs|record|verify> [OPTIONS]"
}

pub struct RecordNvattestArgs {
    pub repo: PathBuf,
    pub source_archive: PathBuf,
    pub bundle_archive: PathBuf,
    pub output_root: PathBuf,
    pub report: PathBuf,
    pub census: PathBuf,
    pub controls: PathBuf,
    pub dumpbin_dir: PathBuf,
    pub validation: PathBuf,
    pub evidence: PathBuf,
    pub receipt: PathBuf,
    pub product_commit: String,
    pub cargo_lock_sha256: String,
    pub builder_host: String,
    pub toolchain: String,
}

/// `--name value` pairs, each named once, every one required.
fn parse_options<'a>(
    args: impl Iterator<Item = &'a String>,
    names: &[&str],
) -> Result<Vec<String>, NvattestWindowsSourceError> {
    let mut values: Vec<Option<String>> = vec![None; names.len()];
    let mut rest = args;
    while let Some(arg) = rest.next() {
        let slot = names
            .iter()
            .position(|name| arg.strip_prefix("--") == Some(name))
            .ok_or_else(|| {
                NvattestWindowsSourceError::new(format!("unknown option {arg}; {}", usage()))
            })?;
        let value = rest
            .next()
            .ok_or_else(|| NvattestWindowsSourceError::new(format!("missing value for {arg}")))?;
        if values[slot].replace(value.clone()).is_some() {
            return Err(NvattestWindowsSourceError::new(format!(
                "duplicate option {arg}"
            )));
        }
    }
    values
        .into_iter()
        .zip(names)
        .map(|(value, name)| {
            value.ok_or_else(|| NvattestWindowsSourceError::new(format!("missing --{name}")))
        })
        .collect()
}

pub fn run_cli(args: &[String]) -> Result<String, NvattestWindowsSourceError> {
    let mut iter = args.iter();
    let command = iter
        .next()
        .ok_or_else(|| NvattestWindowsSourceError::new(usage()))?;
    match command.as_str() {
        "verify-inputs" => {
            let values = parse_options(iter, &["source-archive", "bundle-archive"])?;
            verify_input_archives(Path::new(&values[0]), Path::new(&values[1]))?;
            Ok("nvattest-windows verify-inputs: ok".into())
        }
        "record" => {
            let v = parse_options(
                iter,
                &[
                    "repo",
                    "source-archive",
                    "bundle-archive",
                    "output-root",
                    "report",
                    "census",
                    "controls",
                    "dumpbin-dir",
                    "validation",
                    "evidence",
                    "receipt",
                    "product-commit",
                    "cargo-lock-sha256",
                    "builder-host",
                    "toolchain",
                ],
            )?;
            let path = |index: usize| PathBuf::from(&v[index]);
            let args = RecordNvattestArgs {
                repo: path(0),
                source_archive: path(1),
                bundle_archive: path(2),
                output_root: path(3),
                report: path(4),
                census: path(5),
                controls: path(6),
                dumpbin_dir: path(7),
                validation: path(8),
                evidence: path(9),
                receipt: path(10),
                product_commit: v[11].clone(),
                cargo_lock_sha256: v[12].clone(),
                builder_host: v[13].clone(),
                toolchain: v[14].clone(),
            };
            let receipt_display = args.receipt.display().to_string();
            let publication = record_nvattest_build(args)?;
            Ok(format!(
                "nvattest-windows record: receipt written to {receipt_display} ({publication})"
            ))
        }
        "verify" => {
            let values =
                parse_options(iter, &["receipt", "evidence", "validation", "output-root"])?;
            let paths: Vec<&Path> = values.iter().map(Path::new).collect();
            verify_nvattest_build(paths[0], paths[1], paths[2], paths[3])?;
            Ok(format!(
                "nvattest-windows verify: output at {} matches receipt {}",
                values[3], values[0]
            ))
        }
        _ => Err(NvattestWindowsSourceError::new(usage())),
    }
}

pub(crate) fn require_pinned_archive(
    label: &str,
    bytes: &[u8],
    pin: &ArchivePin,
) -> Result<(), NvattestWindowsSourceError> {
    if bytes.len() as u64 != pin.bytes || sha256_hex(bytes) != pin.sha256 {
        return Err(NvattestWindowsSourceError::new(format!(
            "{label}: archive size or digest differs from pin"
        )));
    }
    Ok(())
}

fn read(label: &str, path: &Path) -> Result<Vec<u8>, NvattestWindowsSourceError> {
    fs::read(path)
        .map_err(|e| NvattestWindowsSourceError::new(format!("{label}: {}: {e}", path.display())))
}

fn read_pinned_archives(
    pins: &Pins,
    source_path: &Path,
    bundle_path: &Path,
) -> Result<(Vec<u8>, Vec<u8>), NvattestWindowsSourceError> {
    let source = read("source-archive", source_path)?;
    require_pinned_archive("source-archive", &source, &pins.source_archive)?;
    let bundle = read("bundle-archive", bundle_path)?;
    require_pinned_archive("bundle-archive", &bundle, &pins.bundle_archive)?;
    Ok((source, bundle))
}

pub fn verify_input_archives(
    source_path: &Path,
    bundle_path: &Path,
) -> Result<(), NvattestWindowsSourceError> {
    read_pinned_archives(&production_pins(), source_path, bundle_path).map(|_| ())
}

pub fn verify_checkout_provenance(
    repo_root: &Path,
    expected_commit: &str,
    expected_lock_sha: &str,
) -> Result<(), NvattestWindowsSourceError> {
    let status = git_command(
        repo_root,
        &[
            "status",
            "--porcelain=v1",
            "--untracked-files=all",
            "--ignore-submodules=none",
        ],
    )?;
    require_clean(!status.is_empty())
        .map_err(|e| NvattestWindowsSourceError::new(e.to_string()))?;
    let commit = git_command(repo_root, &["rev-parse", "HEAD"])?;
    require_commit(expected_commit, commit.trim())
        .map_err(|e| NvattestWindowsSourceError::new(e.to_string()))?;
    let digest = lock_digest(&repo_root.join("core/Cargo.lock"))
        .map_err(|e| NvattestWindowsSourceError::new(e.to_string()))?;
    require_lock(expected_lock_sha, &digest)
        .map_err(|e| NvattestWindowsSourceError::new(e.to_string()))?;
    Ok(())
}

fn git_command(root: &Path, args: &[&str]) -> Result<String, NvattestWindowsSourceError> {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|e| NvattestWindowsSourceError::new(format!("git {}: {e}", args.join(" "))))?;
    if !output.status.success() {
        return Err(NvattestWindowsSourceError::new(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    String::from_utf8(output.stdout)
        .map_err(|e| NvattestWindowsSourceError::new(format!("git output utf-8 error: {e}")))
}

/// Everything `record` needs besides the checkout, read and validated without
/// touching either destination.
pub(crate) struct RecordInputs<'a> {
    pub(crate) repo: &'a Path,
    pub(crate) source_archive: &'a Path,
    pub(crate) bundle_archive: &'a Path,
    pub(crate) output_root: &'a Path,
    pub(crate) report: &'a Path,
    pub(crate) census: &'a Path,
    pub(crate) controls: &'a Path,
    pub(crate) dumpbin_dir: &'a Path,
    pub(crate) validation: &'a Path,
    pub(crate) product: Provenance,
    pub(crate) builder: BuilderIdentity,
}

pub(crate) struct RecordedDocuments {
    pub(crate) receipt: ControlledBuildReceipt,
    pub(crate) receipt_bytes: Vec<u8>,
    pub(crate) evidence_bytes: Vec<u8>,
}

/// Assemble evidence and receipt exactly as they will be written, then run
/// the producer's pure admission over them under `pins`.
pub(crate) fn assemble_and_admit(
    pins: &Pins,
    inputs: &RecordInputs<'_>,
) -> Result<RecordedDocuments, NvattestWindowsSourceError> {
    let (source_bytes, bundle_bytes) =
        read_pinned_archives(pins, inputs.source_archive, inputs.bundle_archive)?;
    let report_bytes = read("report", inputs.report)?;
    let census: NvattestToolCensus = serde_json::from_slice(&read("census", inputs.census)?)
        .map_err(|e| NvattestWindowsSourceError::new(format!("census: {e}")))?;
    let controls: NvattestDriverControls =
        serde_json::from_slice(&read("controls", inputs.controls)?)
            .map_err(|e| NvattestWindowsSourceError::new(format!("controls: {e}")))?;
    if controls.schema != NVATTEST_DRIVER_CONTROLS_SCHEMA_V1 {
        return Err(NvattestWindowsSourceError::new(format!(
            "controls: schema must be {NVATTEST_DRIVER_CONTROLS_SCHEMA_V1}"
        )));
    }
    let dumpbin = NVATTEST_DUMPBIN_FILES
        .iter()
        .map(|name| read("dumpbin", &inputs.dumpbin_dir.join(name)).map(|b| base64_encode(&b)))
        .collect::<Result<Vec<_>, _>>()?;
    let validation = read("validation", inputs.validation)?;
    let output = |label: &str| {
        read(
            "output",
            &label
                .split('/')
                .fold(inputs.output_root.to_path_buf(), |path, part| {
                    path.join(part)
                }),
        )
    };
    let exe = output(NVATTEST_EXE_OUTPUT_LABEL)?;
    let license = output(NVATTEST_LICENSE_LABEL)?;
    let runtime = NVATTEST_RUNTIME_OUTPUT_LABELS
        .iter()
        .map(|label| output(label))
        .collect::<Result<Vec<_>, _>>()?;
    let notices_index = read(
        "notices-index",
        &inputs.repo.join(NVATTEST_NOTICES_INDEX_PATH),
    )?;
    let notices_body = read(
        "notices-body",
        &inputs.repo.join(NVATTEST_NOTICES_BODY_PATH),
    )?;

    let mut dumpbin = dumpbin.into_iter();
    let evidence = NvattestWindowsBuildEvidence {
        schema: NVATTEST_BUILD_EVIDENCE_SCHEMA_V1.into(),
        report_base64: base64_encode(&report_bytes),
        census,
        invocation: controls.invocation,
        refusals: controls.refusals,
        network: controls.network,
        dumpbin_dependents: NvattestDumpbinDependents {
            nvattest_exe: dumpbin.next().unwrap(),
            msvcp140: dumpbin.next().unwrap(),
            vcruntime140: dumpbin.next().unwrap(),
            vcruntime140_1: dumpbin.next().unwrap(),
        },
    };
    let evidence_bytes = serde_json::to_vec_pretty(&evidence)
        .map_err(|e| NvattestWindowsSourceError::new(format!("evidence: {e}")))?;
    let outputs = census_outputs(&[(NVATTEST_EXE_OUTPUT_LABEL, exe.as_slice())])
        .map_err(|e| NvattestWindowsSourceError::new(e.to_string()))?;
    let receipt = ControlledBuildReceipt {
        schema: CONTROLLED_BUILD_RECEIPT_SCHEMA_V1.into(),
        source: SourceIdentity {
            product: inputs.product.clone(),
            windows_dependency: DependencySource {
                repository: pins.sdk_repo.into(),
                revision: pins.sdk_revision.into(),
                content_sha256: pins.source_archive.sha256.into(),
            },
        },
        inputs: vec![
            InputIdentityEntry {
                label: NVATTEST_SOURCE_INPUT_LABEL.into(),
                sha256: pins.source_archive.sha256.into(),
                size: pins.source_archive.bytes,
            },
            InputIdentityEntry {
                label: NVATTEST_BUNDLE_INPUT_LABEL.into(),
                sha256: pins.bundle_archive.sha256.into(),
                size: pins.bundle_archive.bytes,
            },
        ],
        builder: inputs.builder.clone(),
        configuration: BuildConfiguration {
            target_triple: NVATTEST_TARGET_TRIPLE.into(),
            profile: NVATTEST_BUILD_PROFILE.into(),
            flags: vec![],
            network_access_denied: true,
        },
        outputs,
        supporting: vec![SupportingArtifactRef {
            label: NVATTEST_BUILD_EVIDENCE_LABEL.into(),
            sha256: sha256_hex(&evidence_bytes),
        }],
        validation: ValidationReference {
            description: NVATTEST_VALIDATION_DESCRIPTION.into(),
            sha256: sha256_hex(&validation),
        },
    };
    let receipt_bytes = encode_controlled_build_receipt(&receipt)
        .map_err(|e| NvattestWindowsSourceError::new(e.to_string()))?;

    // The build copies stand in for the payload's MSVC members here; admission
    // still holds them to the committed MSVC runtime pins.
    admit(
        pins,
        &AdmissionBytes {
            receipt: &receipt_bytes,
            evidence: &evidence_bytes,
            validation: &validation,
            notices_index: &notices_index,
            notices_body: &notices_body,
            source_archive: &source_bytes,
            bundle_archive: &bundle_bytes,
            output_exe: &exe,
            output_license: &license,
            output_msvcp140: &runtime[0],
            output_vcruntime140: &runtime[1],
            output_vcruntime140_1: &runtime[2],
        },
        &MsvcRuntimeBytes {
            msvcp140: &runtime[0],
            vcruntime140: &runtime[1],
            vcruntime140_1: &runtime[2],
        },
    )?;
    Ok(RecordedDocuments {
        receipt,
        receipt_bytes,
        evidence_bytes,
    })
}

/// Publish the evidence, then the receipt that binds it, both create-new.
/// A receipt that cannot be published removes the evidence this call created.
pub(crate) fn publish_documents(
    documents: &RecordedDocuments,
    evidence_path: &Path,
    receipt_path: &Path,
) -> Result<&'static str, NvattestWindowsSourceError> {
    for path in [evidence_path, receipt_path] {
        if fs::symlink_metadata(path).is_ok() {
            return Err(NvattestWindowsSourceError::new(format!(
                "destination already exists and is never replaced: {}",
                path.display()
            )));
        }
    }
    let evidence = solstone_core_journal_io::write_bytes_exclusive_detailed(
        evidence_path,
        &documents.evidence_bytes,
        solstone_core_journal_io::AtomicWriteOptions::default(),
    )
    .map_err(|e| NvattestWindowsSourceError::new(format!("evidence publication failed: {e}")))?;
    if !matches!(
        evidence.final_name,
        solstone_core_journal_io::FinalNameConfirmation::Confirmed { .. }
    ) || !matches!(
        evidence.cleanup,
        solstone_core_journal_io::StageCleanup::Removed
    ) {
        let removal = fs::remove_file(evidence_path)
            .map(|()| "removed".to_string())
            .unwrap_or_else(|e| format!("could not be removed: {e}"));
        return Err(NvattestWindowsSourceError::new(format!(
            "evidence publication is unconfirmed: {evidence:?}; evidence {removal}"
        )));
    }
    let withdraw = |message: String| {
        let removal = fs::remove_file(evidence_path)
            .map(|()| "removed".to_string())
            .unwrap_or_else(|e| format!("could not be removed: {e}"));
        NvattestWindowsSourceError::new(format!("{message}; published evidence {removal}"))
    };
    let publication = write_controlled_build_receipt_exclusive(receipt_path, &documents.receipt)
        .map_err(|e| withdraw(e.to_string()))?;
    let state = match &publication {
        ControlledBuildReceiptPublication::Durable { .. } => "durable",
        ControlledBuildReceiptPublication::PublishedButNotDurable { .. } => {
            "published-but-not-durable"
        }
        ControlledBuildReceiptPublication::PublicationUnconfirmed { publication, .. } => {
            return Err(withdraw(format!(
                "receipt publication is unconfirmed: {publication:?}"
            )));
        }
    };
    let written = read("receipt", receipt_path).map_err(|e| withdraw(e.to_string()))?;
    if written != documents.receipt_bytes {
        return Err(withdraw(
            "published receipt bytes differ from the admitted receipt; the receipt remains for inspection"
                .into(),
        ));
    }
    Ok(state)
}

pub fn record_nvattest_build(
    args: RecordNvattestArgs,
) -> Result<&'static str, NvattestWindowsSourceError> {
    // Provenance first, before any archive read or destination creation.
    verify_checkout_provenance(&args.repo, &args.product_commit, &args.cargo_lock_sha256)?;
    let documents = assemble_and_admit(
        &production_pins(),
        &RecordInputs {
            repo: &args.repo,
            source_archive: &args.source_archive,
            bundle_archive: &args.bundle_archive,
            output_root: &args.output_root,
            report: &args.report,
            census: &args.census,
            controls: &args.controls,
            dumpbin_dir: &args.dumpbin_dir,
            validation: &args.validation,
            product: Provenance {
                commit: args.product_commit,
                lock_sha256: args.cargo_lock_sha256,
            },
            builder: BuilderIdentity {
                host: args.builder_host,
                toolchain: args.toolchain,
            },
        },
    )?;
    publish_documents(&documents, &args.evidence, &args.receipt)
}

/// Re-read the persisted receipt, check it still binds the persisted evidence
/// and validation bytes, then re-hash and re-census the output it names.
pub fn verify_nvattest_build(
    receipt_path: &Path,
    evidence_path: &Path,
    validation_path: &Path,
    output_root: &Path,
) -> Result<(), NvattestWindowsSourceError> {
    let receipt = decode_controlled_build_receipt(&read("receipt", receipt_path)?)
        .map_err(|e| NvattestWindowsSourceError::new(e.to_string()))?;
    crate::produce::windows_inputs::verify_receipt_document_binding(
        &receipt,
        &read("evidence", evidence_path)?,
        &read("validation", validation_path)?,
        NVATTEST_BUILD_EVIDENCE_LABEL,
    )?;
    if receipt.outputs.len() != 1 || receipt.outputs[0].label != NVATTEST_EXE_OUTPUT_LABEL {
        return Err(NvattestWindowsSourceError::new(
            "receipt-output: receipt outputs must be exactly bin/nvattest.exe",
        ));
    }
    verify_persisted_controlled_build_artifacts(receipt_path, output_root, OUTPUT_LIMITS)
        .map_err(|e| NvattestWindowsSourceError::new(e.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nvattest_windows::test_support::{Fixture, WrittenInputs, controls};

    struct DriverFiles {
        dir: PathBuf,
        report: PathBuf,
        census: PathBuf,
        controls: PathBuf,
    }

    fn record_inputs<'a>(
        written: &'a WrittenInputs,
        files: &'a DriverFiles,
        repo: &'a Path,
    ) -> RecordInputs<'a> {
        RecordInputs {
            repo,
            source_archive: &written.source_archive,
            bundle_archive: &written.bundle_archive,
            output_root: &written.output_root,
            report: &files.report,
            census: &files.census,
            controls: &files.controls,
            dumpbin_dir: &files.dir,
            validation: &written.validation,
            product: Provenance {
                commit: "a".repeat(40),
                lock_sha256: "b".repeat(64),
            },
            builder: BuilderIdentity {
                host: "test-builder".into(),
                toolchain: "MSVC 14.44.35207".into(),
            },
        }
    }

    /// The driver's files for the fixture build, beside its output root.
    fn driver_files(fixture: &Fixture, written: &WrittenInputs) -> DriverFiles {
        let dir = written.root.path().join("driver");
        fs::create_dir_all(&dir).unwrap();
        let files = DriverFiles {
            report: dir.join("build-report.json"),
            census: dir.join("tool-census.json"),
            controls: dir.join("controls.json"),
            dir,
        };
        fs::write(&files.report, fixture.report_bytes()).unwrap();
        fs::write(
            &files.census,
            serde_json::to_vec(&fixture.evidence.census).unwrap(),
        )
        .unwrap();
        fs::write(
            &files.controls,
            serde_json::to_vec(&controls(&fixture.pins)).unwrap(),
        )
        .unwrap();
        let dumpbin = &fixture.evidence.dumpbin_dependents;
        for (name, text) in NVATTEST_DUMPBIN_FILES.iter().zip([
            &dumpbin.nvattest_exe,
            &dumpbin.msvcp140,
            &dumpbin.vcruntime140,
            &dumpbin.vcruntime140_1,
        ]) {
            fs::write(
                files.dir.join(name),
                crate::nvattest_windows::base64_decode(text).unwrap(),
            )
            .unwrap();
        }
        files
    }

    fn repo() -> &'static Path {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(3)
            .unwrap()
    }

    #[test]
    fn require_pinned_archive_refuses_one_byte_off() {
        let pin = ArchivePin {
            bytes: 4,
            sha256: "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08",
        };
        assert!(require_pinned_archive("source-archive", b"test", &pin).is_ok());
        for label in ["source-archive", "bundle-archive"] {
            let error = require_pinned_archive(label, b"tesT", &pin).unwrap_err();
            assert!(error.message.starts_with(label), "{error}");
            assert!(require_pinned_archive(label, b"tes", &pin).is_err());
        }
    }

    #[test]
    fn record_admits_and_publishes_what_produce_admits() {
        let fixture = Fixture::new();
        let written = fixture.write_inputs();
        let files = driver_files(&fixture, &written);
        let documents =
            assemble_and_admit(&fixture.pins, &record_inputs(&written, &files, repo())).unwrap();
        let evidence = written.root.path().join("out-evidence.json");
        let receipt = written.root.path().join("out-receipt.json");
        publish_documents(&documents, &evidence, &receipt).unwrap();
        assert_eq!(fs::read(&receipt).unwrap(), documents.receipt_bytes);
        verify_nvattest_build(
            &receipt,
            &evidence,
            &written.validation,
            &written.output_root,
        )
        .unwrap();
        // A second publication never replaces either name.
        assert!(publish_documents(&documents, &evidence, &receipt).is_err());
        assert_eq!(fs::read(&receipt).unwrap(), documents.receipt_bytes);
    }

    #[test]
    fn record_refuses_what_admission_refuses_before_writing_anything() {
        let mut fixture = Fixture::new();
        fixture.report.sources.pop();
        let written = fixture.write_inputs();
        let files = driver_files(&fixture, &written);
        let error = assemble_and_admit(&fixture.pins, &record_inputs(&written, &files, repo()))
            .err()
            .unwrap();
        assert!(error.message.starts_with("report-sources"), "{error}");

        // Production pins refuse the fixture archives at the archive boundary.
        let fixture = Fixture::new();
        let written = fixture.write_inputs();
        let files = driver_files(&fixture, &written);
        let error =
            assemble_and_admit(&production_pins(), &record_inputs(&written, &files, repo()))
                .err()
                .unwrap();
        assert!(error.message.starts_with("source-archive"), "{error}");
        assert!(
            fs::read_dir(written.root.path()).unwrap().all(|e| !e
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("out-"))
        );
    }

    #[test]
    fn a_receipt_that_cannot_be_published_leaves_no_evidence_behind() {
        let fixture = Fixture::new();
        let written = fixture.write_inputs();
        let files = driver_files(&fixture, &written);
        let documents =
            assemble_and_admit(&fixture.pins, &record_inputs(&written, &files, repo())).unwrap();
        let evidence = written.root.path().join("out-evidence.json");
        // The receipt's parent does not exist, so its publication fails.
        let receipt = written.root.path().join("missing-dir/out-receipt.json");
        let error = publish_documents(&documents, &evidence, &receipt).unwrap_err();
        assert!(
            error.message.contains("published evidence removed"),
            "{error}"
        );
        assert!(!evidence.exists());
        assert!(!receipt.exists());
        // An existing evidence name refuses before anything is written.
        fs::write(&evidence, b"prior").unwrap();
        let receipt = written.root.path().join("out-receipt.json");
        assert!(publish_documents(&documents, &evidence, &receipt).is_err());
        assert_eq!(fs::read(&evidence).unwrap(), b"prior");
        assert!(!receipt.exists());
    }

    #[test]
    fn verify_refuses_a_changed_output_or_unbound_documents() {
        let fixture = Fixture::new();
        let written = fixture.write_inputs();
        let files = driver_files(&fixture, &written);
        let documents =
            assemble_and_admit(&fixture.pins, &record_inputs(&written, &files, repo())).unwrap();
        let evidence = written.root.path().join("out-evidence.json");
        let receipt = written.root.path().join("out-receipt.json");
        publish_documents(&documents, &evidence, &receipt).unwrap();
        let validation = &written.validation;
        verify_nvattest_build(&receipt, &evidence, validation, &written.output_root).unwrap();

        let exe = written.output_root.join("bin/nvattest.exe");
        let original = fs::read(&exe).unwrap();
        let mut changed = original.clone();
        *changed.last_mut().unwrap() ^= 1;
        fs::write(&exe, &changed).unwrap();
        assert!(
            verify_nvattest_build(&receipt, &evidence, validation, &written.output_root).is_err()
        );
        fs::write(&exe, &original).unwrap();

        fs::write(validation, b"changed validation\n").unwrap();
        let error = verify_nvattest_build(&receipt, &evidence, validation, &written.output_root)
            .unwrap_err();
        assert!(error.message.contains("unbound-evidence"), "{error}");
    }

    #[test]
    fn cli_options_are_exact() {
        let args = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(run_cli(&args(&["verify-inputs", "--source-archive", "a"])).is_err());
        assert!(
            run_cli(&args(&[
                "verify-inputs",
                "--source-archive",
                "a",
                "--source-archive",
                "b",
                "--bundle-archive",
                "c"
            ]))
            .unwrap_err()
            .message
            .contains("duplicate")
        );
        assert!(
            run_cli(&args(&["verify-inputs", "--source-sha256", "a"]))
                .unwrap_err()
                .message
                .contains("unknown option")
        );
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn nvattest_product_identity_cannot_be_restamped_over_dirty_or_other_source() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        fs::create_dir_all(repo.join("core")).unwrap();
        git_command(&repo, &["init", "."]).unwrap();
        git_command(&repo, &["config", "user.email", "tester@example.com"]).unwrap();
        git_command(&repo, &["config", "user.name", "Tester"]).unwrap();
        fs::write(repo.join("core/Cargo.lock"), b"lockfile content\n").unwrap();
        git_command(&repo, &["add", "."]).unwrap();
        git_command(&repo, &["commit", "-m", "initial"]).unwrap();
        let commit = git_command(&repo, &["rev-parse", "HEAD"])
            .unwrap()
            .trim()
            .to_string();
        let lock = lock_digest(&repo.join("core/Cargo.lock")).unwrap();

        let work = temp.path().join("work");
        fs::create_dir_all(&work).unwrap();
        let make_args =
            |commit: String, lock: String, source: PathBuf, receipt: PathBuf| RecordNvattestArgs {
                repo: repo.clone(),
                source_archive: source,
                bundle_archive: work.join("bundle.tar"),
                output_root: work.join("output"),
                report: work.join("report.json"),
                census: work.join("census.json"),
                controls: work.join("controls.json"),
                dumpbin_dir: work.clone(),
                validation: work.join("validation.log"),
                evidence: work.join("evidence.json"),
                receipt,
                product_commit: commit,
                cargo_lock_sha256: lock,
                builder_host: "test-host".into(),
                toolchain: "MSVC".into(),
            };
        for (commit, lock, dirty, boundary) in [
            ("0".repeat(40), lock.clone(), false, "mismatched-commit"),
            (commit.clone(), "0".repeat(64), false, "stale-lock"),
            (commit.clone(), lock.clone(), true, "dirty-tree"),
        ] {
            if dirty {
                fs::write(repo.join("untracked.txt"), b"dirty").unwrap();
            }
            let receipt = work.join("receipt.json");
            let error = record_nvattest_build(make_args(
                commit,
                lock,
                work.join("source.tar"),
                receipt.clone(),
            ))
            .unwrap_err();
            assert!(error.message.contains(boundary), "{error}");
            assert!(!receipt.exists() && !work.join("evidence.json").exists());
        }
        fs::remove_file(repo.join("untracked.txt")).unwrap();
        let tiny = work.join("source_tiny.tar");
        fs::write(&tiny, b"tiny").unwrap();
        let receipt = work.join("receipt.json");
        let error =
            record_nvattest_build(make_args(commit, lock, tiny, receipt.clone())).unwrap_err();
        assert!(error.message.contains("source-archive"), "{error}");
        assert!(!receipt.exists() && !work.join("evidence.json").exists());
    }
}
