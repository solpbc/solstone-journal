// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Source-bound controlled recording and verification CLI for the Windows
//! NVIDIA GPU attestation verifier.

use std::fs;
use std::path::{Path, PathBuf};

use crate::artifact_verify::{
    ControlledBuildArtifactVerificationLimits, verify_controlled_build_artifacts,
};
use crate::controlled_build::{
    BuildConfiguration, BuilderIdentity, CONTROLLED_BUILD_RECEIPT_SCHEMA_V1,
    ControlledBuildReceipt, DependencySource, InputIdentityEntry, SourceIdentity,
    SupportingArtifactRef, ValidationReference, census_outputs, decode_controlled_build_receipt,
};
use crate::digest::sha256_hex;
use crate::nvattest_windows::{
    ArchivePin, NVATTEST_BUILD_EVIDENCE_LABEL, NVATTEST_BUILD_EVIDENCE_SCHEMA_V1,
    NVATTEST_EXE_OUTPUT_LABEL, NvattestInvocation, NvattestNetworkEvidence, NvattestRefusalEntry,
    NvattestRefusals, NvattestToolCensus, NvattestWindowsBuildEvidence, base64_encode,
    production_pins,
};
use crate::provenance::{Provenance, lock_digest, require_clean, require_commit, require_lock};

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
    pub validation: PathBuf,
    pub evidence: PathBuf,
    pub receipt: PathBuf,
    pub product_commit: String,
    pub cargo_lock_sha256: String,
    pub builder_host: String,
    pub toolchain: String,
    pub bundle_path: String,
    pub manifest_sha256: String,
    pub refusal_manifest_exit: i32,
    pub refusal_manifest_boundary: String,
    pub refusal_reuse_exit: i32,
    pub refusal_reuse_boundary: String,
    pub refusal_corrupt_exit: i32,
    pub refusal_corrupt_boundary: String,
    pub network_positive: String,
    pub network_negative: String,
    pub network_rules_remaining: u32,
}

