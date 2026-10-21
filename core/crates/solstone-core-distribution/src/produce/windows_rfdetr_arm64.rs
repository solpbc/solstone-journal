// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Source-pinned ARM RFDETR recording and retained-byte admission.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::windows_build::{capture_source, read_bounded};
use super::windows_inputs::{
    AdmittedControlledInput, snapshot_file, verify_receipt_document_binding, with_snapshot,
};
use super::windows_openmp::{self, AdmittedOpenMp, OpenMpFiles};
use crate::controlled_build::{
    BuilderIdentity, ControlledBuildReceipt, DependencySource, InputIdentityEntry, SourceIdentity,
    ValidationReference, decode_controlled_build_receipt,
};
use crate::digest::sha256_hex;
use crate::rfdetr_windows;
use crate::rfdetr_windows_arm64::{
    self as recipe, RfdetrSourceEvidence, RfdetrWindowsArm64Evidence, RfdetrWindowsArm64Validation,
};
use crate::rfdetr_windows_source;

const DOCUMENT_LIMIT: u64 = 4 * 1024 * 1024;
// RF/ggml clang diagnostics are retained in the validation document.
const VALIDATION_LIMIT: u64 = 16 * 1024 * 1024;
const CAPTURE_SCHEMA: &str = "solstone.rfdetr-windows-arm64-build-capture.v1";

pub struct RfdetrArm64InputPaths<'a> {
    pub receipt: &'a Path,
    pub evidence: &'a Path,
    pub validation: &'a Path,
    pub rf_bundle: &'a Path,
    pub ggml_bundle: &'a Path,
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
    rf: InputIdentityEntry,
    ggml: InputIdentityEntry,
    cmake: InputIdentityEntry,
    notices: BTreeMap<String, Vec<u8>>,
}

fn source_inputs(
    repo: &Path,
    rf: &Path,
    ggml: &Path,
    cmake: &Path,
) -> Result<SourceInputs, String> {
    with_snapshot(|root| {
        let rf = snapshot_file(root, "rf.bundle", rf, 512 * 1024 * 1024)?;
        let ggml = snapshot_file(root, "ggml.bundle", ggml, 512 * 1024 * 1024)?;
        let inspected_rf = rfdetr_windows_source::inspect_git_bundle(
            &rf,
            rfdetr_windows::RFDETR_WINDOWS_RF_BUNDLE_LABEL,
        )
        .map_err(|e| e.to_string())?;
        let inspected_ggml = rfdetr_windows_source::inspect_git_bundle(
            &ggml,
            rfdetr_windows::RFDETR_WINDOWS_GGML_BUNDLE_LABEL,
        )
        .map_err(|e| e.to_string())?;
        let mut notices = BTreeMap::new();
        for (bundle, commit, files) in [
            (
                &rf,
                rfdetr_windows::RF_DETR_COMMIT,
                vec![
                    ("LICENSE", "LICENSE"),
                    ("third_party/stb/stb_image.h", "notices/stb-LICENSE.txt"),
                ],
            ),
            (
                &ggml,
                rfdetr_windows::GGML_COMMIT,
                vec![("LICENSE", "third_party/ggml/LICENSE")],
            ),
        ] {
            let bare = root.join(if commit == rfdetr_windows::RF_DETR_COMMIT {
                "rf-objects"
            } else {
                "ggml-objects"
            });
            std::fs::create_dir(&bare).map_err(|e| e.to_string())?;
            let local = bare.join("input.bundle");
            std::fs::copy(bundle, &local).map_err(|e| e.to_string())?;
            let git = |args: &[&str]| {
                rfdetr_windows_source::git_checked(&bare, args).map_err(|e| e.to_string())
            };
            git(&["init", "--bare", "."])?;
            git(&["bundle", "unbundle", "input.bundle"])?;
            for (member, label) in files {
                let blob = git(&["show", &format!("{commit}:{member}")])?;
                let bytes = if label == "notices/stb-LICENSE.txt" {
                    let start = blob
                        .rfind("ALTERNATIVE A - MIT License")
                        .ok_or("RFDETR stb notice missing")?;
                    blob[start..]
                        .strip_suffix("*/\n")
                        .ok_or("RFDETR stb notice terminator missing")?
                        .as_bytes()
                        .to_vec()
                } else {
                    blob.into_bytes()
                };
                let (size, pin) = match label {
                    "LICENSE" => (
                        11369,
                        "c5cb7d6c35a9a9a07563274ca608a012ca740770331e4f49cb17b00e6520bffa",
                    ),
                    "third_party/ggml/LICENSE" => (
                        1078,
                        "94f29bbed6a22c35b992c5c6ebf0e7c92f13b836b90f36f461c9cf2f0f1d010d",
                    ),
                    _ => (
                        2434,
                        "02513c7c7041500985a4478ba8d23f90dc861ed19a42b6d1b5200d1e808d4024",
                    ),
                };
                if bytes.len() != size || sha256_hex(&bytes) != pin {
                    return Err("RFDETR source notice differs from pinned bytes".into());
                }
                notices.insert(label.into(), bytes);
            }
        }
        let cmake = snapshot_file(root, "cmake.zip", cmake, 128 * 1024 * 1024)?;
        let (_, cmake) = crate::ced_windows_source::inspect_cmake_windows_archive(repo, &cmake)
            .map_err(|e| e.to_string())?;
        Ok(SourceInputs {
            rf: inspected_rf,
            ggml: inspected_ggml,
            cmake,
            notices,
        })
    })
}

