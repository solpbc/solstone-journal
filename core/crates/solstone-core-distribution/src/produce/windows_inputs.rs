// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Pre-sign native input admission. Inventory owns installation destinations;
//! these values retain bytes under the original controlled-build output labels.
//! Historical product identities and original receipt/log bytes are preserved.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use super::windows_build::read_bounded;
use crate::artifact_verify::{
    ControlledBuildArtifactVerificationLimits, verify_controlled_build_artifacts,
};
use crate::controlled_build::{
    ControlledBuildReceipt, DependencySource, SupportingArtifactRef,
    decode_controlled_build_receipt,
};
use crate::digest::sha256_hex;

const DOCUMENT_LIMIT: u64 = 4 * 1024 * 1024;
const OUTPUT_LIMIT: u64 = 128 * 1024 * 1024;
const SOURCE_LIMIT: u64 = 512 * 1024 * 1024;
const TOOL_LIMIT: u64 = 128 * 1024 * 1024;

pub struct ControlledInputPaths<'a> {
    pub receipt: &'a Path,
    pub evidence: &'a Path,
    pub validation: &'a Path,
    pub source_archive: &'a Path,
    pub cmake_archive: &'a Path,
    pub output_root: &'a Path,
}

/// Constructed only by dependency-specific admission below. This is a retained
/// input to inventory staging, not another manifest or signing authority.
#[derive(Debug)]
pub struct AdmittedControlledInput {
    receipt: ControlledBuildReceipt,
    receipt_bytes: Vec<u8>,
    evidence_bytes: Vec<u8>,
    validation_bytes: Vec<u8>,
    outputs: BTreeMap<String, Vec<u8>>,
}

impl AdmittedControlledInput {
    pub fn receipt(&self) -> &ControlledBuildReceipt {
        &self.receipt
    }
    pub fn receipt_bytes(&self) -> &[u8] {
        &self.receipt_bytes
    }
    pub fn evidence_bytes(&self) -> &[u8] {
        &self.evidence_bytes
    }
    pub fn validation_bytes(&self) -> &[u8] {
        &self.validation_bytes
    }
    pub fn outputs(&self) -> &BTreeMap<String, Vec<u8>> {
        &self.outputs
    }

    pub(crate) fn new(
        receipt: ControlledBuildReceipt,
        receipt_bytes: Vec<u8>,
        evidence_bytes: Vec<u8>,
        validation_bytes: Vec<u8>,
        outputs: BTreeMap<String, Vec<u8>>,
    ) -> Self {
        Self {
            receipt,
            receipt_bytes,
            evidence_bytes,
            validation_bytes,
            outputs,
        }
    }

    pub(super) fn into_retained_members(mut self) -> Result<BTreeMap<String, Vec<u8>>, String> {
        for (label, bytes) in [
            ("receipt.json", self.receipt_bytes),
            ("build-evidence.json", self.evidence_bytes),
            ("validation.log", self.validation_bytes),
        ] {
            if self.outputs.insert(label.into(), bytes).is_some() {
                return Err("native output collides with retained evidence label".into());
            }
        }
        Ok(self.outputs)
    }
}

pub struct NvattestInputPaths<'a> {
    pub receipt: &'a Path,
    pub evidence: &'a Path,
    pub validation: &'a Path,
    pub source_archive: &'a Path,
    pub bundle_archive: &'a Path,
    pub output_root: &'a Path,
}

pub(crate) fn verify_receipt_document_binding(
    receipt: &ControlledBuildReceipt,
    evidence_bytes: &[u8],
    validation_bytes: &[u8],
    evidence_label: &str,
) -> Result<(), String> {
    if receipt.supporting
        != [SupportingArtifactRef {
            label: evidence_label.into(),
            sha256: sha256_hex(evidence_bytes),
        }]
        || validation_bytes.is_empty()
        || receipt.validation.sha256 != sha256_hex(validation_bytes)
    {
        return Err(
            "unbound-evidence: native receipt does not bind the original evidence and validation bytes".into(),
        );
    }
    Ok(())
}

struct CapturedBuild {
    receipt: ControlledBuildReceipt,
    receipt_bytes: Vec<u8>,
    evidence_bytes: Vec<u8>,
    validation_bytes: Vec<u8>,
}

impl CapturedBuild {
    fn read(paths: &ControlledInputPaths<'_>, evidence_label: &str) -> Result<Self, String> {
        let receipt_bytes = read_bounded(paths.receipt, DOCUMENT_LIMIT)?;
        let receipt = decode_controlled_build_receipt(&receipt_bytes).map_err(|e| e.to_string())?;
        let evidence_bytes = read_bounded(paths.evidence, DOCUMENT_LIMIT)?;
        let validation_bytes = read_bounded(paths.validation, DOCUMENT_LIMIT)?;
        verify_receipt_document_binding(
            &receipt,
            &evidence_bytes,
            &validation_bytes,
            evidence_label,
        )?;
        Ok(Self {
            receipt,
            receipt_bytes,
            evidence_bytes,
            validation_bytes,
        })
    }

    fn finish(
        self,
        output_root: &Path,
        expected_label: &str,
    ) -> Result<AdmittedControlledInput, String> {
        if self.receipt.outputs.len() != 1 || self.receipt.outputs[0].label != expected_label {
            return Err("native receipt has an unexpected output set".into());
        }
        verify_controlled_build_artifacts(
            output_root,
            &self.receipt,
            ControlledBuildArtifactVerificationLimits::new(
                16,
                2,
                (OUTPUT_LIMIT
                    + if expected_label == crate::parakeet_windows::PARAKEET_SERVER_OUTPUT_LABEL {
                        crate::parakeet_windows::PARAKEET_MODEL_SIZE_BYTES
                    } else {
                        0
                    }) as usize,
            ),
        )
        .map_err(|e| e.to_string())?;
        let output = &self.receipt.outputs[0];
        // Re-reading for staging must compare against the original admission,
        // never learn a replacement file's digest after the verifier returns.
        let bytes = capture_output(
            &super::windows_stage::join_components(output_root, expected_label),
            output,
        )?;
        let pe = crate::pe_dependencies::inspect_dependencies(&bytes)?;
        if pe.is_dll != expected_label.ends_with(".dll") {
            return Err(
                "native output PE kind differs from its declared library/command role".into(),
            );
        }
        Ok(AdmittedControlledInput {
            receipt: self.receipt,
            receipt_bytes: self.receipt_bytes,
            evidence_bytes: self.evidence_bytes,
            validation_bytes: self.validation_bytes,
            outputs: BTreeMap::from([(expected_label.into(), bytes)]),
        })
    }
}

/// Reuse CED's pinned source/archive, export, CPU-import and configuration
/// validators. Legacy evidence binds its cache digest; this admission does not
/// claim to have recovered or re-executed that historical CMake cache.
pub fn admit_ced(
    repo: &Path,
    paths: ControlledInputPaths<'_>,
) -> Result<AdmittedControlledInput, String> {
    use crate::ced_windows::*;
    use crate::ced_windows_source::{
        inspect_ced_windows_source_archive, inspect_cmake_windows_archive,
    };
    let captured = CapturedBuild::read(&paths, CED_WINDOWS_BUILD_EVIDENCE_LABEL)?;
    let evidence =
        decode_ced_windows_build_evidence(&captured.evidence_bytes).map_err(|e| e.to_string())?;
    with_snapshot(|snapshot| {
        let source_path = snapshot_file(
            snapshot,
            "source.tar.gz",
            paths.source_archive,
            SOURCE_LIMIT,
        )?;
        preflight_source_archive(&source_path)?;
        let source = inspect_ced_windows_source_archive(&source_path).map_err(|e| e.to_string())?;
        let tool_path = snapshot_file(snapshot, "cmake.zip", paths.cmake_archive, TOOL_LIMIT)?;
        let (_, cmake) =
            inspect_cmake_windows_archive(repo, &tool_path).map_err(|e| e.to_string())?;
        if evidence.source_archive != source.archive
            || evidence.cmake_archive != cmake
            || evidence.ggml != source.ggml
            || evidence.export_definition_sha256 != source.export_definition_sha256
            || captured.receipt.source.windows_dependency
                != (DependencySource {
                    repository: source.ced.repository,
                    revision: source.ced.revision,
                    content_sha256: source.archive.sha256,
                })
        {
            return Err("CED receipt evidence differs from pinned source/tool admission".into());
        }
        let mut expected = assemble_receipt_draft(
            captured.receipt.source.clone(),
            evidence,
            captured.receipt.builder.clone(),
            captured.receipt.outputs.clone(),
        )
        .map_err(|e| e.to_string())?;
        expected.schema = Some(captured.receipt.schema.clone());
        expected.validation = Some(captured.receipt.validation.clone());
        if expected.validate().map_err(|e| e.to_string())? != captured.receipt {
            return Err(
                "CED receipt differs from its admitted source/configuration/evidence".into(),
            );
        }
        let mut admitted = captured.finish(paths.output_root, CED_DLL_OUTPUT_LABEL)?;
        attach_source_notices(&mut admitted, &source_path, CED_NOTICES)?;
        Ok(admitted)
    })
}

