// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! ARM RFDETR receipt profile. The historical x64 profile remains separate.

use std::collections::{BTreeMap, BTreeSet};

use crate::controlled_build::{
    BuildConfiguration, BuilderIdentity, ControlledBuildReceiptDraft, DependencySource,
    InputIdentityEntry, SourceIdentity, SupportingArtifactRef,
};
use crate::digest::sha256_hex;
use crate::produce::windows_openmp::AdmittedOpenMp;
use crate::rfdetr_windows::{
    self, GGML_COMMIT, GGML_REPOSITORY, RF_DETR_COMMIT, RF_DETR_REPOSITORY,
};
use crate::windows_clang::{self, ToolCensus, parse_cache, windows_path};
use solstone_core_installed_payload::WindowsArchitecture;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RfdetrWindowsArm64Validation {
    pub host: crate::windows_clang::BuildValidation,
    pub compile_options: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RfdetrSourceEvidence {
    pub rf_bundle: InputIdentityEntry,
    pub ggml_bundle: InputIdentityEntry,
    pub ggml: DependencySource,
    pub cmake_archive: InputIdentityEntry,
    pub cmake_cache_sha256: String,
}

pub const EVIDENCE_SCHEMA: &str = "solstone.rfdetr-windows-arm64-build-evidence.v1";
pub const EVIDENCE_LABEL: &str = "provenance/windows-aarch64/rfdetr-build-evidence.json";
pub const VALIDATION_SCHEMA: &str = "solstone.rfdetr-windows-arm64-validation.v1";

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
    for flag in rfdetr_windows::RFDETR_WINDOWS_BUILD_FLAGS {
        let (key, value) = flag
            .strip_prefix("-D")
            .expect("RFDETR flag")
            .split_once('=')
            .expect("RFDETR value");
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
        ["build-target=rfdetr-cli", "build-mode=serial-nmake"]
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
pub struct RfdetrWindowsArm64Evidence {
    pub schema: String,
    pub rfdetr: RfdetrSourceEvidence,
    pub compile_options_sha256: String,
    pub host_inputs: Vec<InputIdentityEntry>,
    pub openmp: Vec<InputIdentityEntry>,
    pub archive_tools: Vec<InputIdentityEntry>,
    pub system_tools: Vec<InputIdentityEntry>,
    pub network_denied_tools: Vec<String>,
    pub configuration: BTreeMap<String, String>,
    pub build_log_sha256: String,
}

impl RfdetrWindowsArm64Evidence {
    fn tool_census(&self) -> ToolCensus<'_> {
        ToolCensus {
            archive_tools: &self.archive_tools,
            system_tools: &self.system_tools,
            network_denied_tools: &self.network_denied_tools,
        }
    }

    pub fn validate(&self, source: &SourceIdentity, openmp: &AdmittedOpenMp) -> Result<(), String> {
        if source.windows_dependency.repository != RF_DETR_REPOSITORY
            || source.windows_dependency.revision != RF_DETR_COMMIT
            || source.windows_dependency.content_sha256 != self.rfdetr.rf_bundle.sha256
            || self.rfdetr.rf_bundle.label != rfdetr_windows::RFDETR_WINDOWS_RF_BUNDLE_LABEL
            || self.rfdetr.ggml_bundle.label != rfdetr_windows::RFDETR_WINDOWS_GGML_BUNDLE_LABEL
            || self.rfdetr.ggml
                != (DependencySource {
                    repository: GGML_REPOSITORY.into(),
                    revision: GGML_COMMIT.into(),
                    content_sha256: self.rfdetr.ggml_bundle.sha256.clone(),
                })
        {
            return Err("RFDETR ARM source bundle identities differ".into());
        }
        let host = pinned_host_inputs();
        let cmake = &host[1];
        if self.schema != EVIDENCE_SCHEMA
            || self.host_inputs != host
            || self.configuration != cache_configuration()
            || self.openmp != openmp.build_inputs()
            || self.rfdetr.cmake_archive.size != cmake.size
            || self.rfdetr.cmake_archive.sha256 != cmake.sha256
            || self.build_log_sha256.len() != 64
            || !self
                .build_log_sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(
                "RFDETR ARM source, host pins, OpenMP cohort or configuration differ".into(),
            );
        }
        windows_clang::validate_tool_census(&self.tool_census())
    }

    pub fn validate_capture(
        &self,
        validation: &RfdetrWindowsArm64Validation,
    ) -> Result<(), String> {
        validate_compile_options(&validation.compile_options)?;
        if self.compile_options_sha256
            != sha256_hex(
                &serde_json::to_vec(&validation.compile_options).map_err(|e| e.to_string())?,
            )
        {
            return Err("RFDETR ARM compile-option evidence differs".into());
        }
        let validation = &validation.host;
        if validation.schema != VALIDATION_SCHEMA
            || self.rfdetr.cmake_cache_sha256 != sha256_hex(validation.cache.as_bytes())
            || self.build_log_sha256 != sha256_hex(validation.build_log.as_bytes())
            || !validation.install_manifest.is_empty()
            || !validation
                .build_log
                .lines()
                .any(|line| line == "RFDETR_ARM64_WINDOWS_BUILD_EXIT=0")
            || validation
                .python_path_configuration
                .lines()
                .collect::<Vec<_>>()
                != ["python312.zip", "."]
        {
            return Err("RFDETR ARM validation does not bind its original successful build".into());
        }
        let cache = parse_cache(&validation.cache)?;
        for (key, value) in cache_configuration() {
            if cache.get(key.as_str()).copied() != Some(value.as_str()) {
                return Err(format!("RFDETR ARM cache differs at {key}"));
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
                    .ok_or("RFDETR ARM cache lacks an input path")?,
            )? != format!("{work}{suffix}")
            {
                return Err(format!("RFDETR ARM input path differs at {key}"));
            }
        }
        windows_clang::validate_host_capture(&self.tool_census(), validation)
    }
}

