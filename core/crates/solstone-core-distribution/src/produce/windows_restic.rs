// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Retain original restic controlled-source build documents and output bytes.

use std::collections::BTreeMap;
use std::path::Path;

use super::windows_build::read_bounded;
use super::windows_inputs::{AdmittedControlledInput, verify_receipt_document_binding};
use crate::controlled_build::{
    ControlledBuildReceipt, ValidationReference, decode_controlled_build_receipt,
};
use crate::digest::sha256_hex;
use crate::restic_windows::{self as recipe, ResticWindowsBuildEvidence};

pub struct ResticInputPaths<'a> {
    pub receipt: &'a Path,
    pub evidence: &'a Path,
    pub validation: &'a Path,
    pub source_archive: &'a Path,
    pub go_archive: &'a Path,
    pub modules_archive: &'a Path,
    pub output_root: &'a Path,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResticValidation {
    pub schema: String,
    pub build_log: String,
    pub go_build_info: String,
    pub network_denied_tools: Vec<String>,
}

pub fn validate_capture(
    source: &[u8],
    go: &[u8],
    modules: &[u8],
    evidence: &ResticWindowsBuildEvidence,
    validation: &ResticValidation,
) -> Result<Vec<crate::controlled_build::InputIdentityEntry>, String> {
    let inspected = crate::restic_windows_inputs::inspect(source, go, modules)?;
    evidence.validate(&inspected.tools)?;
    if validation.schema != "solstone.restic-windows-validation.v1"
        || sha256_hex(validation.build_log.as_bytes()) != evidence.build_log_sha256
        || sha256_hex(validation.go_build_info.as_bytes()) != evidence.go_build_info_sha256
        || validation.network_denied_tools != evidence.network_denied_tools
        || !validation
            .build_log
            .lines()
            .any(|line| line == "RESTIC_WINDOWS_BUILD_EXIT=0")
    {
        return Err("restic validation does not bind the successful fenced build".into());
    }
    crate::restic_windows_inputs::validate_build_info(
        validation.go_build_info.as_bytes(),
        &inspected.go_sum,
    )?;
    Ok(inspected.tools)
}

pub fn expected_receipt(
    original: &ControlledBuildReceipt,
    evidence: &ResticWindowsBuildEvidence,
    tools: &[crate::controlled_build::InputIdentityEntry],
    validation_bytes: &[u8],
    output_bytes: &[u8],
) -> Result<ControlledBuildReceipt, String> {
    let mut draft = recipe::assemble_receipt_draft(
        original.source.clone(),
        evidence,
        tools,
        original.builder.clone(),
        output_bytes,
    )?;
    draft.schema = Some(crate::controlled_build::CONTROLLED_BUILD_RECEIPT_SCHEMA_V1.into());
    draft.validation = Some(ValidationReference {
        description: "restic Windows ARM64 network-denied source build and Go/PE inspection".into(),
        sha256: sha256_hex(validation_bytes),
    });
    draft.validate().map_err(|e| e.to_string())
}

// This factory has one flat output. The shared artifact verifier checks named
// files; this recipe also refuses any undeclared file or directory.
fn read_output(root: &Path) -> Result<Vec<u8>, String> {
    let mut entries = std::fs::read_dir(root).map_err(|e| e.to_string())?;
    let entry = entries
        .next()
        .transpose()
        .map_err(|e| e.to_string())?
        .ok_or("restic output is missing")?;
    if entry.file_name() != std::ffi::OsStr::new(recipe::OUTPUT_LABEL)
        || entries.next().is_some()
        || !entry.file_type().map_err(|e| e.to_string())?.is_file()
    {
        return Err("restic output root must contain exactly restic.exe".into());
    }
    read_bounded(&root.join(recipe::OUTPUT_LABEL), 64 * 1024 * 1024)
}