fn nested_source(
    rf: &InputIdentityEntry,
    ggml: &InputIdentityEntry,
    cmake: InputIdentityEntry,
    cache: &str,
) -> RfdetrSourceEvidence {
    RfdetrSourceEvidence {
        rf_bundle: rf.clone(),
        ggml_bundle: ggml.clone(),
        cmake_archive: cmake,
        ggml: DependencySource {
            repository: rfdetr_windows::GGML_REPOSITORY.into(),
            revision: rfdetr_windows::GGML_COMMIT.into(),
            content_sha256: ggml.sha256.clone(),
        },
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
            .ok_or("RFDETR ARM output missing")?;
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        if entry.file_name() != std::ffi::OsStr::new(expected)
            || entries.next().is_some()
            || (directory && !kind.is_dir())
            || (!directory && !kind.is_file())
        {
            return Err("RFDETR ARM output population differs from bin/rfdetr-cli.exe".into());
        }
        Ok(())
    }
    one(root, "bin", true)?;
    one(&root.join("bin"), "rfdetr-cli.exe", false)?;
    read_bounded(
        &root.join(rfdetr_windows::RFDETR_CLI_OUTPUT_LABEL),
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
    evidence: &RfdetrWindowsArm64Evidence,
    builder: BuilderIdentity,
    output: &[u8],
    openmp: &AdmittedOpenMp,
    validation: &[u8],
) -> Result<ControlledBuildReceipt, String> {
    let mut draft = recipe::assemble_receipt_draft(source, evidence, builder, output, openmp)?;
    draft.schema = Some(crate::controlled_build::CONTROLLED_BUILD_RECEIPT_SCHEMA_V1.into());
    draft.validation = Some(ValidationReference {
        description: "RFDETR Windows ARM64 network-denied clang/OpenMP build and PE inspection"
            .into(),
        sha256: sha256_hex(validation),
    });
    draft.validate().map_err(|e| e.to_string())
}

pub fn admit(
    repo: &Path,
    paths: RfdetrArm64InputPaths<'_>,
) -> Result<AdmittedControlledInput, String> {
    let openmp = windows_openmp::admit(paths.openmp.paths()?)?;
    let SourceInputs {
        rf,
        ggml,
        cmake,
        mut notices,
    } = source_inputs(
        repo,
        paths.rf_bundle,
        paths.ggml_bundle,
        &paths.openmp.cmake_archive,
    )?;
    let receipt_bytes = read_bounded(paths.receipt, DOCUMENT_LIMIT)?;
    let receipt = decode_controlled_build_receipt(&receipt_bytes).map_err(|e| e.to_string())?;
    let evidence_bytes = read_bounded(paths.evidence, DOCUMENT_LIMIT)?;
    let evidence: RfdetrWindowsArm64Evidence =
        serde_json::from_slice(&evidence_bytes).map_err(|e| e.to_string())?;
    let validation_bytes = read_validation(paths.validation)?;
    let validation: RfdetrWindowsArm64Validation =
        serde_json::from_slice(&validation_bytes).map_err(|e| e.to_string())?;
    verify_receipt_document_binding(
        &receipt,
        &evidence_bytes,
        &validation_bytes,
        recipe::EVIDENCE_LABEL,
    )?;
    if evidence.rfdetr != nested_source(&rf, &ggml, cmake, &validation.host.cache)
        || receipt.source.windows_dependency
            != (DependencySource {
                repository: rfdetr_windows::RF_DETR_REPOSITORY.into(),
                revision: rfdetr_windows::RF_DETR_COMMIT.into(),
                content_sha256: rf.sha256.clone(),
            })
    {
        return Err("RFDETR ARM receipt differs from pinned source/tool admission".into());
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
            "RFDETR ARM original receipt differs from its source/configuration/OpenMP/output"
                .into(),
        );
    }
    verify_outputs(paths.output_root, &receipt)?;
    if notices
        .insert(rfdetr_windows::RFDETR_CLI_OUTPUT_LABEL.into(), output)
        .is_some()
    {
        return Err("RFDETR output collides with a pinned source notice".into());
    }
    Ok(AdmittedControlledInput::new(
        receipt,
        receipt_bytes,
        evidence_bytes,
        validation_bytes,
        notices,
    ))
}

