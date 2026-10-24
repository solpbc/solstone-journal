// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Source-pinned ARM CED recording and retained-byte admission.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::windows_build::{capture_source, read_bounded};
use super::windows_inputs::{
    AdmittedControlledInput, preflight_source_archive, snapshot_file,
    verify_receipt_document_binding, with_snapshot,
};
use super::windows_openmp::{self, AdmittedOpenMp, OpenMpFiles};
use crate::ced_windows::{self, CedWindowsBuildEvidence};
use crate::ced_windows_arm64::{
    self as recipe, CedWindowsArm64Evidence, CedWindowsArm64Validation,
};
use crate::ced_windows_source::{self, CedWindowsSourceArchive};
use crate::controlled_build::{
    BuilderIdentity, ControlledBuildReceipt, DependencySource, InputIdentityEntry, SourceIdentity,
    ValidationReference, decode_controlled_build_receipt,
};
use crate::digest::sha256_hex;

const DOCUMENT_LIMIT: u64 = 4 * 1024 * 1024;
const CAPTURE_SCHEMA: &str = "solstone.ced-windows-arm64-build-capture.v1";

pub struct CedArm64InputPaths<'a> {
    pub receipt: &'a Path,
    pub evidence: &'a Path,
    pub validation: &'a Path,
    pub source_archive: &'a Path,
    pub output_root: &'a Path,
    pub openmp: &'a OpenMpFiles,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeCapture {
    schema: String,
    archive_tools: Vec<InputIdentityEntry>,
    system_tools: Vec<InputIdentityEntry>,
    network_denied_tools: Vec<String>,
    configuration: BTreeMap<String, String>,
    development: Vec<InputIdentityEntry>,
}

struct SourceInputs {
    source: CedWindowsSourceArchive,
    cmake: InputIdentityEntry,
    notices: BTreeMap<String, Vec<u8>>,
}

fn source_inputs(repo: &Path, archive: &Path, cmake: &Path) -> Result<SourceInputs, String> {
    with_snapshot(|root| {
        let source = snapshot_file(root, "source.tar.gz", archive, 512 * 1024 * 1024)?;
        preflight_source_archive(&source)?;
        let inspected = ced_windows_source::inspect_ced_windows_source_archive(&source)
            .map_err(|e| e.to_string())?;
        let notices = super::windows_inputs::ced_source_notices(&source)?;
        let source = inspected;
        let cmake = snapshot_file(root, "cmake.zip", cmake, 128 * 1024 * 1024)?;
        let (_, cmake) = ced_windows_source::inspect_cmake_windows_archive(repo, &cmake)
            .map_err(|e| e.to_string())?;
        Ok(SourceInputs {
            source,
            cmake,
            notices,
        })
    })
}

fn nested_source(
    source: &CedWindowsSourceArchive,
    cmake: InputIdentityEntry,
    cache: &str,
) -> CedWindowsBuildEvidence {
    CedWindowsBuildEvidence {
        schema: ced_windows::CED_WINDOWS_BUILD_EVIDENCE_SCHEMA_V1.into(),
        source_archive: source.archive.clone(),
        cmake_archive: cmake,
        ggml: source.ggml.clone(),
        export_definition_sha256: source.export_definition_sha256.clone(),
        cmake_cache_sha256: sha256_hex(cache.as_bytes()),
    }
}

fn read_output(root: &Path) -> Result<Vec<u8>, String> {
    fn one(root: &Path, expected: &str, directory: bool) -> Result<(), String> {
        let mut entries = std::fs::read_dir(root).map_err(|e| e.to_string())?;
        let entry = entries
            .next()
            .transpose()
            .map_err(|e| e.to_string())?
            .ok_or("CED ARM output missing")?;
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        if entry.file_name() != std::ffi::OsStr::new(expected)
            || entries.next().is_some()
            || (directory && !kind.is_dir())
            || (!directory && !kind.is_file())
        {
            return Err("CED ARM output population differs from bin/ced.dll".into());
        }
        Ok(())
    }
    one(root, "bin", true)?;
    one(&root.join("bin"), "ced.dll", false)?;
    read_bounded(
        &root.join(ced_windows::CED_DLL_OUTPUT_LABEL),
        128 * 1024 * 1024,
    )
}