pub fn assemble_receipt_draft(
    source: SourceIdentity,
    evidence: &RfdetrWindowsArm64Evidence,
    builder: BuilderIdentity,
    output: &[u8],
    openmp: &AdmittedOpenMp,
) -> Result<ControlledBuildReceiptDraft, String> {
    evidence.validate(&source, openmp)?;
    let dependencies =
        crate::pe_dependencies::inspect_dependencies(output, WindowsArchitecture::Aarch64)?;
    let pe = crate::pe::parse_pe(output).map_err(|e| e.to_string())?;
    rfdetr_windows::verify_cpu_only_imports(&pe).map_err(|e| e.to_string())?;
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
            "RFDETR ARM requires its shared native OpenMP runtime and native CRT closure".into(),
        );
    }
    let mut inputs = vec![
        evidence.rfdetr.rf_bundle.clone(),
        evidence.rfdetr.ggml_bundle.clone(),
    ];
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
                rfdetr_windows::RFDETR_CLI_OUTPUT_LABEL,
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

fn validate_compile_options(options: &BTreeMap<String, String>) -> Result<(), String> {
    if options.keys().map(String::as_str).collect::<Vec<_>>()
        != ["ggml-cpu", "rfdetr", "rfdetr-cli"]
    {
        return Err("RFDETR ARM compile-option target population differs".into());
    }
    for (target, text) in options {
        let mut found_cpp = false;
        let mut tinyblas_defined = false;
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            if key.trim() == "CXX_DEFINES" {
                tinyblas_defined |= value
                    .split_whitespace()
                    .any(|flag| matches!(flag, "-DGGML_USE_LLAMAFILE" | "/DGGML_USE_LLAMAFILE"));
            }
            if !matches!(key.trim(), "C_FLAGS" | "CXX_FLAGS") {
                continue;
            }
            let flags: Vec<_> = value
                .split_whitespace()
                .map(str::to_ascii_lowercase)
                .collect();
            if key.trim() == "CXX_FLAGS" {
                found_cpp = true;
            }
            if !flags
                .iter()
                .any(|flag| matches!(flag.as_str(), "/md" | "-md"))
                || flags.iter().any(|flag| {
                    matches!(
                        flag.as_str(),
                        "/mt"
                            | "-mt"
                            | "/mtd"
                            | "-mtd"
                            | "/mdd"
                            | "-mdd"
                            | "-march=native"
                            | "-mcpu=native"
                    ) || flag.starts_with("/arch:")
                        || flag.starts_with("-mavx")
                })
            {
                return Err(format!(
                    "RFDETR ARM {target} flags differ from its portable MD profile"
                ));
            }
        }
        if !found_cpp || (target == "ggml-cpu" && !tinyblas_defined) {
            return Err(format!(
                "RFDETR ARM {target} compile definitions or flags missing"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn options() -> BTreeMap<String, String> {
        ["ggml-cpu", "rfdetr", "rfdetr-cli"].into_iter().map(|name| (name.into(),
            "CXX_DEFINES = -DGGML_USE_LLAMAFILE\nC_FLAGS = /MD /O2 /DNDEBUG\nCXX_FLAGS = /MD /O2 /DNDEBUG\n".into())).collect()
    }
    #[test]
    fn native_cpu_recipe_keeps_tinyblas_and_shared_openmp() {
        let config = cache_configuration();
        for (key, expected) in [
            ("GGML_LLAMAFILE", "ON"),
            ("GGML_NATIVE", "OFF"),
            ("GGML_OPENMP", "ON"),
            ("CMAKE_MSVC_RUNTIME_LIBRARY", "MultiThreadedDLL"),
            ("CMAKE_CXX_COMPILER_TARGET", "aarch64-pc-windows-msvc"),
        ] {
            assert_eq!(config[key], expected);
        }
        assert_eq!(
            build_configuration().target_triple,
            "aarch64-pc-windows-msvc"
        );
        assert!(build_configuration().network_access_denied);
        assert!(validate_compile_options(&options()).is_ok());
        let nmake = options()
            .into_iter()
            .map(|(k, v)| (k, v.replace("/MD", "-MD")))
            .collect();
        assert!(validate_compile_options(&nmake).is_ok());
    }
    #[test]
    fn actual_target_flags_refuse_static_crt_native_or_x86_optimizations() {
        for target in ["rfdetr-cli", "rfdetr", "ggml-cpu"] {
            for foreign in [
                "/MT",
                "-MT",
                "-MDd",
                "/MDd",
                "/MD /arch:AVX2",
                "/MD -mavx2",
                "/MD -march=native",
            ] {
                let mut input = options();
                input.insert(target.into(), input[target].replace("/MD", foreign));
                assert!(
                    validate_compile_options(&input).is_err(),
                    "{target}: {foreign}"
                );
            }
        }
        for fake in [
            "# GGML_USE_LLAMAFILE",
            "-DNO_GGML_USE_LLAMAFILE",
            "-DGGML_USE_LLAMAFILE=0",
        ] {
            let mut input = options();
            let flags = input["ggml-cpu"].replace("-DGGML_USE_LLAMAFILE", fake);
            input.insert("ggml-cpu".into(), flags);
            assert!(validate_compile_options(&input).is_err(), "{fake}");
        }
        let mut input = options();
        input.remove("rfdetr");
        assert!(validate_compile_options(&input).is_err());
        let mut input = options();
        input.get_mut("ggml-cpu").unwrap().clear();
        assert!(validate_compile_options(&input).is_err());
    }
}
