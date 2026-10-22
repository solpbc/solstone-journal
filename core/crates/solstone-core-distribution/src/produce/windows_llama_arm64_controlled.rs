// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Record and replay the original controlled ARM Llama build capture.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::windows_build::{capture_source, read_bounded};
use super::windows_inputs::{AdmittedControlledInput, snapshot_file, with_snapshot};
use super::windows_openmp::{self, OpenMpFiles};
use crate::controlled_build::{
    BuilderIdentity, ControlledBuildReceipt, ControlledBuildReceiptDraft, DependencySource,
    InputIdentityEntry, SourceIdentity, SupportingArtifactRef, ValidationReference,
};
use crate::digest::sha256_hex;
use crate::llama_windows_arm64_evidence::{self as recipe, BuildEvidence, DriverSourceIdentity};
use crate::provenance::Provenance;

const DOCUMENT_LIMIT: u64 = 16 * 1024 * 1024;
const DRIVER: &str = "core/distribution/llama-windows-arm64-build.ps1";
const HELPER: &str = "core/distribution/ffmpeg-windows-arm64-tools.ps1";

pub struct LlamaArm64InputPaths<'a> {
    pub receipt: &'a Path,
    pub evidence: &'a Path,
    pub source_archive: &'a Path,
    pub sdk_archive: &'a Path,
    pub capture_root: &'a Path,
    pub openmp: &'a OpenMpFiles,
}

struct SourceInputs {
    archive: InputIdentityEntry,
    census: BTreeMap<String, (u64, String)>,
    notices: BTreeMap<String, Vec<u8>>,
}

fn source_inputs(source: &Path, sdk: &Path) -> Result<SourceInputs, String> {
    with_snapshot(|root| {
        let source = snapshot_file(
            root,
            "source.tar.gz",
            source,
            crate::llama_windows_source::SOURCE_BYTES,
        )?;
        let sdk = snapshot_file(root, "sdk.exe", sdk, crate::llama_windows_source::SDK_BYTES)?;
        let (archive, census) = crate::llama_windows_source::inspect_source_with_census(&source)
            .map_err(|e| e.to_string())?;
        crate::llama_windows_source::inspect_sdk(&sdk).map_err(|e| e.to_string())?;
        let notices = super::windows_inputs::llama_source_notices(&source)?;
        Ok(SourceInputs {
            archive,
            census,
            notices,
        })
    })
}

fn driver_identity(repo: &Path, product: &Provenance) -> Result<DriverSourceIdentity, String> {
    if product.commit.len() != 40
        || !product
            .commit
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err("Llama capture product commit is not a full Git identity".into());
    }
    let blob = |member: &str| -> Result<Vec<u8>, String> {
        crate::rfdetr_windows_source::git_checked(
            repo,
            &[
                "show",
                "--no-textconv",
                &format!("{}:{member}", product.commit),
            ],
        )
        .map(String::into_bytes)
        .map_err(|e| e.to_string())
    };
    if sha256_hex(&blob("core/Cargo.lock")?) != product.lock_sha256 {
        return Err("Llama original product lock differs from its Git object".into());
    }
    Ok(DriverSourceIdentity {
        product: product.clone(),
        driver_sha256: sha256_hex(&blob(DRIVER)?),
        helper_sha256: sha256_hex(&blob(HELPER)?),
    })
}

