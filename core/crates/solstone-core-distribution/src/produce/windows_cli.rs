// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Local Windows production from fixed, typed input paths. Input files provide
//! no digests, output destinations, trust overrides or new admission authority.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

use super::windows_archives::{admit_msvc, admit_pdfium, admit_rclone, admit_restic};
use super::windows_build::{build_windows_product, capture_source, read_bounded};
use super::windows_inputs::{
    ControlledInputPaths, OnnxInputPaths, RfdetrInputPaths, admit_ced, admit_ffmpeg_notices,
    admit_onnx, admit_parakeet, admit_rfdetr,
};
use super::windows_stage::{AdmittedWindowsNativeInputs, stage_windows_payload};

const USAGE: &str = "produce windows-x86_64 DEST --inputs LOCAL_JSON --logs FRESH_DIRECTORY (absolute paths required)";

fn validate_rust_notices(index: &[u8], notices: &[u8], lock: &str) -> Result<(), String> {
    let index: serde_json::Value = serde_json::from_slice(index).map_err(|e| e.to_string())?;
    if index["schema"].as_str() != Some("solstone.windows-rust-notices.v1")
        || index["cargo_lock_sha256"].as_str() != Some(lock)
        || index["notices_sha256"].as_str() != Some(&crate::digest::sha256_hex(notices))
    {
        return Err("Windows Rust notices do not match the current lock and notice bytes".into());
    }
    Ok(())
}

#[derive(Debug)]
struct InputPath(PathBuf);

impl<'de> Deserialize<'de> for InputPath {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let path = PathBuf::deserialize(deserializer)?;
        require_absolute(&path).map_err(serde::de::Error::custom)?;
        Ok(Self(path))
    }
}