pub fn admit(paths: ResticInputPaths<'_>) -> Result<AdmittedControlledInput, String> {
    let receipt_bytes = read_bounded(paths.receipt, 4 * 1024 * 1024)?;
    let receipt = decode_controlled_build_receipt(&receipt_bytes).map_err(|e| e.to_string())?;
    let evidence_bytes = read_bounded(paths.evidence, 4 * 1024 * 1024)?;
    let evidence: ResticWindowsBuildEvidence =
        serde_json::from_slice(&evidence_bytes).map_err(|e| e.to_string())?;
    let validation_bytes = read_bounded(paths.validation, 4 * 1024 * 1024)?;
    let validation: ResticValidation =
        serde_json::from_slice(&validation_bytes).map_err(|e| e.to_string())?;
    verify_receipt_document_binding(
        &receipt,
        &evidence_bytes,
        &validation_bytes,
        recipe::EVIDENCE_LABEL,
    )?;
    let source = read_bounded(paths.source_archive, recipe::SOURCE_BYTES)?;
    let go = read_bounded(paths.go_archive, recipe::GO_BYTES)?;
    let modules = read_bounded(paths.modules_archive, recipe::MODULES_BYTES)?;
    let tools = validate_capture(&source, &go, &modules, &evidence, &validation)?;
    crate::artifact_verify::verify_controlled_build_artifacts(
        paths.output_root,
        &receipt,
        crate::artifact_verify::ControlledBuildArtifactVerificationLimits::new(
            1,
            0,
            64 * 1024 * 1024,
        ),
    )
    .map_err(|e| e.to_string())?;
    let output = read_output(paths.output_root)?;
    if expected_receipt(&receipt, &evidence, &tools, &validation_bytes, &output)? != receipt {
        return Err(
            "restic original receipt differs from admitted source/configuration/evidence/output"
                .into(),
        );
    }
    // The one allowed output is captured now; future staging consumes these
    // bytes rather than reopening the producer's mutable output path.
    Ok(AdmittedControlledInput::new(
        receipt,
        receipt_bytes,
        evidence_bytes,
        validation_bytes,
        BTreeMap::from([(recipe::OUTPUT_LABEL.into(), output)]),
    ))
}

const USAGE: &str = "restic-windows verify-inputs|record|verify --source-archive ABS --go-archive ABS --modules-archive ABS [FLAG VALUE]";

fn flags(args: &[String], allowed: &[&str]) -> Result<BTreeMap<String, String>, String> {
    if !args.len().is_multiple_of(2) {
        return Err(USAGE.into());
    }
    let mut values = BTreeMap::new();
    for pair in args.chunks_exact(2) {
        if !allowed.contains(&pair[0].as_str())
            || pair[1].is_empty()
            || values.insert(pair[0].clone(), pair[1].clone()).is_some()
        {
            return Err("unknown, duplicate or empty restic build flag".into());
        }
    }
    for key in allowed {
        if !values.contains_key(*key) {
            return Err(format!("missing restic build flag: {key}"));
        }
    }
    Ok(values)
}
fn absolute(values: &BTreeMap<String, String>, key: &str) -> Result<std::path::PathBuf, String> {
    let path = std::path::PathBuf::from(&values[key]);
    if !path.is_absolute()
        || path
            .components()
            .any(|c| c == std::path::Component::ParentDir)
    {
        return Err(format!("absolute restic input/output path required: {key}"));
    }
    Ok(path)
}
fn publish(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    output
        .write_all(bytes)
        .and_then(|()| output.sync_all())
        .map_err(|e| e.to_string())
}