fn expected_receipt(
    source: SourceIdentity,
    evidence: &BuildEvidence,
    builder: BuilderIdentity,
    files: &BTreeMap<String, Vec<u8>>,
) -> Result<ControlledBuildReceipt, String> {
    if source.product != evidence.product
        || source.windows_dependency
            != (DependencySource {
                repository: "https://github.com/ggml-org/llama.cpp".into(),
                revision: crate::llama_windows_source::LLAMA_COMMIT.into(),
                content_sha256: crate::llama_windows_source::SOURCE_SHA256.into(),
            })
    {
        return Err("Llama receipt source differs from its original captured source".into());
    }
    let mut inputs = vec![evidence.source_archive.clone()];
    inputs.extend(evidence.host_inputs.clone());
    inputs.extend(evidence.openmp_inputs.clone());
    let outputs = ["bin/llama-server.exe", "bin/vulkan-1.dll"]
        .map(|label| (label, files[&format!("output/{label}")].as_slice()));
    let draft = ControlledBuildReceiptDraft {
        schema: Some(crate::controlled_build::CONTROLLED_BUILD_RECEIPT_SCHEMA_V1.into()), source: Some(source), inputs: Some(inputs), builder: Some(builder), configuration: Some(recipe::build_configuration()),
        outputs: Some(crate::controlled_build::census_outputs(&outputs).map_err(|e| e.to_string())?),
        validation: Some(ValidationReference {description: "Windows x64 to ARM64 Llama CPU/OpenMP/Vulkan build; original network-denied capture and artifact inspection".into(), sha256: sha256_hex(&files["report/validation.json"])}),
        supporting: Some(vec![SupportingArtifactRef {label: recipe::EVIDENCE_LABEL.into(), sha256: sha256_hex(&serde_json::to_vec(evidence).map_err(|e| e.to_string())?)}]),
    };
    draft.validate().map_err(|e| e.to_string())
}

pub fn admit(
    repo: &Path,
    paths: LlamaArm64InputPaths<'_>,
) -> Result<AdmittedControlledInput, String> {
    let openmp = windows_openmp::admit(paths.openmp.paths()?)?;
    let source = source_inputs(paths.source_archive, paths.sdk_archive)?;
    let receipt_bytes = read_bounded(paths.receipt, DOCUMENT_LIMIT)?;
    let receipt = crate::controlled_build::decode_controlled_build_receipt(&receipt_bytes)
        .map_err(|e| e.to_string())?;
    let evidence_bytes = read_bounded(paths.evidence, DOCUMENT_LIMIT)?;
    let files = crate::llama_windows_capture::read_capture(paths.capture_root)?;
    let identity = driver_identity(repo, &receipt.source.product)?;
    let evidence =
        recipe::inspect_capture(&files, source.archive, &source.census, &openmp, &identity)?;
    if serde_json::to_vec(&evidence).map_err(|e| e.to_string())? != evidence_bytes
        || expected_receipt(
            receipt.source.clone(),
            &evidence,
            receipt.builder.clone(),
            &files,
        )? != receipt
    {
        return Err("Llama ARM original receipt/evidence differs from its native capture".into());
    }
    let mut outputs = source.notices;
    for label in ["bin/llama-server.exe", "bin/vulkan-1.dll"] {
        if outputs
            .insert(label.into(), files[&format!("output/{label}")].clone())
            .is_some()
        {
            return Err("Llama native output collides with a source notice".into());
        }
    }
    Ok(AdmittedControlledInput::new(
        receipt,
        receipt_bytes,
        evidence_bytes,
        files["report/validation.json"].clone(),
        outputs,
    ))
}

const USAGE: &str = "llama-windows-arm64 verify-inputs|record|verify --source-archive ABS --sdk-archive ABS --openmp-inputs ABS_JSON [FLAG VALUE]";

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
            return Err("unknown, empty or duplicate Llama ARM input flag".into());
        }
    }
    if allowed.iter().any(|key| !values.contains_key(*key)) {
        return Err("missing Llama ARM input flag".into());
    }
    Ok(values)
}

fn absolute(values: &BTreeMap<String, String>, key: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(&values[key]);
    if !path.is_absolute()
        || path
            .components()
            .any(|part| part == std::path::Component::ParentDir)
    {
        return Err(format!("ordinary absolute Llama path required: {key}"));
    }
    Ok(path)
}