/// Admit the original static CPU server receipt and independently pin the model
/// bytes that inventory will stage. Neither source nor model pins are restamped.
pub fn admit_parakeet(
    repo: &Path,
    paths: ControlledInputPaths<'_>,
    model_path: &Path,
) -> Result<AdmittedControlledInput, String> {
    use crate::parakeet_windows::*;
    use crate::parakeet_windows_source::{
        PARAKEET_WINDOWS_BUILD_EVIDENCE_LABEL, ParakeetWindowsBuildEvidence,
        inspect_parakeet_windows_source_archive, parakeet_controlled_build_configuration,
    };
    let captured = CapturedBuild::read(&paths, PARAKEET_WINDOWS_BUILD_EVIDENCE_LABEL)?;
    let evidence: ParakeetWindowsBuildEvidence =
        serde_json::from_slice(&captured.evidence_bytes).map_err(|e| e.to_string())?;
    crate::parakeet_windows_source::validate_build_evidence(&evidence)
        .map_err(|e| e.to_string())?;
    with_snapshot(|snapshot| {
        let source_path = snapshot_file(
            snapshot,
            "source.tar.gz",
            paths.source_archive,
            SOURCE_LIMIT,
        )?;
        preflight_source_archive(&source_path)?;
        let source =
            inspect_parakeet_windows_source_archive(&source_path).map_err(|e| e.to_string())?;
        let tool_path = snapshot_file(snapshot, "cmake.zip", paths.cmake_archive, TOOL_LIMIT)?;
        let (_, cmake) = crate::ced_windows_source::inspect_cmake_windows_archive(repo, &tool_path)
            .map_err(|e| e.to_string())?;
        if evidence.source_archive != source.archive
            || evidence.cmake_archive != cmake
            || captured.receipt.source.windows_dependency
                != (DependencySource {
                    repository: source.parakeet.repository,
                    revision: source.parakeet.revision,
                    content_sha256: source.archive.sha256,
                })
            || captured.receipt.inputs != [evidence.source_archive, cmake, evidence.model]
            || captured.receipt.configuration != parakeet_controlled_build_configuration()
            || captured.receipt.outputs.len() != 1
        {
            return Err(
                "Parakeet receipt differs from pinned source/configuration/input evidence".into(),
            );
        }
        verify_import_closure(&captured.receipt.outputs[0].census).map_err(|e| e.to_string())?;
        let model = read_bounded(model_path, PARAKEET_MODEL_SIZE_BYTES)?;
        require_identity(&model, PARAKEET_MODEL_SIZE_BYTES, PARAKEET_MODEL_SHA256)?;
        let mut admitted = captured.finish(paths.output_root, PARAKEET_SERVER_OUTPUT_LABEL)?;
        admitted
            .outputs
            .insert(PARAKEET_MODEL_OUTPUT_LABEL.into(), model);
        attach_source_notices(&mut admitted, &source_path, PARAKEET_NOTICES)?;
        Ok(admitted)
    })
}

pub struct OnnxInputPaths<'a> {
    pub build: ControlledInputPaths<'a>,
    pub mirror_archive: &'a Path,
    pub python_archive: &'a Path,
    pub protoc_archive: &'a Path,
    pub cmake_cache: &'a Path,
}

pub struct RfdetrInputPaths<'a> {
    /// `source_archive` is the exact RF Git bundle for this dependency.
    pub build: ControlledInputPaths<'a>,
    pub ggml_bundle: &'a Path,
    pub cmake_cache: &'a Path,
    pub build_options: &'a Path,
    pub subprocess_evidence: &'a Path,
    pub license: &'a Path,
    pub ggml_license: &'a Path,
    /// Original identical license block from the pinned stb header files.
    pub stb_license: &'a Path,
}

/// Reuse the RF recorder's source/configuration/stream validators against a
/// bounded private snapshot. Original evidence is never serialized again.
pub fn admit_rfdetr(
    repo: &Path,
    paths: RfdetrInputPaths<'_>,
) -> Result<AdmittedControlledInput, String> {
    use crate::rfdetr_windows::*;
    use crate::rfdetr_windows_source::*;
    let captured = CapturedBuild::read(&paths.build, RFDETR_WINDOWS_BUILD_EVIDENCE_LABEL)?;
    let evidence = decode_rfdetr_windows_build_evidence(&captured.evidence_bytes)
        .map_err(|e| e.to_string())?;
    evidence
        .validate_against_source(&captured.receipt.source)
        .map_err(|e| e.to_string())?;
    with_snapshot(|snapshot| {
        let rf_path = snapshot_file(
            snapshot,
            "rf.bundle",
            paths.build.source_archive,
            SOURCE_LIMIT,
        )?;
        let ggml_path = snapshot_file(snapshot, "ggml.bundle", paths.ggml_bundle, SOURCE_LIMIT)?;
        let cmake_path =
            snapshot_file(snapshot, "cmake.zip", paths.build.cmake_archive, TOOL_LIMIT)?;
        let rf = inspect_git_bundle(&rf_path, RFDETR_WINDOWS_RF_BUNDLE_LABEL)
            .map_err(|e| e.to_string())?;
        let ggml = inspect_git_bundle(&ggml_path, RFDETR_WINDOWS_GGML_BUNDLE_LABEL)
            .map_err(|e| e.to_string())?;
        let (_, cmake) =
            crate::ced_windows_source::inspect_cmake_windows_archive(repo, &cmake_path)
                .map_err(|e| e.to_string())?;
        if evidence.rf_bundle != rf
            || evidence.ggml_bundle != ggml
            || evidence.cmake_archive != cmake
        {
            return Err("RF receipt differs from exact source/tool input admission".into());
        }
        let mut expected = assemble_receipt_draft(
            captured.receipt.source.clone(),
            evidence,
            captured.receipt.builder.clone(),
            captured.receipt.outputs.clone(),
        )
        .map_err(|e| e.to_string())?;
        expected.schema = Some(captured.receipt.schema.clone());
        expected.validation = Some(captured.receipt.validation.clone());
        if expected.validate().map_err(|e| e.to_string())? != captured.receipt {
            return Err(
                "RF receipt differs from admitted source/configuration/output evidence".into(),
            );
        }
        let cache = snapshot_file(
            snapshot,
            "CMakeCache.txt",
            paths.cmake_cache,
            DOCUMENT_LIMIT,
        )?;
        let options = snapshot_file(
            snapshot,
            "rfdetr-cli.vcxproj",
            paths.build_options,
            DOCUMENT_LIMIT,
        )?;
        let subprocess = snapshot_rfdetr_streams(snapshot, paths.subprocess_evidence)?;
        // Verify the original output tree census before retaining the bytes.
        let mut admitted = captured.finish(paths.build.output_root, RFDETR_CLI_OUTPUT_LABEL)?;
        let receipt_path = snapshot.join("receipt.json");
        let evidence_path = snapshot.join("evidence.json");
        fs::write(&receipt_path, admitted.receipt_bytes()).map_err(|e| e.to_string())?;
        fs::write(&evidence_path, admitted.evidence_bytes()).map_err(|e| e.to_string())?;
        let output_root = snapshot.join("output");
        fs::create_dir_all(output_root.join("bin")).map_err(|e| e.to_string())?;
        fs::write(
            output_root.join(RFDETR_CLI_OUTPUT_LABEL),
            &admitted.outputs()[RFDETR_CLI_OUTPUT_LABEL],
        )
        .map_err(|e| e.to_string())?;
        verify_rfdetr_windows_assembly(
            &receipt_path,
            &output_root,
            &evidence_path,
            &cache,
            &options,
            &subprocess,
        )
        .map_err(|e| e.to_string())?;
        for (path, notice) in [
            (paths.license, &RF_NOTICE),
            (paths.ggml_license, &GGML_NOTICE),
            (paths.stb_license, &STB_NOTICE),
        ] {
            let bytes = read_bounded(path, notice.bytes)?;
            require_identity(&bytes, notice.bytes, notice.sha256)?;
            admitted.outputs.insert(notice.member.into(), bytes);
        }
        Ok(admitted)
    })
}

/// The NVIDIA GPU verifier. The input file names paths only; every pin comes
/// from the committed constants, and nothing here can supply another value.
pub fn admit_nvattest(
    repo: &Path,
    paths: NvattestInputPaths<'_>,
    msvc: crate::nvattest_windows::MsvcRuntimeBytes<'_>,
) -> Result<AdmittedControlledInput, String> {
    admit_nvattest_under(
        repo,
        &paths,
        &crate::nvattest_windows::production_pins(),
        &msvc,
    )
}

/// Routine tests' fixture-pin seam; never compiled into the producer.
#[cfg(test)]
pub(crate) fn admit_nvattest_with_pins(
    repo: &Path,
    paths: NvattestInputPaths<'_>,
    pins: &crate::nvattest_windows::Pins,
    msvc: crate::nvattest_windows::MsvcRuntimeBytes<'_>,
) -> Result<AdmittedControlledInput, String> {
    admit_nvattest_under(repo, &paths, pins, &msvc)
}