fn read_validation(path: &Path) -> Result<Vec<u8>, String> {
    read_bounded(path, VALIDATION_LIMIT)
        .map_err(|error| format!("RF-DETR validation document: {error}"))
}

const USAGE: &str = "rfdetr-windows-arm64 verify-inputs|record|verify --rf-bundle ABS --ggml-bundle ABS --openmp-inputs ABS_JSON [FLAG VALUE]";

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
            return Err("unknown, duplicate or empty RFDETR ARM flag".into());
        }
    }
    if allowed.iter().any(|key| !values.contains_key(*key)) {
        return Err("missing RFDETR ARM flag".into());
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
        return Err(format!(
            "ordinary absolute RFDETR ARM path required: {name}"
        ));
    }
    Ok(path)
}

pub fn run_cli(start: &Path, args: &[String]) -> Result<String, String> {
    let (operation, rest) = args.split_first().ok_or(USAGE)?;
    let mut allowed = vec!["--rf-bundle", "--ggml-bundle", "--openmp-inputs"];
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
        .ok_or("RFDETR ARM needs its product checkout")?;
    let repo = inventory
        .ancestors()
        .nth(3)
        .ok_or("missing RFDETR product root")?;
    let rf_path = absolute(&values, "--rf-bundle")?;
    let ggml_path = absolute(&values, "--ggml-bundle")?;
    let openmp_files: OpenMpFiles = serde_json::from_slice(&read_bounded(
        &absolute(&values, "--openmp-inputs")?,
        DOCUMENT_LIMIT,
    )?)
    .map_err(|e| e.to_string())?;
    if operation == "verify" {
        let input = admit(
            repo,
            RfdetrArm64InputPaths {
                receipt: &absolute(&values, "--receipt")?,
                evidence: &absolute(&values, "--evidence")?,
                validation: &absolute(&values, "--validation")?,
                rf_bundle: &rf_path,
                ggml_bundle: &ggml_path,
                output_root: &absolute(&values, "--output-root")?,
                openmp: &openmp_files,
            },
        )?;
        return Ok(format!(
            "RFDETR_ARM64_WINDOWS_VERIFY_PASS output_sha256={} retained_members={}",
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
    let SourceInputs {
        rf, ggml, cmake, ..
    } = source_inputs(repo, &rf_path, &ggml_path, &openmp_files.cmake_archive)?;
    if operation == "verify-inputs" {
        return serde_json::to_string(&serde_json::json!({"rf_bundle": rf, "ggml_bundle": ggml,
            "host_inputs": recipe::pinned_host_inputs(), "tools": crate::openmp_windows::pinned_tools(),
            "openmp_inputs": openmp.build_inputs(), "configuration": recipe::cache_configuration(),
            "capture_schema": CAPTURE_SCHEMA, "validation_schema": recipe::VALIDATION_SCHEMA})).map_err(|e| e.to_string());
    }
    let before = capture_source(repo)?;
    if before.commit != values["--product-commit"]
        || before.lock_sha256 != values["--cargo-lock-sha256"]
    {
        return Err("RFDETR ARM recorder source differs from its driver".into());
    }
    let capture: NativeCapture = serde_json::from_slice(&read_bounded(
        &absolute(&values, "--capture")?,
        DOCUMENT_LIMIT,
    )?)
    .map_err(|e| e.to_string())?;
    if capture.schema != CAPTURE_SCHEMA {
        return Err("unknown RFDETR ARM capture schema".into());
    }
    if capture.development != openmp.build_inputs()[4..] {
        return Err(
            "RFDETR ARM copied OpenMP development bytes differ from the original cohort".into(),
        );
    }
    let validation_bytes = read_validation(&absolute(&values, "--validation")?)?;
    let validation: RfdetrWindowsArm64Validation =
        serde_json::from_slice(&validation_bytes).map_err(|e| e.to_string())?;
    let evidence = RfdetrWindowsArm64Evidence {
        schema: recipe::EVIDENCE_SCHEMA.into(),
        rfdetr: nested_source(&rf, &ggml, cmake, &validation.host.cache),
        host_inputs: recipe::pinned_host_inputs(),
        openmp: openmp.build_inputs(),
        archive_tools: capture.archive_tools,
        system_tools: capture.system_tools,
        network_denied_tools: capture.network_denied_tools,
        configuration: capture.configuration,
        build_log_sha256: sha256_hex(validation.host.build_log.as_bytes()),
        compile_options_sha256: sha256_hex(
            &serde_json::to_vec(&validation.compile_options).map_err(|e| e.to_string())?,
        ),
    };
    evidence.validate_capture(&validation)?;
    let output_root = absolute(&values, "--output-root")?;
    let output = read_output(&output_root)?;
    let receipt = complete_receipt(
        SourceIdentity {
            product: before.clone(),
            windows_dependency: DependencySource {
                repository: rfdetr_windows::RF_DETR_REPOSITORY.into(),
                revision: rfdetr_windows::RF_DETR_COMMIT.into(),
                content_sha256: rf.sha256.clone(),
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
        return Err("source changed during RFDETR ARM recording".into());
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
        return Err("RFDETR ARM receipt publication unconfirmed".into());
    }
    Ok(format!(
        "RFDETR_ARM64_WINDOWS_RECORD_PASS output_sha256={} publication={publication:?}",
        receipt.outputs[0].pre_signing_sha256
    ))
}

#[cfg(all(test, feature = "full-tests"))]
mod source_tests {
    use super::*;

    #[test]
    #[ignore = "requires the retained native RF-DETR validation document"]
    fn real_native_validation_retains_full_compiler_diagnostics() {
        let file = PathBuf::from(
            std::env::var_os("SOLSTONE_RFDETR_ARM_VALIDATION").expect("native validation file"),
        );
        let bytes = read_validation(&file).unwrap();
        assert!(bytes.len() as u64 > DOCUMENT_LIMIT);
        let validation: RfdetrWindowsArm64Validation = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(validation.host.schema, recipe::VALIDATION_SCHEMA);
        assert!(
            validation
                .host
                .build_log
                .lines()
                .any(|line| line == "RFDETR_ARM64_WINDOWS_BUILD_EXIT=0")
        );
        assert_eq!(
            validation
                .compile_options
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["ggml-cpu", "rfdetr", "rfdetr-cli"]
        );
    }

    #[test]
    #[ignore = "requires the retained pinned RF/ggml bundles and CMake archive"]
    fn real_source_bundles_derive_notices_and_refuse_swapped_roles() {
        let map = PathBuf::from(
            std::env::var_os("SOLSTONE_RFDETR_ARM_SOURCE_INPUTS").expect("source input map"),
        );
        let inputs: BTreeMap<String, PathBuf> =
            serde_json::from_slice(&std::fs::read(map).unwrap()).unwrap();
        let inventory =
            crate::inventory::repository_inventory_path(Path::new(env!("CARGO_MANIFEST_DIR")))
                .unwrap();
        let repo = inventory.ancestors().nth(3).unwrap();
        let admitted =
            source_inputs(repo, &inputs["rf"], &inputs["ggml"], &inputs["cmake"]).unwrap();
        assert_eq!(
            admitted
                .notices
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            [
                "LICENSE",
                "notices/stb-LICENSE.txt",
                "third_party/ggml/LICENSE"
            ]
        );
        assert!(source_inputs(repo, &inputs["ggml"], &inputs["rf"], &inputs["cmake"]).is_err());
    }
}