fn verify_outputs(root: &Path, receipt: &ControlledBuildReceipt) -> Result<(), String> {
    crate::artifact_verify::verify_controlled_build_artifacts(
        root,
        receipt,
        crate::artifact_verify::ControlledBuildArtifactVerificationLimits::new(
            2,
            2,
            128 * 1024 * 1024,
        ),
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}

fn complete_receipt(
    source: SourceIdentity,
    evidence: &CedWindowsArm64Evidence,
    builder: BuilderIdentity,
    output: &[u8],
    openmp: &AdmittedOpenMp,
    validation: &[u8],
) -> Result<ControlledBuildReceipt, String> {
    let mut draft = recipe::assemble_receipt_draft(source, evidence, builder, output, openmp)?;
    draft.schema = Some(crate::controlled_build::CONTROLLED_BUILD_RECEIPT_SCHEMA_V1.into());
    draft.validation = Some(ValidationReference {
        description: "CED Windows ARM64 network-denied clang/OpenMP build and PE inspection".into(),
        sha256: sha256_hex(validation),
    });
    draft.validate().map_err(|e| e.to_string())
}

pub fn admit(
    repo: &Path,
    paths: CedArm64InputPaths<'_>,
) -> Result<AdmittedControlledInput, String> {
    let openmp = windows_openmp::admit(paths.openmp.paths()?)?;
    let SourceInputs {
        source,
        cmake,
        mut notices,
    } = source_inputs(repo, paths.source_archive, &paths.openmp.cmake_archive)?;
    let receipt_bytes = read_bounded(paths.receipt, DOCUMENT_LIMIT)?;
    let receipt = decode_controlled_build_receipt(&receipt_bytes).map_err(|e| e.to_string())?;
    let evidence_bytes = read_bounded(paths.evidence, DOCUMENT_LIMIT)?;
    let evidence: CedWindowsArm64Evidence =
        serde_json::from_slice(&evidence_bytes).map_err(|e| e.to_string())?;
    let validation_bytes = read_bounded(paths.validation, DOCUMENT_LIMIT)?;
    let validation: CedWindowsArm64Validation =
        serde_json::from_slice(&validation_bytes).map_err(|e| e.to_string())?;
    verify_receipt_document_binding(
        &receipt,
        &evidence_bytes,
        &validation_bytes,
        recipe::EVIDENCE_LABEL,
    )?;
    if evidence.ced != nested_source(&source, cmake, &validation.cache)
        || receipt.source.windows_dependency
            != (DependencySource {
                repository: source.ced.repository,
                revision: source.ced.revision,
                content_sha256: source.archive.sha256,
            })
    {
        return Err("CED ARM receipt differs from pinned source/tool admission".into());
    }
    evidence.validate_capture(&validation)?;
    let output = read_output(paths.output_root)?;
    if complete_receipt(
        receipt.source.clone(),
        &evidence,
        receipt.builder.clone(),
        &output,
        &openmp,
        &validation_bytes,
    )? != receipt
    {
        return Err(
            "CED ARM original receipt differs from its source/configuration/OpenMP/output".into(),
        );
    }
    verify_outputs(paths.output_root, &receipt)?;
    if notices
        .insert(ced_windows::CED_DLL_OUTPUT_LABEL.into(), output)
        .is_some()
    {
        return Err("CED output collides with a pinned source notice".into());
    }
    Ok(AdmittedControlledInput::new(
        receipt,
        receipt_bytes,
        evidence_bytes,
        validation_bytes,
        notices,
    ))
}

const USAGE: &str = "ced-windows-arm64 verify-inputs|record|verify --source-archive ABS --openmp-inputs ABS_JSON [FLAG VALUE]";

fn parse_flags(args: &[String], allowed: &[&str]) -> Result<BTreeMap<String, String>, String> {
    if !args.len().is_multiple_of(2) {
        return Err(USAGE.into());
    }
    let mut values = BTreeMap::new();
    for pair in args.chunks_exact(2) {
        if !allowed.contains(&pair[0].as_str())
            || pair[1].is_empty()
            || values.insert(pair[0].clone(), pair[1].clone()).is_some()
        {
            return Err("unknown, duplicate or empty CED ARM flag".into());
        }
    }
    if allowed.iter().any(|key| !values.contains_key(*key)) {
        return Err("missing CED ARM flag".into());
    }
    Ok(values)
}

fn absolute(values: &BTreeMap<String, String>, name: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(&values[name]);
    if !path.is_absolute()
        || path
            .components()
            .any(|p| p == std::path::Component::ParentDir)
    {
        return Err(format!("ordinary absolute CED ARM path required: {name}"));
    }
    Ok(path)
}

pub fn run_cli(start: &Path, args: &[String]) -> Result<String, String> {
    let (operation, rest) = args.split_first().ok_or(USAGE)?;
    let mut allowed = vec!["--source-archive", "--openmp-inputs"];
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
    let values = parse_flags(rest, &allowed)?;
    let inventory = crate::inventory::repository_inventory_path(start)
        .ok_or("CED ARM needs its product checkout")?;
    let repo = inventory
        .ancestors()
        .nth(3)
        .ok_or("missing CED product root")?;
    let source_path = absolute(&values, "--source-archive")?;
    let openmp_files: OpenMpFiles = serde_json::from_slice(&read_bounded(
        &absolute(&values, "--openmp-inputs")?,
        DOCUMENT_LIMIT,
    )?)
    .map_err(|e| e.to_string())?;
    if operation == "verify" {
        let input = admit(
            repo,
            CedArm64InputPaths {
                receipt: &absolute(&values, "--receipt")?,
                evidence: &absolute(&values, "--evidence")?,
                validation: &absolute(&values, "--validation")?,
                source_archive: &source_path,
                output_root: &absolute(&values, "--output-root")?,
                openmp: &openmp_files,
            },
        )?;
        return Ok(format!(
            "CED_ARM64_WINDOWS_VERIFY_PASS output_sha256={} retained_members={}",
            input.receipt().outputs[0].pre_signing_sha256,
            input
                .outputs()
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(",")
        ));
    }
    let openmp = windows_openmp::admit(openmp_files.paths()?)?;
    let SourceInputs { source, cmake, .. } =
        source_inputs(repo, &source_path, &openmp_files.cmake_archive)?;
    if operation == "verify-inputs" {
        return serde_json::to_string(&serde_json::json!({"source": source.archive,
            "host_inputs": recipe::pinned_host_inputs(), "tools": crate::openmp_windows::pinned_tools(),
            "openmp_inputs": openmp.build_inputs(), "configuration": recipe::cache_configuration(),
            "capture_schema": CAPTURE_SCHEMA, "validation_schema": recipe::VALIDATION_SCHEMA})).map_err(|e| e.to_string());
    }
    let before = capture_source(repo)?;
    if before.commit != values["--product-commit"]
        || before.lock_sha256 != values["--cargo-lock-sha256"]
    {
        return Err("CED ARM recorder source differs from its driver".into());
    }
    let capture: NativeCapture = serde_json::from_slice(&read_bounded(
        &absolute(&values, "--capture")?,
        DOCUMENT_LIMIT,
    )?)
    .map_err(|e| e.to_string())?;
    if capture.schema != CAPTURE_SCHEMA {
        return Err("unknown CED ARM capture schema".into());
    }
    if capture.development != openmp.build_inputs()[4..] {
        return Err(
            "CED ARM copied OpenMP development bytes differ from the original cohort".into(),
        );
    }
    let validation_bytes = read_bounded(&absolute(&values, "--validation")?, DOCUMENT_LIMIT)?;
    let validation: CedWindowsArm64Validation =
        serde_json::from_slice(&validation_bytes).map_err(|e| e.to_string())?;
    let evidence = CedWindowsArm64Evidence {
        schema: recipe::EVIDENCE_SCHEMA.into(),
        ced: nested_source(&source, cmake, &validation.cache),
        host_inputs: recipe::pinned_host_inputs(),
        openmp: openmp.build_inputs(),
        archive_tools: capture.archive_tools,
        system_tools: capture.system_tools,
        network_denied_tools: capture.network_denied_tools,
        configuration: capture.configuration,
        build_log_sha256: sha256_hex(validation.build_log.as_bytes()),
    };
    evidence.validate_capture(&validation)?;
    let output_root = absolute(&values, "--output-root")?;
    let output = read_output(&output_root)?;
    let receipt = complete_receipt(
        SourceIdentity {
            product: before.clone(),
            windows_dependency: DependencySource {
                repository: source.ced.repository,
                revision: source.ced.revision,
                content_sha256: source.archive.sha256,
            },
        },
        &evidence,
        BuilderIdentity {
            host: values["--builder-host"].clone(),
            toolchain: "LLVM22.1.6 clang-cl/NMake x64 -> ARM64; original admitted OpenMP".into(),
        },
        &output,
        &openmp,
        &validation_bytes,
    )?;
    verify_outputs(&output_root, &receipt)?;
    if capture_source(repo)? != before {
        return Err("source changed during CED ARM recording".into());
    }
    windows_openmp::publish(
        &absolute(&values, "--evidence")?,
        &serde_json::to_vec(&evidence).map_err(|e| e.to_string())?,
    )?;
    let publication = crate::controlled_build::write_controlled_build_receipt_exclusive(
        &absolute(&values, "--receipt")?,
        &receipt,
    )
    .map_err(|e| e.to_string())?;
    if matches!(
        publication,
        crate::controlled_build::ControlledBuildReceiptPublication::PublicationUnconfirmed { .. }
    ) {
        return Err("CED ARM receipt publication unconfirmed".into());
    }
    Ok(format!(
        "CED_ARM64_WINDOWS_RECORD_PASS output_sha256={} publication={publication:?}",
        receipt.outputs[0].pre_signing_sha256
    ))
}
