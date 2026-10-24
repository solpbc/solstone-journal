// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! ARM Parakeet receipt profile with the original patched server and model.

use std::collections::{BTreeMap, BTreeSet};

use crate::controlled_build::{
    BuildConfiguration, BuilderIdentity, ControlledBuildReceiptDraft, InputIdentityEntry,
    SourceIdentity, SupportingArtifactRef,
};
use crate::digest::sha256_hex;
use crate::parakeet_windows as facts;
use crate::parakeet_windows_source::{self as source, ParakeetWindowsBuildEvidence};
use crate::produce::windows_openmp::AdmittedOpenMp;
use crate::windows_clang::{self, ToolCensus, parse_cache, windows_path};
use solstone_core_installed_payload::WindowsArchitecture;

pub use crate::windows_clang::BuildValidation as ParakeetWindowsArm64Validation;

pub const EVIDENCE_SCHEMA: &str = "solstone.parakeet-windows-arm64-build-evidence.v1";
pub const EVIDENCE_LABEL: &str = "provenance/windows-aarch64/parakeet-build-evidence.json";
pub const VALIDATION_SCHEMA: &str = "solstone.parakeet-windows-arm64-validation.v1";

pub fn cache_configuration() -> BTreeMap<String, String> {
    let mut configuration = crate::openmp_windows::cache_configuration();
    for name in [
        "LIBOMP_ARCH",
        "LIBOMP_LIB_TYPE",
        "LIBOMP_ENABLE_SHARED",
        "LIBOMP_ENABLE_ASSERTIONS",
        "OPENMP_ENABLE_LIBOMPTARGET",
        "OPENMP_ENABLE_OMPT_TOOLS",
        "LIBOMP_FORTRAN_MODULES",
    ] {
        configuration.remove(name);
    }
    configuration.insert(
        "CMAKE_MSVC_RUNTIME_LIBRARY".into(),
        "MultiThreadedDLL".into(),
    );
    for flag in source::parakeet_controlled_build_configuration().flags {
        let (key, value) = flag
            .strip_prefix("-D")
            .expect("Parakeet flag")
            .split_once('=')
            .expect("Parakeet value");
        configuration.insert(key.into(), value.into());
    }
    for (key, value) in [
        ("GGML_OPENMP", "ON"),
        ("GGML_BACKEND_DL", "OFF"),
        ("GGML_OPENCL", "OFF"),
        ("BASH_EXECUTABLE", "FALSE"),
        ("CMAKE_REQUIRE_FIND_PACKAGE_OpenMP", "ON"),
        ("OpenMP_C_FLAGS", "/openmp"),
        ("OpenMP_CXX_FLAGS", "/openmp"),
        ("OpenMP_C_LIB_NAMES", "libomp"),
        ("OpenMP_CXX_LIB_NAMES", "libomp"),
    ] {
        configuration.insert(key.into(), value.into());
    }
    configuration
}

pub fn build_configuration() -> BuildConfiguration {
    let mut flags: Vec<_> = cache_configuration()
        .into_iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    flags.extend(
        ["build-target=parakeet-server", "build-mode=serial-nmake"]
            .into_iter()
            .map(str::to_owned),
    );
    BuildConfiguration {
        target_triple: WindowsArchitecture::Aarch64.rust_triple().into(),
        profile: "Release".into(),
        flags,
        network_access_denied: true,
    }
}

pub fn pinned_host_inputs() -> Vec<InputIdentityEntry> {
    crate::openmp_windows::pinned_inputs()
        .into_iter()
        .skip(1)
        .collect()
}