fn admit_nvattest_under(
    repo: &Path,
    paths: &NvattestInputPaths<'_>,
    pins: &crate::nvattest_windows::Pins,
    msvc: &crate::nvattest_windows::MsvcRuntimeBytes<'_>,
) -> Result<AdmittedControlledInput, String> {
    use crate::nvattest_windows::*;
    // The bundle's largest member, the Strawberry Perl archive, is 299,399,930
    // bytes; 512 MiB admits it with margin. Each whole archive stays bounded by
    // its pinned size, since the streams are uncompressed tar.
    const BUNDLE_MEMBER_LIMIT: u64 = 512 * 1024 * 1024;
    const SOURCE_MEMBER_LIMIT: u64 = 64 * 1024 * 1024;
    let receipt_bytes = read_bounded(paths.receipt, DOCUMENT_LIMIT)?;
    let evidence_bytes = read_bounded(paths.evidence, DOCUMENT_LIMIT)?;
    let validation_bytes = read_bounded(paths.validation, DOCUMENT_LIMIT)?;
    let notices_index = read_bounded(
        &super::windows_stage::join_components(repo, NVATTEST_NOTICES_INDEX_PATH),
        DOCUMENT_LIMIT,
    )?;
    let notices_body = read_bounded(
        &super::windows_stage::join_components(repo, NVATTEST_NOTICES_BODY_PATH),
        DOCUMENT_LIMIT,
    )?;
    with_snapshot(|snapshot| {
        let source_path = snapshot_file(
            snapshot,
            "source.tar",
            paths.source_archive,
            pins.source_archive.bytes,
        )
        .map_err(|e| format!("source-archive: {e}"))?;
        let bundle_path = snapshot_file(
            snapshot,
            "bundle.tar",
            paths.bundle_archive,
            pins.bundle_archive.bytes,
        )
        .map_err(|e| format!("bundle-archive: {e}"))?;
        for (path, pin, member_limit) in [
            (&source_path, &pins.source_archive, SOURCE_MEMBER_LIMIT),
            (&bundle_path, &pins.bundle_archive, BUNDLE_MEMBER_LIMIT),
        ] {
            preflight_tar_stream(
                fs::File::open(path).map_err(|e| e.to_string())?,
                pin.bytes,
                member_limit,
                1024 * 1024,
                100_000,
            )?;
        }
        let source_bytes = fs::read(&source_path).map_err(|e| e.to_string())?;
        let bundle_bytes = fs::read(&bundle_path).map_err(|e| e.to_string())?;
        let output = |label: &str, limit: u64| {
            read_bounded(
                &super::windows_stage::join_components(paths.output_root, label),
                limit,
            )
        };
        let output_exe = output(NVATTEST_EXE_OUTPUT_LABEL, OUTPUT_LIMIT)?;
        let output_license = output(NVATTEST_LICENSE_LABEL, DOCUMENT_LIMIT)?;
        let [msvcp140, vcruntime140, vcruntime140_1] =
            NVATTEST_RUNTIME_OUTPUT_LABELS.map(|label| output(label, OUTPUT_LIMIT));
        let (msvcp140, vcruntime140, vcruntime140_1) = (msvcp140?, vcruntime140?, vcruntime140_1?);
        let admitted = admit(
            pins,
            &AdmissionBytes {
                receipt: &receipt_bytes,
                evidence: &evidence_bytes,
                validation: &validation_bytes,
                notices_index: &notices_index,
                notices_body: &notices_body,
                source_archive: &source_bytes,
                bundle_archive: &bundle_bytes,
                output_exe: &output_exe,
                output_license: &output_license,
                output_msvcp140: &msvcp140,
                output_vcruntime140: &vcruntime140,
                output_vcruntime140_1: &vcruntime140_1,
            },
            msvc,
        )?;
        // Re-read through the retained root; the staged bytes stay the ones
        // admitted above, never a replacement learned after this returns.
        verify_controlled_build_artifacts(
            paths.output_root,
            admitted.receipt(),
            crate::nvattest_windows_source::OUTPUT_LIMITS,
        )
        .map_err(|e| e.to_string())?;
        Ok(admitted.into_admitted_controlled_input())
    })
}

/// Paths only: the receipt replays the original capture and supplies no trust override.
pub struct LlamaInputPaths<'a> {
    pub receipt: &'a Path,
    pub evidence: &'a Path,
    pub source_archive: &'a Path,
    pub sdk_archive: &'a Path,
    pub cmake_archive: &'a Path,
    pub capture_root: &'a Path,
}

/// Retain exactly the pre-sign engine/loader bytes revalidated by the capture
/// authority. Package signing must not rewrite the historical receipt.
pub fn admit_llama(
    repo: &Path,
    paths: LlamaInputPaths<'_>,
) -> Result<AdmittedControlledInput, String> {
    use crate::{llama_windows_capture as capture, llama_windows_source as source};
    let receipt_bytes = read_bounded(paths.receipt, DOCUMENT_LIMIT)?;
    let receipt = decode_controlled_build_receipt(&receipt_bytes).map_err(|e| e.to_string())?;
    let evidence_bytes = read_bounded(paths.evidence, DOCUMENT_LIMIT)?;
    with_snapshot(|snapshot| {
        let archive = snapshot_file(
            snapshot,
            "llama.tar.gz",
            paths.source_archive,
            source::SOURCE_BYTES,
        )?;
        let (source_identity, census) =
            source::inspect_source_with_census(&archive).map_err(|e| e.to_string())?;
        let sdk = source::inspect_sdk(paths.sdk_archive).map_err(|e| e.to_string())?;
        let cmake_path = snapshot_file(snapshot, "cmake.zip", paths.cmake_archive, TOOL_LIMIT)?;
        let (_, cmake) =
            crate::ced_windows_source::inspect_cmake_windows_archive(repo, &cmake_path)
                .map_err(|e| e.to_string())?;
        let files = capture::read_capture(paths.capture_root)?;
        let evidence = capture::inspect(&files, &census, &source_identity, &sdk, &cmake)?;
        let (expected, expected_evidence) = capture::receipt(
            &evidence,
            &files,
            vec![source_identity, sdk, cmake],
            &receipt.builder.host,
        )?;
        if receipt != expected || evidence_bytes != expected_evidence {
            return Err("llama receipt/evidence differs from original source, tools, capture or pre-sign bytes".into());
        }
        let outputs = ["bin/llama-server.exe", "bin/vulkan-1.dll"]
            .into_iter()
            .map(|label| (label.to_owned(), files[&format!("output/{label}")].clone()))
            .collect();
        let mut admitted = AdmittedControlledInput {
            receipt,
            receipt_bytes,
            evidence_bytes,
            validation_bytes: files["report/execution.json"].clone(),
            outputs,
        };
        attach_source_notices(&mut admitted, &archive, LLAMA_NOTICES)?;
        // These embedded license comments are copied verbatim from the exact
        // pinned source files above, including upstream copyright attribution.
        for (member, destination, first) in [
            ("loader/loader/cJSON.h", "notices/cJSON-LICENSE.txt", true),
            (
                "loader/loader/dirent_on_windows.c",
                "notices/dirent-LICENSE.txt",
                false,
            ),
            (
                "llama/vendor/stb/stb_image.h",
                "notices/stb-LICENSE.txt",
                false,
            ),
            (
                "llama/vendor/miniaudio/miniaudio.h",
                "notices/miniaudio-LICENSE.txt",
                false,
            ),
        ] {
            let bytes = admitted
                .outputs
                .remove(member)
                .ok_or("missing embedded source notice")?;
            let text = std::str::from_utf8(&bytes).map_err(|e| e.to_string())?;
            let start = if first {
                text.find("/*")
            } else {
                text.rfind("/*")
            }
            .ok_or("missing license comment")?;
            let end = text[start..]
                .find("*/")
                .ok_or("unterminated license comment")?
                + start
                + 2;
            if admitted
                .outputs
                .insert(destination.into(), text.as_bytes()[start..end].to_vec())
                .is_some()
            {
                return Err("duplicate llama notice destination".into());
            }
        }
        Ok(admitted)
    })
}