pub fn run_cli(start: &Path, args: &[String]) -> Result<String, String> {
    let (operation, rest) = args.split_first().ok_or(USAGE)?;
    let mut allowed = vec!["--source-archive", "--sdk-archive", "--openmp-inputs"];
    match operation.as_str() {
        "verify-inputs" => allowed.push("--work-root"),
        "verify" => allowed.extend(["--capture-root", "--receipt", "--evidence"]),
        "record" => allowed.extend([
            "--capture-root",
            "--receipt",
            "--evidence",
            "--product-commit",
            "--cargo-lock-sha256",
            "--builder-host",
        ]),
        _ => return Err(USAGE.into()),
    }
    let values = flags(rest, &allowed)?;
    let inventory = crate::inventory::repository_inventory_path(start)
        .ok_or("Llama ARM requires its product checkout")?;
    let repo = inventory
        .ancestors()
        .nth(3)
        .ok_or("Llama ARM product root missing")?;
    let source_path = absolute(&values, "--source-archive")?;
    let sdk_path = absolute(&values, "--sdk-archive")?;
    let openmp_files: OpenMpFiles = serde_json::from_slice(&read_bounded(
        &absolute(&values, "--openmp-inputs")?,
        DOCUMENT_LIMIT,
    )?)
    .map_err(|e| e.to_string())?;
    if operation == "verify" {
        let input = admit(
            repo,
            LlamaArm64InputPaths {
                receipt: &absolute(&values, "--receipt")?,
                evidence: &absolute(&values, "--evidence")?,
                source_archive: &source_path,
                sdk_archive: &sdk_path,
                capture_root: &absolute(&values, "--capture-root")?,
                openmp: &openmp_files,
            },
        )?;
        return Ok(format!(
            "LLAMA_ARM64_WINDOWS_VERIFY_PASS outputs={}",
            input.receipt().outputs.len()
        ));
    }
    let openmp = windows_openmp::admit(openmp_files.paths()?)?;
    let source = source_inputs(&source_path, &sdk_path)?;
    if operation == "verify-inputs" {
        let work = &values["--work-root"];
        crate::windows_clang::windows_path(work)?;
        return serde_json::to_string(&serde_json::json!({
            "source": source.archive, "host_inputs": crate::llama_windows_arm64::pinned_host_inputs(),
            "tools": crate::llama_windows_arm64::pinned_archive_tools(), "openmp_inputs": openmp.build_inputs(),
            "configuration": recipe::configuration(), "engine_arguments": recipe::configure_arguments("engine", work)?, "loader_arguments": recipe::configure_arguments("loader", work)?,
            "generated_members": crate::llama_windows_arm64_generated::reviewed_members(), "validation_schema": recipe::VALIDATION_SCHEMA,
        })).map_err(|e| e.to_string());
    }
    let before = capture_source(repo)?;
    if before.commit != values["--product-commit"]
        || before.lock_sha256 != values["--cargo-lock-sha256"]
    {
        return Err("Llama ARM recorder source differs from the driver".into());
    }
    let capture_root = absolute(&values, "--capture-root")?;
    let canonical_capture = std::fs::canonicalize(&capture_root).map_err(|e| e.to_string())?;
    for key in ["--receipt", "--evidence"] {
        let path = absolute(&values, key)?;
        let parent = std::fs::canonicalize(path.parent().ok_or("Llama record parent missing")?)
            .map_err(|e| e.to_string())?;
        if parent.starts_with(&canonical_capture) {
            return Err("Llama admission records must stay outside the original capture".into());
        }
    }
    let identity = driver_identity(repo, &before)?;
    let files = crate::llama_windows_capture::read_capture(&capture_root)?;
    let evidence =
        recipe::inspect_capture(&files, source.archive, &source.census, &openmp, &identity)?;
    let receipt = expected_receipt(
        SourceIdentity {
            product: before.clone(),
            windows_dependency: DependencySource {
                repository: "https://github.com/ggml-org/llama.cpp".into(),
                revision: crate::llama_windows_source::LLAMA_COMMIT.into(),
                content_sha256: crate::llama_windows_source::SOURCE_SHA256.into(),
            },
        },
        &evidence,
        BuilderIdentity {
            host: values["--builder-host"].clone(),
            toolchain: "LLVM22.1.6 clang-cl / MSVC14.44 / native x64 shader generation".into(),
        },
        &files,
    )?;
    if capture_source(repo)? != before {
        return Err("Llama recorder source changed during recording".into());
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
        return Err("Llama ARM receipt publication unconfirmed".into());
    }
    Ok("LLAMA_ARM64_WINDOWS_RECORD_PASS".into())
}