pub fn model_identity() -> InputIdentityEntry {
    InputIdentityEntry {
        label: facts::PARAKEET_MODEL_OUTPUT_LABEL.into(),
        size: facts::PARAKEET_MODEL_SIZE_BYTES,
        sha256: facts::PARAKEET_MODEL_SHA256.into(),
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParakeetWindowsArm64Evidence {
    pub schema: String,
    pub parakeet: ParakeetWindowsBuildEvidence,
    pub host_inputs: Vec<InputIdentityEntry>,
    pub openmp: Vec<InputIdentityEntry>,
    pub archive_tools: Vec<InputIdentityEntry>,
    pub system_tools: Vec<InputIdentityEntry>,
    pub network_denied_tools: Vec<String>,
    pub configuration: BTreeMap<String, String>,
    pub build_log_sha256: String,
}

impl ParakeetWindowsArm64Evidence {
    fn tool_census(&self) -> ToolCensus<'_> {
        ToolCensus {
            archive_tools: &self.archive_tools,
            system_tools: &self.system_tools,
            network_denied_tools: &self.network_denied_tools,
        }
    }

    pub fn validate(&self, source: &SourceIdentity, openmp: &AdmittedOpenMp) -> Result<(), String> {
        source::validate_build_evidence(&self.parakeet).map_err(|e| e.to_string())?;
        if source.windows_dependency.repository != "https://github.com/mudler/parakeet.cpp.git"
            || source.windows_dependency.revision != facts::PARAKEET_CPP_COMMIT
            || source.windows_dependency.content_sha256 != self.parakeet.source_archive.sha256
            || self.parakeet.model != model_identity()
        {
            return Err("Parakeet ARM source or fixed model identity differs".into());
        }
        let host = pinned_host_inputs();
        let cmake = &host[1];
        if self.schema != EVIDENCE_SCHEMA
            || self.host_inputs != host
            || self.configuration != cache_configuration()
            || self.openmp != openmp.build_inputs()
            || self.parakeet.cmake_archive.size != cmake.size
            || self.parakeet.cmake_archive.sha256 != cmake.sha256
            || self.build_log_sha256.len() != 64
            || !self
                .build_log_sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(
                "Parakeet ARM source, host pins, OpenMP cohort or configuration differ".into(),
            );
        }
        windows_clang::validate_tool_census(&self.tool_census())
    }

    pub fn validate_capture(
        &self,
        validation: &ParakeetWindowsArm64Validation,
    ) -> Result<(), String> {
        if validation.schema != VALIDATION_SCHEMA
            || self.parakeet.cmake_cache_sha256 != sha256_hex(validation.cache.as_bytes())
            || self.build_log_sha256 != sha256_hex(validation.build_log.as_bytes())
            || !validation.install_manifest.is_empty()
            || !validation
                .build_log
                .lines()
                .any(|line| line == "PARAKEET_ARM64_WINDOWS_BUILD_EXIT=0")
            || validation
                .python_path_configuration
                .lines()
                .collect::<Vec<_>>()
                != ["python312.zip", "."]
        {
            return Err(
                "Parakeet ARM validation does not bind its original successful build".into(),
            );
        }
        let cache = parse_cache(&validation.cache)?;
        for (key, value) in cache_configuration() {
            if cache.get(key.as_str()).copied() != Some(value.as_str()) {
                return Err(format!("Parakeet ARM cache differs at {key}"));
            }
        }
        let work = windows_path(&validation.work_root)?;
        for (key, suffix) in [
            ("CMAKE_HOME_DIRECTORY", "/source"),
            ("CMAKE_INSTALL_PREFIX", "/install"),
            ("OpenMP_libomp_LIBRARY", "/openmp/lib/libomp.lib"),
            ("OpenMP_C_INCLUDE_DIR", "/openmp/include"),
            ("OpenMP_CXX_INCLUDE_DIR", "/openmp/include"),
        ] {
            if windows_path(
                cache
                    .get(key)
                    .ok_or("Parakeet ARM cache lacks an input path")?,
            )? != format!("{work}{suffix}")
            {
                return Err(format!("Parakeet ARM input path differs at {key}"));
            }
        }
        windows_clang::validate_host_capture(&self.tool_census(), validation)
    }
}

pub fn assemble_receipt_draft(
    source: SourceIdentity,
    evidence: &ParakeetWindowsArm64Evidence,
    builder: BuilderIdentity,
    output: &[u8],
    openmp: &AdmittedOpenMp,
) -> Result<ControlledBuildReceiptDraft, String> {
    evidence.validate(&source, openmp)?;
    let dependencies =
        crate::pe_dependencies::inspect_dependencies(output, WindowsArchitecture::Aarch64)?;
    let pe = crate::pe::parse_pe(output).map_err(|e| e.to_string())?;
    let mut allowlist: Vec<_> = facts::PARAKEET_WINDOWS_IMPORT_ALLOWLIST
        .iter()
        .copied()
        .filter(|name| {
            !name.eq_ignore_ascii_case("vcomp140.dll")
                && !name.eq_ignore_ascii_case("vcruntime140_1.dll")
        })
        .collect();
    allowlist.push("libomp.dll");
    if !crate::import_policy::disallowed_imports(&pe.imports, &allowlist).is_empty() {
        return Err(
            "Parakeet ARM import closure differs from its native CPU/OpenMP profile".into(),
        );
    }
    let imports: BTreeSet<_> = dependencies
        .imports
        .iter()
        .map(|s| s.to_ascii_lowercase())
        .collect();
    if dependencies.is_dll
        || !dependencies.delay_imports.is_empty()
        || !dependencies.forwarders.is_empty()
        || !imports.contains("libomp.dll")
        || imports.contains("vcomp140.dll")
        || imports.contains("vcruntime140_1.dll")
    {
        return Err(
            "Parakeet ARM requires its shared native OpenMP runtime and native CRT closure".into(),
        );
    }
    let mut inputs = vec![evidence.parakeet.source_archive.clone(), model_identity()];
    inputs.extend(pinned_host_inputs());
    inputs.extend(openmp.build_inputs());
    let evidence_bytes = serde_json::to_vec(evidence).map_err(|e| e.to_string())?;
    Ok(ControlledBuildReceiptDraft {
        source: Some(source),
        inputs: Some(inputs),
        builder: Some(builder),
        configuration: Some(build_configuration()),
        outputs: Some(
            crate::controlled_build::census_outputs(&[(
                facts::PARAKEET_SERVER_OUTPUT_LABEL,
                output,
            )])
            .map_err(|e| e.to_string())?,
        ),
        supporting: Some(vec![SupportingArtifactRef {
            label: EVIDENCE_LABEL.into(),
            sha256: sha256_hex(&evidence_bytes),
        }]),
        ..ControlledBuildReceiptDraft::default()
    })
}