const LLAMA_NOTICES: &[SourceNotice] = &[
    SourceNotice {
        member: "llama/LICENSE",
        bytes: 1078,
        sha256: "94f29bbed6a22c35b992c5c6ebf0e7c92f13b836b90f36f461c9cf2f0f1d010d",
    },
    SourceNotice {
        member: "llama/licenses/LICENSE-jsonhpp",
        bytes: 1075,
        sha256: "c0d068392ea65358b798b8c165103560f06e9e3b38c4ab4e2d8810a7b931af86",
    },
    SourceNotice {
        member: "llama/vendor/cpp-httplib/LICENSE",
        bytes: 1075,
        sha256: "4b45cbe16d7b71b89ae6127e26e0d90a029198ca5e958ad8e3d0b8bbed364d8b",
    },
    SourceNotice {
        member: "loader/LICENSE.txt",
        bytes: 11661,
        sha256: "43c0a37e6a0fa7ff3c843b3ec5a4fac84b712558ddac103fbd4c1649662a9ece",
    },
    SourceNotice {
        member: "loader/LICENSES/Apache-2.0.txt",
        bytes: 10280,
        sha256: "074e6e32c86a4c0ef8b3ed25b721ca23aca83df277cd88106ef7177c354615ff",
    },
    SourceNotice {
        member: "loader/LICENSES/MIT-Khronos-old.txt",
        bytes: 1305,
        sha256: "fbeaca472f4f70e276dd1106ca5097435967a22ad6c1d8200aef7ad9f70aaf3f",
    },
    SourceNotice {
        member: "loader/LICENSES/MIT.txt",
        bytes: 1078,
        sha256: "b05785f9f18e6716bab63424b11454513b9943a222595b70411009202fc592b5",
    },
    SourceNotice {
        member: "loader/loader/cJSON.h",
        bytes: 9870,
        sha256: "27b4640a1cd36cb9e3da2c1e63d66ba4d70b4ace0d8d4d5b6c09cc775d6489f3",
    },
    SourceNotice {
        member: "loader/loader/dirent_on_windows.c",
        bytes: 3797,
        sha256: "4d4181b0018d7ddc29a866aebcc0f5a1b6574de56a3ef574b20698792b325338",
    },
    SourceNotice {
        member: "llama/vendor/stb/stb_image.h",
        bytes: 283010,
        sha256: "594c2fe35d49488b4382dbfaec8f98366defca819d916ac95becf3e75f4200b3",
    },
    SourceNotice {
        member: "llama/vendor/miniaudio/miniaudio.h",
        bytes: 4108168,
        sha256: "ac7af4de748b7e26b777f37e01cee313a308a7296a3eb080e2906b320cc55c89",
    },
];

fn snapshot_rfdetr_streams(snapshot: &Path, source: &Path) -> Result<PathBuf, String> {
    snapshot_rfdetr_streams_with_limit(snapshot, source, 256 * 1024 * 1024)
}

fn snapshot_rfdetr_streams_with_limit(
    snapshot: &Path,
    source: &Path,
    limit: usize,
) -> Result<PathBuf, String> {
    use crate::rfdetr_windows_source::{SubprocessEvidenceFileEntry, read_subprocess_stream};
    let raw = read_bounded(source, DOCUMENT_LIMIT)?;
    let entries: Vec<SubprocessEvidenceFileEntry> =
        serde_json::from_slice(&raw).map_err(|e| e.to_string())?;
    if entries.len() > 64 {
        return Err("RF subprocess evidence exceeds entry limit".into());
    }
    let logs = snapshot.join("logs");
    fs::create_dir(&logs).map_err(|e| e.to_string())?;
    let mut remaining = limit;
    for entry in entries {
        for (kind, path) in [("stdout", entry.stdout_path), ("stderr", entry.stderr_path)] {
            // Existing helper enforces safe declared relative names, containment
            // and its per-stream bound before any snapshot path is constructed.
            let bytes = read_subprocess_stream(source, &path, &entry.label, kind)
                .map_err(|e| e.to_string())?;
            remaining = remaining
                .checked_sub(bytes.len())
                .ok_or("RF subprocess evidence exceeds total byte limit")?;
            let dest = logs.join(format!("{}.{kind}", entry.label));
            use std::io::Write;
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(dest)
                .and_then(|mut file| file.write_all(&bytes))
                .map_err(|e| e.to_string())?;
        }
    }
    let path = snapshot.join("subprocess.json");
    fs::write(&path, raw).map_err(|e| e.to_string())?;
    Ok(path)
}

/// Admit the pinned ORT source and exact offline builder inputs. The retained
/// cache is hash-bound to its original sidecar, not regenerated configuration.
pub fn admit_onnx(
    repo: &Path,
    paths: OnnxInputPaths<'_>,
) -> Result<AdmittedControlledInput, String> {
    use crate::onnx_windows_source::*;
    let captured = CapturedBuild::read(&paths.build, ONNX_WINDOWS_BUILD_EVIDENCE_LABEL)?;
    let evidence: OnnxWindowsBuildEvidence =
        serde_json::from_slice(&captured.evidence_bytes).map_err(|e| e.to_string())?;
    validate_build_evidence(&evidence).map_err(|e| e.to_string())?;
    with_snapshot(|snapshot| {
        let source_path = snapshot_file(
            snapshot,
            "source.tar.gz",
            paths.build.source_archive,
            SOURCE_LIMIT,
        )?;
        preflight_source_archive(&source_path)?;
        let source =
            inspect_onnx_windows_source_archive(&source_path).map_err(|e| e.to_string())?;
        let mirror_path = snapshot_file(
            snapshot,
            "mirror.tar.gz",
            paths.mirror_archive,
            SOURCE_LIMIT,
        )?;
        preflight_source_archive(&mirror_path)?;
        let mirror =
            inspect_onnx_windows_mirror_archive(repo, &mirror_path).map_err(|e| e.to_string())?;
        let cmake_path =
            snapshot_file(snapshot, "cmake.zip", paths.build.cmake_archive, TOOL_LIMIT)?;
        let python_path = snapshot_file(snapshot, "python.zip", paths.python_archive, TOOL_LIMIT)?;
        let protoc_path = snapshot_file(snapshot, "protoc.zip", paths.protoc_archive, TOOL_LIMIT)?;
        let cmake = inspect_cmake_archive(repo, &cmake_path).map_err(|e| e.to_string())?;
        let python = inspect_python_archive(repo, &python_path).map_err(|e| e.to_string())?;
        let protoc = inspect_protoc_archive(repo, &protoc_path).map_err(|e| e.to_string())?;
        let cache = read_bounded(paths.cmake_cache, DOCUMENT_LIMIT)?;
        if cache.is_empty()
            || sha256_hex(&cache) != evidence.cmake_cache_sha256
            || evidence.source_archive != source.archive
            || evidence.mirror_archive != mirror.archive
            || evidence.cmake_archive != cmake
            || evidence.python_archive != python
            || evidence.protoc_archive != protoc
            || captured.receipt.source.windows_dependency
                != (DependencySource {
                    repository: source.onnxruntime.repository,
                    revision: source.onnxruntime.revision,
                    content_sha256: source.archive.sha256,
                })
            || captured.receipt.inputs
                != [
                    evidence.source_archive,
                    mirror.archive,
                    cmake,
                    python,
                    protoc,
                ]
            || captured.receipt.configuration != onnx_controlled_build_configuration()
        {
            return Err(
                "ONNX receipt differs from pinned source/configuration/input evidence".into(),
            );
        }
        validate_onnx_cache(&cache)?;
        if !captured.receipt.outputs.iter().any(|output| {
            output.census.exports.iter().any(|symbol| {
            matches!(symbol, crate::pe::PeSymbol::Named(name) if name == "OrtGetApiBase")
        })
        }) {
            return Err("ONNX runtime does not export OrtGetApiBase".into());
        }
        let mut admitted =
            captured.finish(paths.build.output_root, ONNX_WINDOWS_DLL_OUTPUT_LABEL)?;
        attach_source_notices(&mut admitted, &source_path, ONNX_NOTICES)?;
        Ok(admitted)
    })
}

struct SourceNotice {
    member: &'static str,
    bytes: u64,
    sha256: &'static str,
}

// Original notice bytes from the exact source archives admitted above. Their
// labels are source members; only inventory assigns installation destinations.
const GGML_NOTICE: SourceNotice = SourceNotice {
    member: "third_party/ggml/LICENSE",
    bytes: 1078,
    sha256: "94f29bbed6a22c35b992c5c6ebf0e7c92f13b836b90f36f461c9cf2f0f1d010d",
};
const RF_NOTICE: SourceNotice = SourceNotice {
    member: "LICENSE",
    bytes: 11369,
    sha256: "c5cb7d6c35a9a9a07563274ca608a012ca740770331e4f49cb17b00e6520bffa",
};
const STB_NOTICE: SourceNotice = SourceNotice {
    member: "notices/stb-LICENSE.txt",
    bytes: 2434,
    sha256: "02513c7c7041500985a4478ba8d23f90dc861ed19a42b6d1b5200d1e808d4024",
};