pub fn run_cli(start: &Path, args: &[String]) -> Result<String, String> {
    let (operation, rest) = args.split_first().ok_or(USAGE)?;
    let mut allowed = vec!["--source-archive", "--go-archive", "--modules-archive"];
    match operation.as_str() {
        "verify-inputs" => {}
        "record" => allowed.extend([
            "--capture",
            "--validation",
            "--output-root",
            "--receipt",
            "--evidence",
            "--product-commit",
            "--cargo-lock-sha256",
            "--builder-host",
        ]),
        "verify" => allowed.extend(["--validation", "--output-root", "--receipt", "--evidence"]),
        _ => return Err(USAGE.into()),
    }
    let values = flags(rest, &allowed)?;
    let source_path = absolute(&values, "--source-archive")?;
    let go_path = absolute(&values, "--go-archive")?;
    let modules_path = absolute(&values, "--modules-archive")?;
    if operation == "verify" {
        let receipt = absolute(&values, "--receipt")?;
        let evidence = absolute(&values, "--evidence")?;
        let validation = absolute(&values, "--validation")?;
        let output = absolute(&values, "--output-root")?;
        let admitted = admit(ResticInputPaths {
            receipt: &receipt,
            evidence: &evidence,
            validation: &validation,
            source_archive: &source_path,
            go_archive: &go_path,
            modules_archive: &modules_path,
            output_root: &output,
        })?;
        return Ok(format!(
            "RESTIC_WINDOWS_VERIFY_PASS output_sha256={}",
            admitted.receipt().outputs[0].pre_signing_sha256
        ));
    }
    let source = read_bounded(&source_path, recipe::SOURCE_BYTES)?;
    let go = read_bounded(&go_path, recipe::GO_BYTES)?;
    let modules = read_bounded(&modules_path, recipe::MODULES_BYTES)?;
    if operation == "verify-inputs" {
        let inspected = crate::restic_windows_inputs::inspect(&source, &go, &modules)?;
        return Ok(format!(
            "RESTIC_WINDOWS_INPUTS_PASS tools={}",
            inspected.tools.len()
        ));
    }
    let inventory = crate::inventory::repository_inventory_path(start)
        .ok_or("restic recorder needs the product checkout")?;
    let repo = inventory
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .ok_or("product checkout root missing")?;
    let before = super::windows_build::capture_source(repo)?;
    if before.commit != values["--product-commit"]
        || before.lock_sha256 != values["--cargo-lock-sha256"]
    {
        return Err("restic recorder product source differs from the source-bound driver".into());
    }
    let capture = read_bounded(&absolute(&values, "--capture")?, 4 * 1024 * 1024)?;
    let evidence: ResticWindowsBuildEvidence =
        serde_json::from_slice(&capture).map_err(|e| e.to_string())?;
    let validation_bytes = read_bounded(&absolute(&values, "--validation")?, 4 * 1024 * 1024)?;
    let validation: ResticValidation =
        serde_json::from_slice(&validation_bytes).map_err(|e| e.to_string())?;
    let tools = validate_capture(&source, &go, &modules, &evidence, &validation)?;
    let output_root = absolute(&values, "--output-root")?;
    let output = read_output(&output_root)?;
    let mut draft = recipe::assemble_receipt_draft(
        recipe::source_identity(before.clone()),
        &evidence,
        &tools,
        crate::controlled_build::BuilderIdentity {
            host: values["--builder-host"].clone(),
            toolchain: "Go1.26.4 windows/amd64 -> windows/arm64".into(),
        },
        &output,
    )?;
    draft.schema = Some(crate::controlled_build::CONTROLLED_BUILD_RECEIPT_SCHEMA_V1.into());
    draft.validation = Some(ValidationReference {
        description: "restic Windows ARM64 network-denied source build and Go/PE inspection".into(),
        sha256: sha256_hex(&validation_bytes),
    });
    let receipt = draft.validate().map_err(|e| e.to_string())?;
    crate::artifact_verify::verify_controlled_build_artifacts(
        &output_root,
        &receipt,
        crate::artifact_verify::ControlledBuildArtifactVerificationLimits::new(
            1,
            0,
            64 * 1024 * 1024,
        ),
    )
    .map_err(|e| e.to_string())?;
    if super::windows_build::capture_source(repo)? != before {
        return Err("product source changed during restic recording".into());
    }
    let evidence_bytes = serde_json::to_vec(&evidence).map_err(|e| e.to_string())?;
    publish(&absolute(&values, "--evidence")?, &evidence_bytes)?;
    let publication = crate::controlled_build::write_controlled_build_receipt_exclusive(
        &absolute(&values, "--receipt")?,
        &receipt,
    )
    .map_err(|e| e.to_string())?;
    if matches!(
        publication,
        crate::controlled_build::ControlledBuildReceiptPublication::PublicationUnconfirmed { .. }
    ) {
        return Err("restic receipt publication unconfirmed".into());
    }
    Ok(format!(
        "RESTIC_WINDOWS_RECORD_PASS output_sha256={} publication={publication:?}",
        receipt.outputs[0].pre_signing_sha256
    ))
}