pub fn run_cli(args: &[String]) -> Result<String, NvattestWindowsSourceError> {
    let mut iter = args.iter();
    let command = iter
        .next()
        .ok_or_else(|| NvattestWindowsSourceError::new(usage()))?;

    match command.as_str() {
        "verify-inputs" => {
            let mut source_archive = None;
            let mut bundle_archive = None;
            let mut rest = iter;
            while let Some(arg) = rest.next() {
                match arg.as_str() {
                    "--source-archive" => {
                        source_archive = rest.next().map(PathBuf::from);
                    }
                    "--bundle-archive" => {
                        bundle_archive = rest.next().map(PathBuf::from);
                    }
                    _ => {
                        return Err(NvattestWindowsSourceError::new(format!(
                            "unknown option {arg}; {usage}",
                            usage = usage()
                        )));
                    }
                }
            }
            let source_path = source_archive
                .ok_or_else(|| NvattestWindowsSourceError::new("missing --source-archive"))?;
            let bundle_path = bundle_archive
                .ok_or_else(|| NvattestWindowsSourceError::new("missing --bundle-archive"))?;

            verify_input_archives(&source_path, &bundle_path)?;
            Ok("nvattest-windows verify-inputs: ok".into())
        }
        "record" => {
            let mut repo = None;
            let mut source_archive = None;
            let mut bundle_archive = None;
            let mut output_root = None;
            let mut report = None;
            let mut census = None;
            let mut validation = None;
            let mut evidence = None;
            let mut receipt = None;
            let mut product_commit = None;
            let mut cargo_lock_sha256 = None;
            let mut builder_host = None;
            let mut toolchain = None;
            let mut bundle_path = None;
            let mut manifest_sha256 = None;
            let mut refusal_manifest_exit = None;
            let mut refusal_manifest_boundary = None;
            let mut refusal_reuse_exit = None;
            let mut refusal_reuse_boundary = None;
            let mut refusal_corrupt_exit = None;
            let mut refusal_corrupt_boundary = None;
            let mut network_positive = None;
            let mut network_negative = None;
            let mut network_rules_remaining = None;

            let mut rest = iter;
            while let Some(arg) = rest.next() {
                match arg.as_str() {
                    "--repo" => repo = rest.next().map(PathBuf::from),
                    "--source-archive" => source_archive = rest.next().map(PathBuf::from),
                    "--bundle-archive" => bundle_archive = rest.next().map(PathBuf::from),
                    "--output-root" => output_root = rest.next().map(PathBuf::from),
                    "--report" => report = rest.next().map(PathBuf::from),
                    "--census" => census = rest.next().map(PathBuf::from),
                    "--validation" => validation = rest.next().map(PathBuf::from),
                    "--evidence" => evidence = rest.next().map(PathBuf::from),
                    "--receipt" => receipt = rest.next().map(PathBuf::from),
                    "--product-commit" => product_commit = rest.next().cloned(),
                    "--cargo-lock-sha256" => cargo_lock_sha256 = rest.next().cloned(),
                    "--builder-host" => builder_host = rest.next().cloned(),
                    "--toolchain" => toolchain = rest.next().cloned(),
                    "--bundle-path" => bundle_path = rest.next().cloned(),
                    "--manifest-sha256" => manifest_sha256 = rest.next().cloned(),
                    "--refusal-manifest-exit" => {
                        let val = rest.next().ok_or_else(|| {
                            NvattestWindowsSourceError::new(
                                "missing value for --refusal-manifest-exit",
                            )
                        })?;
                        refusal_manifest_exit = Some(val.parse::<i32>().map_err(|e| {
                            NvattestWindowsSourceError::new(format!(
                                "invalid integer for --refusal-manifest-exit: {e}"
                            ))
                        })?);
                    }
                    "--refusal-manifest-boundary" => {
                        refusal_manifest_boundary = rest.next().cloned()
                    }
                    "--refusal-reuse-exit" => {
                        let val = rest.next().ok_or_else(|| {
                            NvattestWindowsSourceError::new(
                                "missing value for --refusal-reuse-exit",
                            )
                        })?;
                        refusal_reuse_exit = Some(val.parse::<i32>().map_err(|e| {
                            NvattestWindowsSourceError::new(format!(
                                "invalid integer for --refusal-reuse-exit: {e}"
                            ))
                        })?);
                    }
                    "--refusal-reuse-boundary" => refusal_reuse_boundary = rest.next().cloned(),
                    "--refusal-corrupt-exit" => {
                        let val = rest.next().ok_or_else(|| {
                            NvattestWindowsSourceError::new(
                                "missing value for --refusal-corrupt-exit",
                            )
                        })?;
                        refusal_corrupt_exit = Some(val.parse::<i32>().map_err(|e| {
                            NvattestWindowsSourceError::new(format!(
                                "invalid integer for --refusal-corrupt-exit: {e}"
                            ))
                        })?);
                    }
                    "--refusal-corrupt-boundary" => refusal_corrupt_boundary = rest.next().cloned(),
                    "--network-positive" => network_positive = rest.next().cloned(),
                    "--network-negative" => network_negative = rest.next().cloned(),
                    "--network-rules-remaining" => {
                        let val = rest.next().ok_or_else(|| {
                            NvattestWindowsSourceError::new(
                                "missing value for --network-rules-remaining",
                            )
                        })?;
                        network_rules_remaining = Some(val.parse::<u32>().map_err(|e| {
                            NvattestWindowsSourceError::new(format!(
                                "invalid integer for --network-rules-remaining: {e}"
                            ))
                        })?);
                    }
                    _ => {
                        return Err(NvattestWindowsSourceError::new(format!(
                            "unknown option {arg}; {usage}",
                            usage = usage()
                        )));
                    }
                }
            }

            let record_args = RecordNvattestArgs {
                repo: repo.ok_or_else(|| NvattestWindowsSourceError::new("missing --repo"))?,
                source_archive: source_archive
                    .ok_or_else(|| NvattestWindowsSourceError::new("missing --source-archive"))?,
                bundle_archive: bundle_archive
                    .ok_or_else(|| NvattestWindowsSourceError::new("missing --bundle-archive"))?,
                output_root: output_root
                    .ok_or_else(|| NvattestWindowsSourceError::new("missing --output-root"))?,
                report: report
                    .ok_or_else(|| NvattestWindowsSourceError::new("missing --report"))?,
                census: census
                    .ok_or_else(|| NvattestWindowsSourceError::new("missing --census"))?,
                validation: validation
                    .ok_or_else(|| NvattestWindowsSourceError::new("missing --validation"))?,
                evidence: evidence
                    .ok_or_else(|| NvattestWindowsSourceError::new("missing --evidence"))?,
                receipt: receipt
                    .ok_or_else(|| NvattestWindowsSourceError::new("missing --receipt"))?,
                product_commit: product_commit
                    .ok_or_else(|| NvattestWindowsSourceError::new("missing --product-commit"))?,
                cargo_lock_sha256: cargo_lock_sha256.ok_or_else(|| {
                    NvattestWindowsSourceError::new("missing --cargo-lock-sha256")
                })?,
                builder_host: builder_host
                    .ok_or_else(|| NvattestWindowsSourceError::new("missing --builder-host"))?,
                toolchain: toolchain
                    .ok_or_else(|| NvattestWindowsSourceError::new("missing --toolchain"))?,
                bundle_path: bundle_path
                    .ok_or_else(|| NvattestWindowsSourceError::new("missing --bundle-path"))?,
                manifest_sha256: manifest_sha256
                    .ok_or_else(|| NvattestWindowsSourceError::new("missing --manifest-sha256"))?,
                refusal_manifest_exit: refusal_manifest_exit.ok_or_else(|| {
                    NvattestWindowsSourceError::new("missing --refusal-manifest-exit")
                })?,
                refusal_manifest_boundary: refusal_manifest_boundary.ok_or_else(|| {
                    NvattestWindowsSourceError::new("missing --refusal-manifest-boundary")
                })?,
                refusal_reuse_exit: refusal_reuse_exit.ok_or_else(|| {
                    NvattestWindowsSourceError::new("missing --refusal-reuse-exit")
                })?,
                refusal_reuse_boundary: refusal_reuse_boundary.ok_or_else(|| {
                    NvattestWindowsSourceError::new("missing --refusal-reuse-boundary")
                })?,
                refusal_corrupt_exit: refusal_corrupt_exit.ok_or_else(|| {
                    NvattestWindowsSourceError::new("missing --refusal-corrupt-exit")
                })?,
                refusal_corrupt_boundary: refusal_corrupt_boundary.ok_or_else(|| {
                    NvattestWindowsSourceError::new("missing --refusal-corrupt-boundary")
                })?,
                network_positive: network_positive
                    .ok_or_else(|| NvattestWindowsSourceError::new("missing --network-positive"))?,
                network_negative: network_negative
                    .ok_or_else(|| NvattestWindowsSourceError::new("missing --network-negative"))?,
                network_rules_remaining: network_rules_remaining.ok_or_else(|| {
                    NvattestWindowsSourceError::new("missing --network-rules-remaining")
                })?,
            };

            let receipt_display = record_args.receipt.display().to_string();
            record_nvattest_build(record_args)?;
            Ok(format!(
                "nvattest-windows record: receipt written to {receipt_display}"
            ))
        }
        "verify" => {
            let mut receipt = None;
            let mut output_root = None;
            let mut rest = iter;
            while let Some(arg) = rest.next() {
                match arg.as_str() {
                    "--receipt" => receipt = rest.next().map(PathBuf::from),
                    "--output-root" => output_root = rest.next().map(PathBuf::from),
                    _ => {
                        return Err(NvattestWindowsSourceError::new(format!(
                            "unknown option {arg}; {usage}",
                            usage = usage()
                        )));
                    }
                }
            }
            let receipt_path =
                receipt.ok_or_else(|| NvattestWindowsSourceError::new("missing --receipt"))?;
            let out_root = output_root
                .ok_or_else(|| NvattestWindowsSourceError::new("missing --output-root"))?;

            verify_nvattest_build(&receipt_path, &out_root)?;
            Ok(format!(
                "nvattest-windows verify: output at {} matches receipt {}",
                out_root.display(),
                receipt_path.display()
            ))
        }
        _ => Err(NvattestWindowsSourceError::new(usage())),
    }
}