/// Source notices only: this cannot admit FFmpeg libraries or final product
/// executables. Their build evidence belongs to the live Cargo constructor.
pub fn admit_ffmpeg_notices(
    repo: &Path,
    archive: &Path,
) -> Result<super::windows_archives::AdmittedArchiveInput, String> {
    let config = read_bounded(
        &super::windows_stage::join_components(repo, "core/distribution/builder-inputs.toml"),
        DOCUMENT_LIMIT,
    )?;
    let pin = solstone_core_ffmpeg_build_support::parse_ffmpeg_pin(
        std::str::from_utf8(&config).map_err(|e| e.to_string())?,
    )?;
    with_snapshot(|snapshot| {
        let bytes = read_bounded(archive, SOURCE_LIMIT)?;
        require_identity(&bytes, pin.size, &pin.sha256)?;
        let path = snapshot.join("ffmpeg.tar.gz");
        fs::write(&path, bytes).map_err(|e| e.to_string())?;
        preflight_source_archive(&path)?;
        let mut members = source_notice_members(&path, FFMPEG_NOTICES)?;
        members.insert("archive.json".into(), serde_json::to_vec(&serde_json::json!({
            "source_commit": pin.commit, "url": pin.url, "bytes": pin.size, "sha256": pin.sha256,
        })).map_err(|e| e.to_string())?);
        Ok(super::windows_archives::AdmittedArchiveInput {
            component: crate::inventory::WindowsNativeComponent::Ffmpeg,
            members,
        })
    })
}
const FFMPEG_NOTICES: &[SourceNotice] = &[
    SourceNotice {
        member: "FFmpeg-946fcce07b6dcd0331c8cc609192aeff5e1924f8/COPYING.GPLv2",
        bytes: 18092,
        sha256: "8177f97513213526df2cf6184d8ff986c675afb514d4e68a404010521b880643",
    },
    SourceNotice {
        member: "FFmpeg-946fcce07b6dcd0331c8cc609192aeff5e1924f8/COPYING.GPLv3",
        bytes: 35147,
        sha256: "8ceb4b9ee5adedde47b31e975c1d90c73ad27b6b165a1dcd80c7c545eb65b903",
    },
    SourceNotice {
        member: "FFmpeg-946fcce07b6dcd0331c8cc609192aeff5e1924f8/COPYING.LGPLv2.1",
        bytes: 26517,
        sha256: "246041b6ecf9bc32d718a62c57877c78b5eb397b6467e74ed7ae2626ab189c30",
    },
    SourceNotice {
        member: "FFmpeg-946fcce07b6dcd0331c8cc609192aeff5e1924f8/COPYING.LGPLv3",
        bytes: 7651,
        sha256: "da7eabb7bafdf7d3ae5e9f223aa5bdc1eece45ac569dc21b3b037520b4464768",
    },
    SourceNotice {
        member: "FFmpeg-946fcce07b6dcd0331c8cc609192aeff5e1924f8/LICENSE.md",
        bytes: 4346,
        sha256: "2e1d16c72fd74e12063776371da757322f8b77589386532f4fd8634bde7de1af",
    },
];
const CED_NOTICES: &[SourceNotice] = &[
    SourceNotice {
        member: "LICENSE",
        bytes: 1076,
        sha256: "dbf9b4d95935d8aba68a8f47ad4a92eb93c969d9863893d2f47e804fc150030e",
    },
    GGML_NOTICE,
];
const PARAKEET_NOTICES: &[SourceNotice] = &[
    SourceNotice {
        member: "LICENSE",
        bytes: 1081,
        sha256: "396cb1a512310cb4fabd73118114c8cb53c7352955a37b583e165f15af64f095",
    },
    GGML_NOTICE,
];
const ONNX_NOTICES: &[SourceNotice] = &[
    SourceNotice {
        member: "LICENSE",
        bytes: 1073,
        sha256: "2f07c72751aed99790b8a4869cf2311df85a860b22ded05fa22803587a48922c",
    },
    SourceNotice {
        member: "ThirdPartyNotices.txt",
        bytes: 325054,
        sha256: "0e07b95f3a8d6230037707c5c4a2b554d12c4cb67369669ac255635528ffcee2",
    },
];

fn attach_source_notices(
    admitted: &mut AdmittedControlledInput,
    snapshot: &Path,
    notices: &[SourceNotice],
) -> Result<(), String> {
    for (member, bytes) in source_notice_members(snapshot, notices)? {
        if admitted.outputs.insert(member, bytes).is_some() {
            return Err("source notice collides with admitted native member".into());
        }
    }
    Ok(())
}

fn source_notice_members(
    snapshot: &Path,
    notices: &[SourceNotice],
) -> Result<BTreeMap<String, Vec<u8>>, String> {
    // This private snapshot has already passed the complete bounded source
    // preflight and dependency validator. Raw iteration avoids extension-body
    // allocation again; these pinned notice members have ordinary short names.
    let file = fs::File::open(snapshot).map_err(|e| e.to_string())?;
    let reader = flate2::read::GzDecoder::new(file).take(2 * 1024 * 1024 * 1024 + 1);
    let mut archive = tar::Archive::new(reader);
    let mut found = BTreeMap::new();
    for entry in archive.entries().map_err(|e| e.to_string())?.raw(true) {
        let mut entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path().map_err(|e| e.to_string())?;
        let Some(notice) = notices.iter().find(|n| path == Path::new(n.member)) else {
            continue;
        };
        if !entry.header().entry_type().is_file() || entry.size() != notice.bytes {
            return Err(format!(
                "source notice has unexpected type/size: {}",
                notice.member
            ));
        }
        let mut bytes = Vec::new();
        (&mut entry)
            .take(notice.bytes + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        require_identity(&bytes, notice.bytes, notice.sha256)?;
        if found.insert(notice.member.into(), bytes).is_some() {
            return Err("duplicate source notice member".into());
        }
    }
    if found.len() != notices.len() {
        return Err("missing source notice member".into());
    }
    Ok(found)
}

fn validate_onnx_cache(bytes: &[u8]) -> Result<(), String> {
    let mut cache = BTreeMap::new();
    for line in std::str::from_utf8(bytes)
        .map_err(|e| e.to_string())?
        .lines()
    {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with("//") {
            continue;
        }
        let (key, value) = line.split_once('=').ok_or("invalid ONNX cache entry")?;
        let key = key.split_once(':').map_or(key, |(key, _)| key);
        if cache.insert(key, value).is_some() {
            return Err(format!("duplicate ONNX cache key {key}"));
        }
    }
    for (key, value) in [
        ("onnxruntime_BUILD_SHARED_LIB", "ON"),
        ("onnxruntime_DISABLE_CONTRIB_OPS", "ON"),
        ("onnxruntime_DISABLE_ML_OPS", "ON"),
        ("onnxruntime_USE_TELEMETRY", "OFF"),
        ("CMAKE_MSVC_RUNTIME_LIBRARY", "MultiThreadedDLL"),
        ("onnxruntime_REDUCED_OPS_BUILD", "ON"),
        ("CMAKE_GENERATOR_PLATFORM", "x64"),
    ] {
        if cache.get(key) != Some(&value) {
            return Err(format!("ONNX cache does not retain {key}={value}"));
        }
    }
    Ok(())
}

fn capture_output(
    path: &Path,
    output: &crate::controlled_build::OutputIdentityEntry,
) -> Result<Vec<u8>, String> {
    let bytes = read_bounded(path, OUTPUT_LIMIT)?;
    require_identity(&bytes, output.size, &output.pre_signing_sha256)?;
    Ok(bytes)
}

fn require_identity(bytes: &[u8], size: u64, sha256: &str) -> Result<(), String> {
    if bytes.len() as u64 != size || sha256_hex(bytes) != sha256 {
        return Err("native input size or SHA-256 differs from its original admission".into());
    }
    Ok(())
}

fn with_snapshot<T>(operation: impl FnOnce(&Path) -> Result<T, String>) -> Result<T, String> {
    let snapshot = tempfile::tempdir().map_err(|e| e.to_string())?;
    let result = operation(snapshot.path());
    match (result, snapshot.close()) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(cleanup)) => Err(format!("native input snapshot cleanup failed: {cleanup}")),
        (Err(error), Err(cleanup)) => Err(format!(
            "{error}; native input snapshot cleanup failed: {cleanup}"
        )),
    }
}

fn snapshot_file(root: &Path, leaf: &str, source: &Path, limit: u64) -> Result<PathBuf, String> {
    let bytes = read_bounded(source, limit)?;
    let path = root.join(leaf);
    fs::write(&path, bytes).map_err(|e| e.to_string())?;
    Ok(path)
}

// Existing source inspectors perform semantic/source-pin checks after this pass.
// The outer Take bounds all decoder output, including tar metadata and padding.
// Raw entries keep tar from allocating GNU/PAX bodies before their size is checked.
fn preflight_source_archive(path: &Path) -> Result<(), String> {
    let file = fs::File::open(path).map_err(|e| e.to_string())?;
    preflight_tar_stream(
        flate2::read::GzDecoder::new(file),
        2 * 1024 * 1024 * 1024,
        256 * 1024 * 1024,
        1024 * 1024,
        100_000,
    )
}

