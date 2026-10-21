// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! ARM input paths select dedicated controlled readers. The caller owns target
//! selection; this file supplies no output destination or admission authority.

use super::*;
use crate::produce::{
    windows_ced_arm64 as ced, windows_llama_arm64_controlled as llama,
    windows_nvattest_arm64 as nvattest, windows_onnx_arm64 as onnx, windows_openmp as openmp,
    windows_parakeet_arm64 as parakeet, windows_restic as restic, windows_rfdetr_arm64 as rfdetr,
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ControlledFiles {
    receipt: InputPath,
    evidence: InputPath,
    validation: InputPath,
    source_archive: InputPath,
    output_root: InputPath,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LlamaFiles {
    receipt: InputPath,
    evidence: InputPath,
    source_archive: InputPath,
    sdk_archive: InputPath,
    capture_root: InputPath,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ParakeetFiles {
    build: ControlledFiles,
    model: InputPath,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RfdetrFiles {
    build: ControlledFiles,
    ggml_bundle: InputPath,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OnnxFiles {
    build: ControlledFiles,
    mirror_archive: InputPath,
    kleidiai_archive: InputPath,
    cmake_archive: InputPath,
    python_archive: InputPath,
    protoc_archive: InputPath,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NvattestFiles {
    receipt: InputPath,
    evidence: InputPath,
    source_archive: InputPath,
    bundle_archive: InputPath,
    capture_root: InputPath,
    output_root: InputPath,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResticFiles {
    build: ControlledFiles,
    go_archive: InputPath,
    modules_archive: InputPath,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LocalInputs {
    llama: LlamaFiles,
    openmp: openmp::OpenMpFiles,
    ced: ControlledFiles,
    parakeet: ParakeetFiles,
    onnx: OnnxFiles,
    rfdetr: RfdetrFiles,
    nvattest: NvattestFiles,
    restic: ResticFiles,
    rclone: ArchiveWithLicense,
    msvc: MsvcFiles,
    pdfium_archive: InputPath,
    ffmpeg_archive: InputPath,
}

impl LocalInputs {
    pub(super) fn ffmpeg_archive(&self) -> &Path {
        &self.ffmpeg_archive.0
    }

    pub(super) fn admit(&self, repo: &Path) -> Result<AdmittedWindowsNativeInputs, String> {
        let architecture = WindowsArchitecture::Aarch64;
        let openmp = openmp::admit(self.openmp.paths()?)?;
        let onnx_files = onnx::OnnxArm64Files {
            source_archive: self.onnx.build.source_archive.0.clone(),
            mirror_archive: self.onnx.mirror_archive.0.clone(),
            kleidiai_archive: self.onnx.kleidiai_archive.0.clone(),
            cmake_archive: self.onnx.cmake_archive.0.clone(),
            python_archive: self.onnx.python_archive.0.clone(),
            protoc_archive: self.onnx.protoc_archive.0.clone(),
        };
        let controlled = vec![
            llama::admit(
                repo,
                llama::LlamaArm64InputPaths {
                    receipt: &self.llama.receipt.0,
                    evidence: &self.llama.evidence.0,
                    source_archive: &self.llama.source_archive.0,
                    sdk_archive: &self.llama.sdk_archive.0,
                    capture_root: &self.llama.capture_root.0,
                    openmp: &self.openmp,
                },
            )?,
            ced::admit(
                repo,
                ced::CedArm64InputPaths {
                    receipt: &self.ced.receipt.0,
                    evidence: &self.ced.evidence.0,
                    validation: &self.ced.validation.0,
                    source_archive: &self.ced.source_archive.0,
                    output_root: &self.ced.output_root.0,
                    openmp: &self.openmp,
                },
            )?,
            parakeet::admit(
                repo,
                parakeet::ParakeetArm64InputPaths {
                    receipt: &self.parakeet.build.receipt.0,
                    evidence: &self.parakeet.build.evidence.0,
                    validation: &self.parakeet.build.validation.0,
                    source_archive: &self.parakeet.build.source_archive.0,
                    model: &self.parakeet.model.0,
                    output_root: &self.parakeet.build.output_root.0,
                    openmp: &self.openmp,
                },
            )?,
            onnx::admit(
                repo,
                onnx::OnnxArm64InputPaths {
                    files: &onnx_files,
                    receipt: &self.onnx.build.receipt.0,
                    evidence: &self.onnx.build.evidence.0,
                    validation: &self.onnx.build.validation.0,
                    output_root: &self.onnx.build.output_root.0,
                },
            )?,
            rfdetr::admit(
                repo,
                rfdetr::RfdetrArm64InputPaths {
                    receipt: &self.rfdetr.build.receipt.0,
                    evidence: &self.rfdetr.build.evidence.0,
                    validation: &self.rfdetr.build.validation.0,
                    rf_bundle: &self.rfdetr.build.source_archive.0,
                    ggml_bundle: &self.rfdetr.ggml_bundle.0,
                    output_root: &self.rfdetr.build.output_root.0,
                    openmp: &self.openmp,
                },
            )?,
            nvattest::admit(
                repo,
                nvattest::InputPaths {
                    receipt: &self.nvattest.receipt.0,
                    evidence: &self.nvattest.evidence.0,
                    source_archive: &self.nvattest.source_archive.0,
                    bundle_archive: &self.nvattest.bundle_archive.0,
                    capture_root: &self.nvattest.capture_root.0,
                    output_root: &self.nvattest.output_root.0,
                },
            )?,
            restic::admit(restic::ResticInputPaths {
                receipt: &self.restic.build.receipt.0,
                evidence: &self.restic.build.evidence.0,
                validation: &self.restic.build.validation.0,
                source_archive: &self.restic.build.source_archive.0,
                go_archive: &self.restic.go_archive.0,
                modules_archive: &self.restic.modules_archive.0,
                output_root: &self.restic.build.output_root.0,
            })?,
            openmp.into_runtime(),
        ];
        AdmittedWindowsNativeInputs::from_controlled(controlled, architecture)?.with_archives(vec![
            admit_rclone(&self.rclone.archive.0, &self.rclone.license.0, architecture)?,
            admit_msvc(
                &self.msvc.archive.0,
                &self.msvc.runtime_license.0,
                architecture,
            )?,
            admit_pdfium(&self.pdfium_archive.0, architecture)?,
            admit_ffmpeg_notices(repo, &self.ffmpeg_archive.0)?,
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input_paths() -> serde_json::Value {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("input");
        let build = serde_json::json!({
            "receipt":path,"evidence":path,"validation":path,
            "source_archive":path,"output_root":path,
        });
        serde_json::json!({
            "llama":{"receipt":path,"evidence":path,"source_archive":path,"sdk_archive":path,"capture_root":path},
            "openmp":{"receipt":path,"evidence":path,"validation":path,"source_archive":path,"llvm_archive":path,"cmake_archive":path,"python_archive":path,"output_root":path,"development_root":path},
            "ced":build,
            "parakeet":{"build":build,"model":path},
            "onnx":{"build":build,"mirror_archive":path,"kleidiai_archive":path,"cmake_archive":path,"python_archive":path,"protoc_archive":path},
            "rfdetr":{"build":build,"ggml_bundle":path},
            "nvattest":{"receipt":path,"evidence":path,"source_archive":path,"bundle_archive":path,"capture_root":path,"output_root":path},
            "restic":{"build":build,"go_archive":path,"modules_archive":path},
            "rclone":{"archive":path,"license":path},
            "msvc":{"archive":path,"runtime_license":path},
            "pdfium_archive":path,"ffmpeg_archive":path,
        })
    }

    #[test]
    fn caller_target_selects_the_closed_input_schema() {
        let paths = input_paths();
        let bytes = serde_json::to_vec(&paths).unwrap();
        assert!(matches!(
            LocalWindowsInputs::decode(&bytes, WindowsArchitecture::Aarch64).unwrap(),
            LocalWindowsInputs::Arm64(_)
        ));
        assert!(LocalWindowsInputs::decode(&bytes, WindowsArchitecture::X86_64).is_err());
        for (key, value) in [
            ("target", serde_json::json!("windows-x86_64")),
            ("signing_key", serde_json::json!("foreign")),
        ] {
            let mut changed = paths.clone();
            changed[key] = value;
            assert!(
                LocalWindowsInputs::decode(
                    &serde_json::to_vec(&changed).unwrap(),
                    WindowsArchitecture::Aarch64
                )
                .is_err()
            );
        }
    }

    #[test]
    fn arm_paths_cannot_supply_legacy_receipt_controls_or_relative_outputs() {
        for (field, value) in [
            ("output_root", serde_json::json!("../foreign")),
            ("cmake_archive", serde_json::json!("legacy-extra-input")),
            ("trusted", serde_json::json!(true)),
        ] {
            let mut paths = input_paths();
            paths["ced"][field] = value;
            assert!(
                LocalWindowsInputs::decode(
                    &serde_json::to_vec(&paths).unwrap(),
                    WindowsArchitecture::Aarch64
                )
                .is_err()
            );
        }
    }
}