pub fn require_pinned_archive(
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

pub fn verify_input_archives(
    source_path: &Path,
    bundle_path: &Path,
) -> Result<(), NvattestWindowsSourceError> {
    let pins = production_pins();
    let source_bytes = fs::read(source_path).map_err(|e| {
        NvattestWindowsSourceError::new(format!("source-archive: {}: {e}", source_path.display()))
    })?;
    require_pinned_archive("source-archive", &source_bytes, &pins.source_archive)?;

    let bundle_bytes = fs::read(bundle_path).map_err(|e| {
        NvattestWindowsSourceError::new(format!("bundle-archive: {}: {e}", bundle_path.display()))
    })?;
    require_pinned_archive("bundle-archive", &bundle_bytes, &pins.bundle_archive)?;

    Ok(())
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
    let mut command = std::process::Command::new("git");
    command.args(args);
    command.current_dir(root);
    let output = command
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

pub fn record_nvattest_build(args: RecordNvattestArgs) -> Result<(), NvattestWindowsSourceError> {
    // 1. Provenance check first before any archive read or file creation
    verify_checkout_provenance(&args.repo, &args.product_commit, &args.cargo_lock_sha256)?;

    // 2. Check manifest_sha256 against production_pins() before any write
    let pins = production_pins();
    if args.manifest_sha256 != pins.manifest_sha256 {
        return Err(NvattestWindowsSourceError::new(
            "bundle-archive: manifest_sha256 differs from pin",
        ));
    }

    // 3. Verify input archives against pins
    verify_input_archives(&args.source_archive, &args.bundle_archive)?;

    // 4. Read report raw bytes (keeping BOM) and base64-encode
    let report_bytes = fs::read(&args.report)
        .map_err(|e| NvattestWindowsSourceError::new(format!("failed to read report: {e}")))?;
    let report_base64 = base64_encode(&report_bytes);

    // 5. Read census JSON
    let census_bytes = fs::read(&args.census)
        .map_err(|e| NvattestWindowsSourceError::new(format!("failed to read census: {e}")))?;
    let census: NvattestToolCensus = serde_json::from_slice(&census_bytes).map_err(|e| {
        NvattestWindowsSourceError::new(format!("failed to parse census JSON: {e}"))
    })?;

    // 6. Build evidence document
    let evidence = NvattestWindowsBuildEvidence {
        schema: NVATTEST_BUILD_EVIDENCE_SCHEMA_V1.into(),
        report_base64,
        census,
        invocation: NvattestInvocation {
            offline: true,
            bundle_path: args.bundle_path,
            manifest_sha256: args.manifest_sha256,
        },
        refusals: NvattestRefusals {
            manifest_digest: NvattestRefusalEntry {
                exit_code: args.refusal_manifest_exit,
                boundary: args.refusal_manifest_boundary,
            },
            reuse_dependencies: NvattestRefusalEntry {
                exit_code: args.refusal_reuse_exit,
                boundary: args.refusal_reuse_boundary,
            },
            corrupt_member: NvattestRefusalEntry {
                exit_code: args.refusal_corrupt_exit,
                boundary: args.refusal_corrupt_boundary,
            },
        },
        network: NvattestNetworkEvidence {
            positive_control: args.network_positive,
            negative_control: args.network_negative,
            rules_remaining: args.network_rules_remaining,
        },
    };

    let evidence_bytes = serde_json::to_vec_pretty(&evidence)
        .map_err(|e| NvattestWindowsSourceError::new(format!("evidence serialize error: {e}")))?;

    // 7. Read validation file
    let validation_bytes = fs::read(&args.validation)
        .map_err(|e| NvattestWindowsSourceError::new(format!("failed to read validation: {e}")))?;

    // 8. Census output executable
    let exe_path = args.output_root.join(NVATTEST_EXE_OUTPUT_LABEL);
    let exe_bytes = fs::read(&exe_path).map_err(|e| {
        NvattestWindowsSourceError::new(format!("failed to read {}: {e}", exe_path.display()))
    })?;
    let outputs = census_outputs(&[(NVATTEST_EXE_OUTPUT_LABEL, exe_bytes.as_slice())])
        .map_err(|e| NvattestWindowsSourceError::new(e.to_string()))?;

    // 9. Build receipt
    let receipt = ControlledBuildReceipt {
        schema: CONTROLLED_BUILD_RECEIPT_SCHEMA_V1.into(),
        source: SourceIdentity {
            product: Provenance {
                commit: args.product_commit,
                lock_sha256: args.cargo_lock_sha256,
            },
            windows_dependency: DependencySource {
                repository: pins.sdk_repo.into(),
                revision: pins.sdk_revision.into(),
                content_sha256: pins.source_archive.sha256.into(),
            },
        },
        inputs: vec![
            InputIdentityEntry {
                label: "source.tar".into(),
                sha256: pins.source_archive.sha256.into(),
                size: pins.source_archive.bytes,
            },
            InputIdentityEntry {
                label: "bundle.tar".into(),
                sha256: pins.bundle_archive.sha256.into(),
                size: pins.bundle_archive.bytes,
            },
        ],
        builder: BuilderIdentity {
            host: args.builder_host,
            toolchain: args.toolchain,
        },
        configuration: BuildConfiguration {
            target_triple: "x86_64-pc-windows-msvc".into(),
            profile: "Release".into(),
            flags: vec![],
            network_access_denied: true,
        },
        outputs,
        supporting: vec![SupportingArtifactRef {
            label: NVATTEST_BUILD_EVIDENCE_LABEL.into(),
            sha256: sha256_hex(&evidence_bytes),
        }],
        validation: ValidationReference {
            description: "nvattest-windows-validation".into(),
            sha256: sha256_hex(&validation_bytes),
        },
    };

    let receipt_json = serde_json::to_vec_pretty(&receipt)
        .map_err(|e| NvattestWindowsSourceError::new(format!("receipt serialize error: {e}")))?;

    // 10. Write evidence, then receipt
    fs::write(&args.evidence, evidence_bytes)
        .map_err(|e| NvattestWindowsSourceError::new(format!("failed to write evidence: {e}")))?;
    fs::write(&args.receipt, receipt_json)
        .map_err(|e| NvattestWindowsSourceError::new(format!("failed to write receipt: {e}")))?;

    Ok(())
}

pub fn verify_nvattest_build(
    receipt_path: &Path,
    output_root: &Path,
) -> Result<(), NvattestWindowsSourceError> {
    let receipt_bytes = fs::read(receipt_path)
        .map_err(|e| NvattestWindowsSourceError::new(format!("failed to read receipt: {e}")))?;
    let receipt = decode_controlled_build_receipt(&receipt_bytes)
        .map_err(|e| NvattestWindowsSourceError::new(e.to_string()))?;

    verify_controlled_build_artifacts(
        output_root,
        &receipt,
        ControlledBuildArtifactVerificationLimits::new(16, 2, 128 * 1024 * 1024),
    )
    .map_err(|e| NvattestWindowsSourceError::new(e.to_string()))?;

    let exe_path = output_root.join(NVATTEST_EXE_OUTPUT_LABEL);
    let exe_bytes = fs::read(&exe_path)
        .map_err(|e| NvattestWindowsSourceError::new(format!("failed to read output exe: {e}")))?;
    if receipt.outputs.len() != 1 || receipt.outputs[0].label != NVATTEST_EXE_OUTPUT_LABEL {
        return Err(NvattestWindowsSourceError::new(
            "report-output: receipt outputs must have exactly bin/nvattest.exe",
        ));
    }
    let expected = &receipt.outputs[0];
    if exe_bytes.len() as u64 != expected.size
        || sha256_hex(&exe_bytes) != expected.pre_signing_sha256
    {
        return Err(NvattestWindowsSourceError::new(
            "report-output: output exe does not match receipt output identity",
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn require_pinned_archive_refuses_flipped_bytes() {
        let bytes = b"test";
        let pin = ArchivePin {
            bytes: 4,
            sha256: "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08",
        };
        assert!(require_pinned_archive("source-archive", bytes, &pin).is_ok());

        let flipped = b"tesT";
        let err_source = require_pinned_archive("source-archive", flipped, &pin).unwrap_err();
        assert!(
            err_source.message.contains("source-archive"),
            "{err_source}"
        );

        let err_bundle = require_pinned_archive("bundle-archive", flipped, &pin).unwrap_err();
        assert!(
            err_bundle.message.contains("bundle-archive"),
            "{err_bundle}"
        );
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn nvattest_product_identity_cannot_be_restamped_over_dirty_or_other_source() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        let _ = git_command(&repo, &["init", "."]).unwrap();
        let _ = git_command(&repo, &["config", "user.email", "tester@example.com"]).unwrap();
        let _ = git_command(&repo, &["config", "user.name", "Tester"]).unwrap();

        fs::create_dir_all(repo.join("core")).unwrap();
        fs::write(repo.join("core/Cargo.lock"), b"lockfile content\n").unwrap();
        let _ = git_command(&repo, &["add", "."]).unwrap();
        let _ = git_command(&repo, &["commit", "-m", "initial"]).unwrap();

        let commit = git_command(&repo, &["rev-parse", "HEAD"]).unwrap();
        let commit = commit.trim().to_string();
        let lock_digest_val = lock_digest(&repo.join("core/Cargo.lock")).unwrap();

        let work = temp.path().join("work");
        fs::create_dir_all(&work).unwrap();

        let make_args = |c: String, l: String, src: PathBuf, rct: PathBuf| RecordNvattestArgs {
            repo: repo.clone(),
            source_archive: src,
            bundle_archive: work.join("bundle.tar"),
            output_root: work.join("output"),
            report: work.join("report.json"),
            census: work.join("census.json"),
            validation: work.join("validation.log"),
            evidence: work.join("evidence.json"),
            receipt: rct,
            product_commit: c,
            cargo_lock_sha256: l,
            builder_host: "test-host".into(),
            toolchain: "MSVC".into(),
            bundle_path: "bundle.tar".into(),
            manifest_sha256: production_pins().manifest_sha256.into(),
            refusal_manifest_exit: 1,
            refusal_manifest_boundary: "boundary".into(),
            refusal_reuse_exit: 1,
            refusal_reuse_boundary: "boundary".into(),
            refusal_corrupt_exit: 1,
            refusal_corrupt_boundary: "boundary".into(),
            network_positive: "connected".into(),
            network_negative: "refused".into(),
            network_rules_remaining: 0,
        };

        // 1. Wrong commit
        let receipt1 = work.join("receipt1.json");
        let err1 = record_nvattest_build(make_args(
            "0".repeat(40),
            lock_digest_val.clone(),
            work.join("source.tar"),
            receipt1.clone(),
        ))
        .unwrap_err();
        assert!(err1.message.contains("mismatched-commit"), "{err1}");
        assert!(!receipt1.exists());

        // 2. Wrong lock
        let receipt2 = work.join("receipt2.json");
        let err2 = record_nvattest_build(make_args(
            commit.clone(),
            "0".repeat(64),
            work.join("source.tar"),
            receipt2.clone(),
        ))
        .unwrap_err();
        assert!(err2.message.contains("stale-lock"), "{err2}");
        assert!(!receipt2.exists());

        // 3. Dirty tree
        fs::write(repo.join("untracked.txt"), b"dirty").unwrap();
        let receipt3 = work.join("receipt3.json");
        let err3 = record_nvattest_build(make_args(
            commit.clone(),
            lock_digest_val.clone(),
            work.join("source.tar"),
            receipt3.clone(),
        ))
        .unwrap_err();
        assert!(err3.message.contains("dirty-tree"), "{err3}");
        assert!(!receipt3.exists());

        // 4. Clean repo, but tiny source archive
        fs::remove_file(repo.join("untracked.txt")).unwrap();
        let tiny_source = work.join("source_tiny.tar");
        fs::write(&tiny_source, b"tiny").unwrap();
        let receipt4 = work.join("receipt4.json");
        let err4 = record_nvattest_build(make_args(
            commit,
            lock_digest_val,
            tiny_source,
            receipt4.clone(),
        ))
        .unwrap_err();
        assert!(err4.message.contains("source-archive"), "{err4}");
        assert!(!receipt4.exists());
    }
}
