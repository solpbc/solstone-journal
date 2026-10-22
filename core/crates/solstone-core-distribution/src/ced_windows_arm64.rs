// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! ARM CED receipt profile. The historical x64 profile remains separate.

use std::collections::{BTreeMap, BTreeSet};

use crate::ced_windows::{self, CedWindowsBuildEvidence};
use crate::controlled_build::{
    BuildConfiguration, BuilderIdentity, ControlledBuildReceiptDraft, InputIdentityEntry,
    SourceIdentity, SupportingArtifactRef,
};
use crate::digest::sha256_hex;
use crate::produce::windows_openmp::AdmittedOpenMp;
use crate::windows_clang::{self, ToolCensus, parse_cache, windows_path};
use solstone_core_installed_payload::WindowsArchitecture;

pub use crate::windows_clang::BuildValidation as CedWindowsArm64Validation;

pub const EVIDENCE_SCHEMA: &str = "solstone.ced-windows-arm64-build-evidence.v1";
pub const EVIDENCE_LABEL: &str = "provenance/windows-aarch64/ced-build-evidence.json";
pub const VALIDATION_SCHEMA: &str = "solstone.ced-windows-arm64-validation.v1";

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
    for flag in ced_windows::CED_WINDOWS_BUILD_FLAGS {
        let (key, value) = flag
            .strip_prefix("-D")
            .expect("CED flag")
            .split_once('=')
            .expect("CED value");
        configuration.insert(key.into(), value.into());
    }
    for (key, value) in [
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
        ["build-target=ced", "build-mode=serial-nmake"]
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

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CedWindowsArm64Evidence {
    pub schema: String,
    pub ced: CedWindowsBuildEvidence,
    pub host_inputs: Vec<InputIdentityEntry>,
    pub openmp: Vec<InputIdentityEntry>,
    pub archive_tools: Vec<InputIdentityEntry>,
    pub system_tools: Vec<InputIdentityEntry>,
    pub network_denied_tools: Vec<String>,
    pub configuration: BTreeMap<String, String>,
    pub build_log_sha256: String,
}

impl CedWindowsArm64Evidence {
    fn tool_census(&self) -> ToolCensus<'_> {
        ToolCensus {
            archive_tools: &self.archive_tools,
            system_tools: &self.system_tools,
            network_denied_tools: &self.network_denied_tools,
        }
    }

    pub fn validate(&self, source: &SourceIdentity, openmp: &AdmittedOpenMp) -> Result<(), String> {
        self.ced
            .validate_against_source(source)
            .map_err(|e| e.to_string())?;
        let host = pinned_host_inputs();
        let cmake = &host[1];
        if self.schema != EVIDENCE_SCHEMA
            || self.host_inputs != host
            || self.configuration != cache_configuration()
            || self.openmp != openmp.build_inputs()
            || self.ced.cmake_archive.size != cmake.size
            || self.ced.cmake_archive.sha256 != cmake.sha256
            || self.build_log_sha256.len() != 64
            || !self
                .build_log_sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err("CED ARM source, host pins, OpenMP cohort or configuration differ".into());
        }
        windows_clang::validate_tool_census(&self.tool_census())
    }

    pub fn validate_capture(&self, validation: &CedWindowsArm64Validation) -> Result<(), String> {
        if validation.schema != VALIDATION_SCHEMA
            || self.ced.cmake_cache_sha256 != sha256_hex(validation.cache.as_bytes())
            || self.build_log_sha256 != sha256_hex(validation.build_log.as_bytes())
            || !validation.install_manifest.is_empty()
            || !validation
                .build_log
                .lines()
                .any(|line| line == "CED_ARM64_WINDOWS_BUILD_EXIT=0")
            || validation
                .python_path_configuration
                .lines()
                .collect::<Vec<_>>()
                != ["python312.zip", "."]
        {
            return Err("CED ARM validation does not bind its original successful build".into());
        }
        let cache = parse_cache(&validation.cache)?;
        for (key, value) in cache_configuration() {
            if cache.get(key.as_str()).copied() != Some(value.as_str()) {
                return Err(format!("CED ARM cache differs at {key}"));
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
            if windows_path(cache.get(key).ok_or("CED ARM cache lacks an input path")?)?
                != format!("{work}{suffix}")
            {
                return Err(format!("CED ARM input path differs at {key}"));
            }
        }
        windows_clang::validate_host_capture(&self.tool_census(), validation)
    }
}

pub fn assemble_receipt_draft(
    source: SourceIdentity,
    evidence: &CedWindowsArm64Evidence,
    builder: BuilderIdentity,
    output: &[u8],
    openmp: &AdmittedOpenMp,
) -> Result<ControlledBuildReceiptDraft, String> {
    evidence.validate(&source, openmp)?;
    let dependencies =
        crate::pe_dependencies::inspect_dependencies(output, WindowsArchitecture::Aarch64)?;
    let pe = crate::pe::parse_pe(output).map_err(|e| e.to_string())?;
    ced_windows::verify_exports(&pe)
        .and_then(|()| ced_windows::verify_cpu_only_imports(&pe))
        .map_err(|e| e.to_string())?;
    let imports: BTreeSet<_> = dependencies
        .imports
        .iter()
        .map(|s| s.to_ascii_lowercase())
        .collect();
    if !dependencies.is_dll
        || !dependencies.delay_imports.is_empty()
        || !dependencies.forwarders.is_empty()
        || !imports.contains("libomp.dll")
        || imports.contains("vcomp140.dll")
        || imports.contains("vcruntime140_1.dll")
    {
        return Err(
            "CED ARM requires its shared native OpenMP runtime and native CRT closure".into(),
        );
    }
    let mut inputs = vec![evidence.ced.source_archive.clone()];
    inputs.extend(pinned_host_inputs());
    inputs.extend(openmp.build_inputs());
    let evidence_bytes = serde_json::to_vec(evidence).map_err(|e| e.to_string())?;
    Ok(ControlledBuildReceiptDraft {
        source: Some(source),
        inputs: Some(inputs),
        builder: Some(builder),
        configuration: Some(build_configuration()),
        outputs: Some(
            crate::controlled_build::census_outputs(&[(ced_windows::CED_DLL_OUTPUT_LABEL, output)])
                .map_err(|e| e.to_string())?,
        ),
        supporting: Some(vec![SupportingArtifactRef {
            label: EVIDENCE_LABEL.into(),
            sha256: sha256_hex(&evidence_bytes),
        }]),
        ..ControlledBuildReceiptDraft::default()
    })
}