fn require_absolute(path: &Path) -> Result<(), String> {
    if !path.is_absolute() || path.components().any(|part| part == Component::ParentDir) {
        return Err(format!(
            "absolute path without parent traversal required: {}",
            path.display()
        ));
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ControlledFiles {
    receipt: InputPath,
    evidence: InputPath,
    validation: InputPath,
    source_archive: InputPath,
    cmake_archive: InputPath,
    output_root: InputPath,
}

impl ControlledFiles {
    fn paths(&self) -> ControlledInputPaths<'_> {
        ControlledInputPaths {
            receipt: &self.receipt.0,
            evidence: &self.evidence.0,
            validation: &self.validation.0,
            source_archive: &self.source_archive.0,
            cmake_archive: &self.cmake_archive.0,
            output_root: &self.output_root.0,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ParakeetFiles {
    build: ControlledFiles,
    model: InputPath,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OnnxFiles {
    build: ControlledFiles,
    mirror_archive: InputPath,
    python_archive: InputPath,
    protoc_archive: InputPath,
    cmake_cache: InputPath,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RfdetrFiles {
    build: ControlledFiles,
    ggml_bundle: InputPath,
    cmake_cache: InputPath,
    build_options: InputPath,
    subprocess_evidence: InputPath,
    license: InputPath,
    ggml_license: InputPath,
    stb_license: InputPath,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ArchiveWithLicense {
    archive: InputPath,
    license: InputPath,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MsvcFiles {
    archive: InputPath,
    runtime_license: InputPath,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LocalInputs {
    ced: ControlledFiles,
    parakeet: ParakeetFiles,
    onnx: OnnxFiles,
    rfdetr: RfdetrFiles,
    restic: ArchiveWithLicense,
    rclone: ArchiveWithLicense,
    msvc: MsvcFiles,
    pdfium_archive: InputPath,
    ffmpeg_archive: InputPath,
}

impl LocalInputs {
    fn admit(&self, repo: &Path) -> Result<AdmittedWindowsNativeInputs, String> {
        let controlled = vec![
            admit_ced(repo, self.ced.paths())?,
            admit_parakeet(repo, self.parakeet.build.paths(), &self.parakeet.model.0)?,
            admit_onnx(
                repo,
                OnnxInputPaths {
                    build: self.onnx.build.paths(),
                    mirror_archive: &self.onnx.mirror_archive.0,
                    python_archive: &self.onnx.python_archive.0,
                    protoc_archive: &self.onnx.protoc_archive.0,
                    cmake_cache: &self.onnx.cmake_cache.0,
                },
            )?,
            admit_rfdetr(
                repo,
                RfdetrInputPaths {
                    build: self.rfdetr.build.paths(),
                    ggml_bundle: &self.rfdetr.ggml_bundle.0,
                    cmake_cache: &self.rfdetr.cmake_cache.0,
                    build_options: &self.rfdetr.build_options.0,
                    subprocess_evidence: &self.rfdetr.subprocess_evidence.0,
                    license: &self.rfdetr.license.0,
                    ggml_license: &self.rfdetr.ggml_license.0,
                    stb_license: &self.rfdetr.stb_license.0,
                },
            )?,
        ];
        AdmittedWindowsNativeInputs::from_controlled(controlled)?.with_archives(vec![
            admit_restic(&self.restic.archive.0, &self.restic.license.0)?,
            admit_rclone(&self.rclone.archive.0, &self.rclone.license.0)?,
            admit_msvc(&self.msvc.archive.0, &self.msvc.runtime_license.0)?,
            admit_pdfium(&self.pdfium_archive.0)?,
            admit_ffmpeg_notices(repo, &self.ffmpeg_archive.0)?,
        ])
    }
}

#[derive(Debug)]
struct Args {
    destination: PathBuf,
    inputs: PathBuf,
    logs: PathBuf,
}

fn parse_args(args: &[String]) -> Result<Args, String> {
    let Some(destination) = args.first() else {
        return Err(USAGE.into());
    };
    let mut inputs = None;
    let mut logs = None;
    let mut flags = args[1..].chunks_exact(2);
    for flag in &mut flags {
        match flag[0].as_str() {
            "--inputs" if inputs.is_none() => inputs = Some(PathBuf::from(&flag[1])),
            "--logs" if logs.is_none() => logs = Some(PathBuf::from(&flag[1])),
            _ => {
                return Err(format!(
                    "unknown or duplicate Windows production option: {}; {USAGE}",
                    flag[0]
                ));
            }
        }
    }
    if !flags.remainder().is_empty() {
        return Err(USAGE.into());
    }
    let result = Args {
        destination: destination.into(),
        inputs: inputs.ok_or(USAGE)?,
        logs: logs.ok_or(USAGE)?,
    };
    for path in [&result.destination, &result.inputs, &result.logs] {
        require_absolute(path)?;
    }
    Ok(result)
}

/// The reviewed native wrapper owns the existing host fence, finite wait,
/// isolated tool environment and cleanup evidence. Build tools remain Unowned.
/// This command produces an unsigned local tree; it cannot sign or promote it.
pub fn run_cli(start: &Path, args: &[String]) -> Result<String, String> {
    let args = parse_args(args)?;
    if !cfg!(windows) {
        return Err("Windows production requires its native MSVC host".into());
    }
    let inventory_path = crate::inventory::repository_inventory_path(start)
        .ok_or("could not locate canonical distribution inventory")?;
    let inventory =
        crate::validate_distribution_inventory(&inventory_path).map_err(|e| e.to_string())?;
    let repo = inventory_path
        .ancestors()
        .nth(3)
        .ok_or("missing repository root")?;
    let before = capture_source(repo)?;
    validate_rust_notices(
        &read_bounded(
            &repo.join("core/distribution/windows-rust-sources.json"),
            4 * 1024 * 1024,
        )?,
        &read_bounded(
            &repo.join("core/distribution/windows-rust-NOTICES.txt"),
            16 * 1024 * 1024,
        )?,
        &before.lock_sha256,
    )?;
    let input_bytes = read_bounded(&args.inputs, 1024 * 1024)?;
    let inputs: LocalInputs = serde_json::from_slice(&input_bytes).map_err(|e| e.to_string())?;
    let native = inputs.admit(repo)?;
    if capture_source(repo)? != before {
        return Err("product source changed during native input admission".into());
    }
    let product = build_windows_product(repo, &inventory, &args.logs, &inputs.ffmpeg_archive.0)?;
    // Retain the actual operator file, including original spelling, independently
    // of the inventory and original dependency receipts. No input file is a grant.
    let mut retained_inputs = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(args.logs.join("input-paths.json"))
        .map_err(|e| e.to_string())?;
    retained_inputs
        .write_all(&input_bytes)
        .map_err(|e| e.to_string())?;
    retained_inputs.sync_all().map_err(|e| e.to_string())?;
    let (staged, published) = stage_windows_payload(repo, &product, &native, &args.destination)
        .map_err(|e| e.to_string())?;
    // Informational command output. The existing unsigned manifest remains the
    // payload identity; this summary never admits a replacement tree or bytes.
    serde_json::to_string(&serde_json::json!({
        "target": "windows-x86_64",
        "source": product.evidence().source,
        "destination": published.destination,
        "unsigned_manifest_sha256": staged.unsigned_manifest_sha256,
        "durability_proven": published.durability_proven,
        "signed": false,
        "files": staged.files.iter().map(|file| serde_json::json!({
            "path": file.dest, "sha256": file.digest, "mode": file.mode,
        })).collect::<Vec<_>>(),
        "runtime_edges": staged.runtime_edges.iter().map(|edge| serde_json::json!({
            "importer": edge.importer, "kind": edge.kind, "library": edge.library, "member": edge.member,
        })).collect::<Vec<_>>(),
    })).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_lock_or_changed_rust_notices_refuse_before_production() {
        let notices = b"original upstream notices";
        let index = serde_json::to_vec(&serde_json::json!({
            "schema": "solstone.windows-rust-notices.v1",
            "cargo_lock_sha256": "current-lock",
            "notices_sha256": crate::digest::sha256_hex(notices),
        }))
        .unwrap();
        assert!(validate_rust_notices(&index, notices, "current-lock").is_ok());
        assert!(validate_rust_notices(&index, notices, "changed-lock").is_err());
        assert!(validate_rust_notices(&index, b"replaced", "current-lock").is_err());
        assert!(validate_rust_notices(b"{}", notices, "current-lock").is_err());
        assert!(validate_rust_notices(b"not-json", notices, "current-lock").is_err());
    }

    fn arguments(root: &Path) -> Vec<String> {
        [
            root.join("payload").display().to_string(),
            "--inputs".into(),
            root.join("inputs.json").display().to_string(),
            "--logs".into(),
            root.join("logs").display().to_string(),
        ]
        .into()
    }

    #[test]
    fn command_requires_explicit_paths_and_refuses_extra_authorities() {
        let root = tempfile::tempdir().unwrap();
        let args = arguments(root.path());
        assert_eq!(parse_args(&args).unwrap().logs, root.path().join("logs"));
        for extra in [
            vec!["--inputs", "other"],
            vec!["--signature-key", "key"],
            vec!["trailing"],
        ] {
            let mut candidate = args.clone();
            candidate.extend(extra.into_iter().map(String::from));
            assert!(parse_args(&candidate).is_err());
        }
        assert!(parse_args(&args[..3]).is_err());
        let mut relative = args;
        relative[0] = "relative".into();
        assert!(parse_args(&relative).is_err());
    }

    #[test]
    fn local_input_objects_reject_relative_paths_and_unrecognized_evidence_fields() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("input");
        let value = serde_json::json!({"archive": path, "license": path});
        assert!(serde_json::from_value::<ArchiveWithLicense>(value.clone()).is_ok());
        for field in ["sha256", "destination", "signature", "optional"] {
            let mut candidate = value.clone();
            candidate[field] = serde_json::json!("override");
            assert!(serde_json::from_value::<ArchiveWithLicense>(candidate).is_err());
        }
        let mut relative = value;
        relative["license"] = serde_json::json!("relative");
        assert!(serde_json::from_value::<ArchiveWithLicense>(relative).is_err());
    }

    #[cfg(not(windows))]
    #[test]
    fn non_native_run_refuses_before_input_read_or_output_creation() {
        let root = tempfile::tempdir().unwrap();
        assert!(
            run_cli(root.path(), &arguments(root.path()))
                .unwrap_err()
                .contains("native MSVC host")
        );
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }
}