fn preflight_tar_stream(
    reader: impl Read,
    expanded_limit: u64,
    member_limit: u64,
    extension_limit: u64,
    entry_limit: usize,
) -> Result<(), String> {
    // One additional byte distinguishes exact-budget EOF from excess output.
    let budget = expanded_limit
        .checked_add(1)
        .ok_or("archive budget overflow")?;
    let mut bounded = reader.take(budget);
    {
        let mut archive = tar::Archive::new(&mut bounded);
        for (index, entry) in archive
            .entries()
            .map_err(|e| e.to_string())?
            .raw(true)
            .enumerate()
        {
            if index >= entry_limit {
                return Err("native source archive exceeds entry limit".into());
            }
            let mut entry = entry.map_err(|e| e.to_string())?;
            let declared = entry.size();
            let kind = entry.header().entry_type();
            if (kind.is_gnu_longname()
                || kind.is_gnu_longlink()
                || kind.is_pax_local_extensions()
                || kind.is_pax_global_extensions())
                && declared > extension_limit
            {
                return Err("native source extension exceeds byte limit".into());
            }
            if declared > member_limit {
                return Err("native source member exceeds byte limit".into());
            }
            let actual = io::copy(&mut entry, &mut io::sink()).map_err(|e| e.to_string())?;
            if actual != declared {
                return Err("native source archive member is truncated".into());
            }
        }
    }
    // Tar stops at its end marker. Account for the rest of the gzip stream too,
    // and force the decoder to validate its trailer before admitting the input.
    io::copy(&mut bounded, &mut io::sink()).map_err(|e| e.to_string())?;
    if budget - bounded.limit() > expanded_limit {
        return Err("native source archive exceeds expanded byte limit".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn source_notices_require_original_bytes_and_exact_members() {
        let root = tempfile::tempdir().unwrap();
        let archive = root.path().join("source.tar.gz");
        let original = b"original notice\r\n";
        let expected = [super::SourceNotice {
            member: "LICENSE",
            bytes: original.len() as u64,
            sha256: "09af4aa36466eb77f823657a1e360a891c1defcc84f0e14918aaa7a3e84624ba",
        }];
        // Bind the expected bytes independently of the extractor.
        assert_eq!(crate::digest::sha256_hex(original), expected[0].sha256);
        for (members, accepted) in [
            (vec![("LICENSE", original.as_slice())], true),
            (vec![("LICENSE", b"modified notice\r\n".as_slice())], false),
            (vec![("other", original.as_slice())], false),
            (
                vec![
                    ("LICENSE", original.as_slice()),
                    ("LICENSE", original.as_slice()),
                ],
                false,
            ),
        ] {
            let gzip = flate2::write::GzEncoder::new(
                std::fs::File::create(&archive).unwrap(),
                flate2::Compression::fast(),
            );
            let mut tar = tar::Builder::new(gzip);
            for (name, bytes) in members {
                let mut header = tar::Header::new_gnu();
                header.set_size(bytes.len() as u64);
                header.set_mode(0o644);
                header.set_cksum();
                tar.append_data(&mut header, name, bytes).unwrap();
            }
            tar.into_inner().unwrap().finish().unwrap();
            super::preflight_source_archive(&archive).unwrap();
            let result = super::source_notice_members(&archive, &expected);
            assert_eq!(result.is_ok(), accepted);
            if let Ok(found) = result {
                assert_eq!(found["LICENSE"], original);
            }
        }
    }

    #[test]
    fn rf_stream_snapshot_preserves_bytes_and_refuses_escape_duplicates_and_excess() {
        use crate::rfdetr_windows_source::SubprocessEvidenceFileEntry;
        let source = tempfile::tempdir().unwrap();
        std::fs::create_dir(source.path().join("logs")).unwrap();
        let stdout = [0, 255, 13, 10];
        std::fs::write(source.path().join("logs/cmake-build.stdout"), stdout).unwrap();
        std::fs::write(source.path().join("logs/cmake-build.stderr"), b"e").unwrap();
        let entry = SubprocessEvidenceFileEntry {
            label: "cmake-build".into(),
            argv: vec!["cmake".into()],
            cwd: "C:/source".into(),
            exit_code: 0,
            stdout_path: "logs/cmake-build.stdout".into(),
            stderr_path: "logs/cmake-build.stderr".into(),
        };
        let metadata = source.path().join("subprocess.json");
        let raw = serde_json::to_vec_pretty(&vec![entry.clone()]).unwrap();
        std::fs::write(&metadata, &raw).unwrap();
        let snapshot = tempfile::tempdir().unwrap();
        let copied =
            super::snapshot_rfdetr_streams_with_limit(snapshot.path(), &metadata, 5).unwrap();
        assert_eq!(std::fs::read(copied).unwrap(), raw);
        assert_eq!(
            std::fs::read(snapshot.path().join("logs/cmake-build.stdout")).unwrap(),
            stdout
        );
        let short = tempfile::tempdir().unwrap();
        assert!(
            super::snapshot_rfdetr_streams_with_limit(short.path(), &metadata, 4)
                .unwrap_err()
                .contains("total byte limit")
        );
        let mut escape = entry.clone();
        escape.stdout_path = "../outside".into();
        for entries in [
            vec![escape],
            vec![entry.clone(), entry.clone()],
            vec![entry; 65],
        ] {
            std::fs::write(&metadata, serde_json::to_vec(&entries).unwrap()).unwrap();
            let rejected = tempfile::tempdir().unwrap();
            assert!(super::snapshot_rfdetr_streams(rejected.path(), &metadata).is_err());
        }
    }

    use super::*;

    fn raw_header(kind: u8, size: u64) -> Vec<u8> {
        let mut header = tar::Header::new_gnu();
        header.set_path("member").unwrap();
        header.set_entry_type(tar::EntryType::new(kind));
        header.set_size(size);
        header.set_mode(0o644);
        header.set_cksum();
        header.as_bytes().to_vec()
    }

    #[test]
    fn archive_extensions_are_checked_before_their_bodies_are_read() {
        for kind in *b"LKxg" {
            // Only a header: an automatic extension read would fail as truncated.
            // The raw pass must instead refuse the declared size before that read.
            let header = raw_header(kind, 1025);
            let error = preflight_tar_stream(header.as_slice(), 8192, 4096, 1024, 8).unwrap_err();
            assert_eq!(
                error, "native source extension exceeds byte limit",
                "{kind}"
            );
        }
    }

    #[test]
    fn compressed_oversized_extensions_fail_at_the_production_boundary() {
        use std::io::Write;
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("source.tar.gz");
        for kind in *b"LKxg" {
            let mut compressed =
                flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
            compressed
                .write_all(&raw_header(kind, 1024 * 1024 + 1))
                .unwrap();
            fs::write(&path, compressed.finish().unwrap()).unwrap();
            assert_eq!(
                preflight_source_archive(&path).unwrap_err(),
                "native source extension exceeds byte limit",
                "{kind}"
            );
        }
    }

    #[test]
    fn archive_budget_includes_headers_padding_and_data_after_end_marker() {
        let mut ordinary = raw_header(b'0', 1);
        ordinary.extend_from_slice(&[0; 512]);
        ordinary.extend_from_slice(&[0; 1024]);
        preflight_tar_stream(ordinary.as_slice(), 2048, 1024, 128, 8).unwrap();
        assert!(preflight_tar_stream(ordinary.as_slice(), 2047, 1024, 128, 8).is_err());

        // The tar iterator stops at the first zero header, but the outer budget
        // must still reject arbitrary decompressed bytes beyond that marker.
        let mut trailing = vec![0; 1024];
        trailing.extend_from_slice(&[1; 1025]);
        assert_eq!(
            preflight_tar_stream(trailing.as_slice(), 2048, 1024, 128, 8).unwrap_err(),
            "native source archive exceeds expanded byte limit"
        );
        let mut headers = raw_header(b'0', 0);
        headers.extend_from_slice(&raw_header(b'0', 0));
        assert!(preflight_tar_stream(headers.as_slice(), 1023, 1024, 128, 8).is_err());
        assert_eq!(
            preflight_tar_stream(headers.as_slice(), 2048, 1024, 128, 1).unwrap_err(),
            "native source archive exceeds entry limit"
        );
    }

    #[test]
    fn archive_decoder_never_delivers_more_than_budget_plus_one() {
        let delivered = std::cell::Cell::new(0usize);
        struct Counting<'a>(&'a std::cell::Cell<usize>);
        impl Read for Counting<'_> {
            fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
                bytes.fill(0);
                self.0.set(self.0.get() + bytes.len());
                Ok(bytes.len())
            }
        }
        assert!(preflight_tar_stream(Counting(&delivered), 2048, 1024, 128, 8).is_err());
        assert_eq!(delivered.get(), 2049);
    }

    #[test]
    fn staging_capture_refuses_substitution_against_original_receipt() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("worker.exe");
        let original = crate::pe_dependencies::tests::image();
        fs::write(&path, &original).unwrap();
        let mut outputs =
            crate::controlled_build::census_outputs(&[("bin/worker.exe", original.as_slice())])
                .unwrap();
        let output = outputs.remove(0);
        let retained = capture_output(&path, &output).unwrap();
        let mut replacement = original.clone();
        *replacement.last_mut().unwrap() ^= 1;
        fs::write(&path, replacement).unwrap();
        assert!(
            capture_output(&path, &output)
                .unwrap_err()
                .contains("original admission")
        );
        assert_eq!(retained, original);
        fs::remove_file(path.clone()).unwrap();
        assert!(capture_output(&path, &output).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn staging_capture_refuses_a_symlink_even_when_bytes_match() {
        let root = tempfile::tempdir().unwrap();
        let bytes = crate::pe_dependencies::tests::image();
        let output =
            crate::controlled_build::census_outputs(&[("bin/worker.exe", bytes.as_slice())])
                .unwrap()
                .remove(0);
        fs::write(root.path().join("real"), bytes).unwrap();
        std::os::unix::fs::symlink(root.path().join("real"), root.path().join("worker.exe"))
            .unwrap();
        assert!(capture_output(&root.path().join("worker.exe"), &output).is_err());
    }

    #[test]
    fn cache_rejects_changed_configuration_and_duplicate_shadowing() {
        let valid = b"onnxruntime_BUILD_SHARED_LIB:BOOL=ON\nonnxruntime_DISABLE_CONTRIB_OPS:BOOL=ON\nonnxruntime_DISABLE_ML_OPS:BOOL=ON\nonnxruntime_USE_TELEMETRY:BOOL=OFF\nCMAKE_MSVC_RUNTIME_LIBRARY:STRING=MultiThreadedDLL\nonnxruntime_REDUCED_OPS_BUILD:BOOL=ON\nCMAKE_GENERATOR_PLATFORM:INTERNAL=x64\n";
        validate_onnx_cache(valid).unwrap();
        let text = std::str::from_utf8(valid).unwrap();
        for (from, to) in [
            ("MultiThreadedDLL", "MultiThreaded"),
            ("x64", "Win32"),
            ("USE_TELEMETRY:BOOL=OFF", "USE_TELEMETRY:BOOL=ON"),
        ] {
            assert!(validate_onnx_cache(text.replace(from, to).as_bytes()).is_err());
        }
        assert!(
            validate_onnx_cache(format!("{text}onnxruntime_BUILD_SHARED_LIB:BOOL=ON\n").as_bytes())
                .unwrap_err()
                .contains("duplicate")
        );
        assert!(validate_onnx_cache(b"not a cache").is_err());
    }

    #[test]
    fn failed_admission_preserves_error_and_removes_its_snapshot() {
        let mut observed = None;
        let failure = with_snapshot::<()>(|root| {
            observed = Some(root.to_owned());
            fs::write(root.join("retained"), b"temporary source input").unwrap();
            Err("original source mismatch".into())
        })
        .unwrap_err();
        assert_eq!(failure, "original source mismatch");
        assert!(!observed.unwrap().exists());
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod llama_capture_tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    #[ignore = "requires the retained native capture and exact source/tool archives"]
    fn llama_original_capture_enters_inventory_without_restamping() {
        let fixture =
            std::env::var("SOLSTONE_LLAMA_ADMISSION_FIXTURE").expect("fixture JSON path required");
        let values: BTreeMap<String, PathBuf> =
            serde_json::from_slice(&fs::read(fixture).unwrap()).unwrap();
        let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(3)
            .unwrap();
        let admit = |receipt: &Path, evidence: &Path| {
            admit_llama(
                repo,
                LlamaInputPaths {
                    receipt,
                    evidence,
                    source_archive: &values["source_archive"],
                    sdk_archive: &values["sdk_archive"],
                    cmake_archive: &values["cmake_archive"],
                    capture_root: &values["capture_root"],
                },
            )
        };
        let input = admit(&values["receipt"], &values["evidence"]).unwrap();
        assert_eq!(input.receipt_bytes(), fs::read(&values["receipt"]).unwrap());
        assert_eq!(
            input.evidence_bytes(),
            fs::read(&values["evidence"]).unwrap()
        );
        for output in &input.receipt().outputs {
            assert_eq!(
                sha256_hex(&input.outputs()[&output.label]),
                output.pre_signing_sha256
            );
        }
        let inventory =
            crate::inventory::load_inventory(&repo.join("core/distribution/inventory.toml"))
                .unwrap();
        let expected: BTreeSet<String> = inventory
            .entry
            .iter()
            .filter_map(|entry| match entry {
                crate::inventory::Entry::WindowsNative {
                    component: crate::inventory::WindowsNativeComponent::Llama,
                    member,
                    ..
                } => Some(member.clone()),
                _ => None,
            })
            .collect();
        let actual: BTreeSet<String> = input
            .outputs
            .keys()
            .cloned()
            .chain(["receipt.json", "build-evidence.json", "validation.log"].map(String::from))
            .collect();
        assert_eq!(actual, expected);
        for name in [
            "notices/cJSON-LICENSE.txt",
            "notices/dirent-LICENSE.txt",
            "notices/stb-LICENSE.txt",
            "notices/miniaudio-LICENSE.txt",
        ] {
            assert!(
                std::str::from_utf8(&input.outputs()[name])
                    .unwrap()
                    .contains("Copyright")
            );
        }
        super::super::windows_stage::AdmittedWindowsNativeInputs::from_controlled(vec![input])
            .unwrap();
        let scratch = tempfile::tempdir().unwrap();
        let evidence = scratch.path().join("evidence.json");
        fs::write(&evidence, b"{}").unwrap();
        assert!(admit(&values["receipt"], &evidence).is_err());
        let receipt = scratch.path().join("receipt.json");
        let mut changed: serde_json::Value =
            serde_json::from_slice(&fs::read(&values["receipt"]).unwrap()).unwrap();
        changed["source"]["product"]["commit"] = serde_json::Value::String("0".repeat(40));
        fs::write(&receipt, serde_json::to_vec(&changed).unwrap()).unwrap();
        assert!(admit(&receipt, &values["evidence"]).is_err());
    }
}

/// The producer's real admission path over the real retained verifier bytes:
/// the pinned SDK source archive, the pinned offline bundle tar, the build's
/// output root, and the SDK report, dumpbin texts and tool population that
/// build captured. No run of the controlled driver exists yet, so the receipt,
/// evidence wrapper, validation log, driver controls and the invoked-version
/// census are synthesized in `synthesize_driver_documents` around those real
/// bytes, as the driver and recorder would write them.
#[cfg(all(test, feature = "full-tests"))]
mod nvattest_retained_tests {
    use super::*;
    use crate::nvattest_windows::test_support::{controls, recorder_receipt};
    use crate::nvattest_windows::*;

    struct Retained {
        source_archive: PathBuf,
        bundle_archive: PathBuf,
        output_root: PathBuf,
        evidence_dir: PathBuf,
    }

    fn retained() -> Retained {
        let var = |name: &str| {
            PathBuf::from(std::env::var(name).unwrap_or_else(|_| panic!("{name} is required")))
        };
        Retained {
            source_archive: var("SOLSTONE_NVATTEST_SOURCE_ARCHIVE"),
            bundle_archive: var("SOLSTONE_NVATTEST_BUNDLE_ARCHIVE"),
            output_root: var("SOLSTONE_NVATTEST_OUTPUT_ROOT"),
            evidence_dir: var("SOLSTONE_NVATTEST_EVIDENCE_DIR"),
        }
    }

    fn repo() -> &'static Path {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(3)
            .unwrap()
    }

    /// The digest of `cmake.exe` inside the bundle's pinned CMake archive.
    fn bundled_cmake_sha256(bundle: &Path) -> String {
        let bytes = fs::read(bundle).unwrap();
        let mut archive = tar::Archive::new(io::Cursor::new(bytes));
        for entry in archive.entries().unwrap() {
            let mut entry = entry.unwrap();
            if entry.path_bytes().as_ref() == b"./cmake-3.31.12-windows-x86_64.zip" {
                let mut zipped = Vec::new();
                entry.read_to_end(&mut zipped).unwrap();
                let mut zip = ::zip::ZipArchive::new(io::Cursor::new(zipped)).unwrap();
                let mut exe = zip
                    .by_name("cmake-3.31.12-windows-x86_64/bin/cmake.exe")
                    .unwrap();
                let mut cmake = Vec::new();
                exe.read_to_end(&mut cmake).unwrap();
                return sha256_hex(&cmake);
            }
        }
        panic!("bundle has no CMake archive");
    }

    /// SYNTHESIZED: the invoked-version census the driver would record, from
    /// the build host's captured tool population (paths, digests, file
    /// versions) and the versions the SDK report recorded by invoking them.
    fn synthesize_census(retained: &Retained, report: &NvattestBuildReport) -> NvattestToolCensus {
        let population: Vec<serde_json::Value> = {
            let raw = fs::read(retained.evidence_dir.join("tool-identities.json")).unwrap();
            serde_json::from_slice(raw.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&raw)).unwrap()
        };
        let find = |suffix: &str| {
            let tool = population
                .iter()
                .find(|t| t["path"].as_str().unwrap().ends_with(suffix))
                .unwrap_or_else(|| panic!("no captured tool {suffix}"));
            (
                tool["path"].as_str().unwrap().to_string(),
                tool["sha256"].as_str().unwrap().to_string(),
                tool["version"].as_str().map(str::to_string),
            )
        };
        let tool = |suffix: &str, version: Option<String>| {
            let (path, sha256, file_version) = find(suffix);
            NvattestToolVersionPath {
                version: version.or(file_version),
                path,
                sha256,
            }
        };
        NvattestToolCensus {
            schema: NVATTEST_TOOL_CENSUS_SCHEMA_V1.into(),
            rustc: tool(r"\bin\rustc.exe", Some(report.tools.rustc.clone())),
            cargo: tool(r"\bin\cargo.exe", Some(report.tools.cargo.clone())),
            cmake: NvattestToolVersionPath {
                version: Some(report.tools.cmake.clone()),
                path: r"C:\work\cmake\cmake-3.31.12-windows-x86_64\bin\cmake.exe".into(),
                sha256: bundled_cmake_sha256(&retained.bundle_archive),
            },
            cl: tool(r"\Hostx64\x64\cl.exe", None),
            link: tool(r"\Hostx64\x64\link.exe", None),
            nmake: tool(r"\Hostx64\x64\nmake.exe", None),
            msbuild: tool(r"\MSBuild\Current\Bin\amd64\MSBuild.exe", None),
            msvc: NvattestMsvcToolsVersion {
                vc_tools_version: Some(report.tools.msvc.clone()),
            },
            windows_sdk: NvattestWindowsSdkVersion {
                version: Some(report.tools.windows_sdk.clone()),
            },
            vs: None,
        }
    }

    /// SYNTHESIZED: receipt, evidence and validation as the recorder writes
    /// them, embedding the real raw report and dumpbin bytes.
    fn synthesize_driver_documents(
        retained: &Retained,
        report_raw: &[u8],
        mutate_receipt: impl FnOnce(&mut crate::controlled_build::ControlledBuildReceipt),
    ) -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
        let pins = production_pins();
        // The census is the driver's own observation, independent of the
        // (possibly forged) report under test: build it from the captured one.
        let captured = fs::read(retained.evidence_dir.join("build-report.json")).unwrap();
        let report = decode_build_report(&captured).unwrap();
        let driver = controls(&pins);
        let dumpbin = |name: &str| {
            base64_encode(
                &fs::read(retained.evidence_dir.join(format!("{name}.dumpbin.txt"))).unwrap(),
            )
        };
        let evidence = NvattestWindowsBuildEvidence {
            schema: NVATTEST_BUILD_EVIDENCE_SCHEMA_V1.into(),
            report_base64: base64_encode(report_raw),
            census: synthesize_census(retained, &report),
            invocation: driver.invocation,
            refusals: driver.refusals,
            network: driver.network,
            dumpbin_dependents: NvattestDumpbinDependents {
                nvattest_exe: dumpbin("nvattest.exe"),
                msvcp140: dumpbin("msvcp140.dll"),
                vcruntime140: dumpbin("vcruntime140.dll"),
                vcruntime140_1: dumpbin("vcruntime140_1.dll"),
            },
        };
        let evidence = serde_json::to_vec_pretty(&evidence).unwrap();
        let validation =
            b"schema=solstone.nvattest-windows-validation.v1\nsynthesized=true\n".to_vec();
        let exe = fs::read(retained.output_root.join("bin/nvattest.exe")).unwrap();
        let mut receipt = recorder_receipt(&pins, &exe, &evidence, &validation);
        mutate_receipt(&mut receipt);
        let dir = tempfile::tempdir().unwrap();
        let paths = (
            dir.path().join("receipt.json"),
            dir.path().join("evidence.json"),
            dir.path().join("validation.log"),
        );
        fs::write(
            &paths.0,
            crate::controlled_build::encode_controlled_build_receipt(&receipt).unwrap(),
        )
        .unwrap();
        fs::write(&paths.1, evidence).unwrap();
        fs::write(&paths.2, validation).unwrap();
        (dir, paths.0, paths.1, paths.2)
    }

    fn admit_retained(
        retained: &Retained,
        report_raw: &[u8],
        mutate_receipt: impl FnOnce(&mut crate::controlled_build::ControlledBuildReceipt),
    ) -> Result<AdmittedControlledInput, String> {
        let (_dir, receipt, evidence, validation) =
            synthesize_driver_documents(retained, report_raw, mutate_receipt);
        let runtime: Vec<Vec<u8>> = NVATTEST_RUNTIME_OUTPUT_LABELS
            .iter()
            .map(|label| fs::read(retained.output_root.join(label)).unwrap())
            .collect();
        admit_nvattest(
            repo(),
            NvattestInputPaths {
                receipt: &receipt,
                evidence: &evidence,
                validation: &validation,
                source_archive: &retained.source_archive,
                bundle_archive: &retained.bundle_archive,
                output_root: &retained.output_root,
            },
            MsvcRuntimeBytes {
                msvcp140: &runtime[0],
                vcruntime140: &runtime[1],
                vcruntime140_1: &runtime[2],
            },
        )
    }

    fn forged_report(report_raw: &[u8], forge: impl FnOnce(&mut serde_json::Value)) -> Vec<u8> {
        let json = report_raw.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap();
        let mut value: serde_json::Value = serde_json::from_slice(json).unwrap();
        forge(&mut value);
        let mut forged = vec![0xEF, 0xBB, 0xBF];
        forged.extend(serde_json::to_vec_pretty(&value).unwrap());
        forged
    }

    #[test]
    #[ignore = "requires the retained verifier archives, build outputs and captured evidence"]
    fn nvattest_retained_real_bytes_admit_through_production_and_forgeries_refuse() {
        let retained = retained();
        let report_raw = fs::read(retained.evidence_dir.join("build-report.json")).unwrap();
        assert!(report_raw.starts_with(&[0xEF, 0xBB, 0xBF]));

        let input = admit_retained(&retained, &report_raw, |_| {}).unwrap();
        let exe = &input.outputs()[NVATTEST_EXE_OUTPUT_LABEL];
        assert_eq!(
            sha256_hex(exe),
            "220849fea69d60563fc6ef0ea7c020450d842565d08e7cbd2eecfecbd41ecdc8"
        );
        // Imports parsed from the real executable bytes match the real report.
        let pe = crate::pe_dependencies::inspect_dependencies(exe).unwrap();
        let report = decode_build_report(&report_raw).unwrap();
        let reported: std::collections::BTreeSet<String> = report
            .imports
            .iter()
            .map(|i| i.to_ascii_lowercase())
            .collect();
        let parsed: std::collections::BTreeSet<String> = pe
            .imports
            .iter()
            .chain(&pe.delay_imports)
            .cloned()
            .collect();
        assert_eq!(parsed, reported);
        assert!(pe.forwarders.is_empty());
        let ca = &input.outputs()[NVATTEST_CA_BUNDLE_LABEL];
        assert_eq!(
            sha256_hex(ca),
            "3ff344e30b9b1ed2971044eabb438a08f2e2245ddb5f8ab1a3ad8b63ab4eaf91"
        );
        assert_eq!(
            sha256_hex(&input.outputs()[NVATTEST_LICENSE_LABEL]),
            "82d36972a71088e8d4a4793313e64e18340c60de08e3175a58360a277c962a33"
        );
        crate::produce::windows_stage::AdmittedWindowsNativeInputs::from_controlled(vec![input])
            .unwrap();

        type Forgery = (
            &'static str,
            Vec<u8>,
            fn(&mut crate::controlled_build::ControlledBuildReceipt),
        );
        let refusals: Vec<Forgery> = vec![
            (
                "report-sources",
                forged_report(&report_raw, |r| {
                    r["sources"].as_array_mut().unwrap().remove(4);
                }),
                |_| {},
            ),
            (
                "regorus-build-revision",
                forged_report(&report_raw, |r| {
                    r["sources"]
                        .as_array_mut()
                        .unwrap()
                        .retain(|s| s["name"] != "regorus-build-revision");
                }),
                |_| {},
            ),
            (
                "report-tools",
                forged_report(&report_raw, |r| r["build_tools"] = serde_json::json!([])),
                |_| {},
            ),
            (
                "tool-census",
                forged_report(&report_raw, |r| {
                    r["tools"]["rustc"] = serde_json::json!("rustc 1.97.1 (8bab26f4f 2026-07-15)");
                }),
                |_| {},
            ),
            ("receipt-inputs", report_raw.clone(), |r| r.inputs.clear()),
            ("receipt-configuration", report_raw.clone(), |r| {
                r.configuration.target_triple = "x86_64-pc-windows-gnu".into();
            }),
        ];
        for (boundary, report, mutate) in refusals {
            match admit_retained(&retained, &report, mutate) {
                Ok(_) => panic!("{boundary}: forged input admitted"),
                Err(error) => {
                    assert!(error.starts_with(boundary), "{boundary}: {error}");
                    println!("refused as expected: {error}");
                }
            }
        }
    }
}
