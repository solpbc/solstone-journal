// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Original native capture admission for the ARM CPU/OpenMP/Vulkan recipe.

use std::collections::{BTreeMap, BTreeSet};

use crate::controlled_build::{BuildConfiguration, InputIdentityEntry};
use crate::digest::sha256_hex;
use crate::llama_windows_arm64::{self as recipe, CacheRole};
use crate::produce::windows_openmp::AdmittedOpenMp;
use crate::windows_clang::{self, NetworkDeny, ToolCensus, windows_path};

pub const VALIDATION_SCHEMA: &str = "solstone.llama-windows-arm64-validation.v1";
pub const EVIDENCE_SCHEMA: &str = "solstone.llama-windows-arm64-evidence.v1";
pub const EVIDENCE_LABEL: &str = "provenance/windows-aarch64/llama-build-evidence.json";
pub const SHADER_CACHE: &str = "engine-build/ggml/src/ggml-vulkan/vulkan-shaders-gen-prefix/src/vulkan-shaders-gen-build/CMakeCache.txt";
pub const BUILD_STEPS: [&str; 5] = [
    "loader-configure",
    "loader-build",
    "offset-replay",
    "engine-configure",
    "engine-build",
];

pub struct DriverSourceIdentity {
    pub product: crate::provenance::Provenance,
    pub driver_sha256: String,
    pub helper_sha256: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeCommand {
    pub label: String,
    pub program_label: String,
    pub arguments: Vec<String>,
    pub cwd: String,
    pub exit_code: i32,
    pub log_sha256: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeValidation {
    pub schema: String,
    pub product: crate::provenance::Provenance,
    pub driver_sha256: String,
    pub helper_sha256: String,
    pub recorder_sha256: String,
    pub work_root: String,
    pub archive_tools: Vec<InputIdentityEntry>,
    pub system_tools: Vec<InputIdentityEntry>,
    pub network_denied_tools: Vec<String>,
    pub tool_paths: BTreeMap<String, String>,
    pub network_denies: Vec<NetworkDeny>,
    pub tools_before: Vec<InputIdentityEntry>,
    pub source_before: BTreeMap<String, (u64, String)>,
    pub source_patched: BTreeMap<String, (u64, String)>,
    pub source_after: BTreeMap<String, (u64, String)>,
    pub caches: BTreeMap<String, String>,
    pub generated_configuration: BTreeMap<String, String>,
    pub openmp_development: Vec<InputIdentityEntry>,
    pub python_path_configuration: String,
    pub sdk_state_before: String,
    pub sdk_state_after: String,
    pub network_positive_error: i32,
    pub network_blocked_before_error: i32,
    pub network_blocked_after_error: i32,
    pub commands: Vec<NativeCommand>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildEvidence {
    pub schema: String,
    pub product: crate::provenance::Provenance,
    pub driver_sha256: String,
    pub helper_sha256: String,
    pub source_archive: InputIdentityEntry,
    pub host_inputs: Vec<InputIdentityEntry>,
    pub openmp_inputs: Vec<InputIdentityEntry>,
    pub configuration: BTreeMap<String, BTreeMap<String, String>>,
    pub capture_members: Vec<InputIdentityEntry>,
    pub artifact_facts: Vec<InputIdentityEntry>,
}

fn validate_driver_identity(
    validation: &NativeValidation,
    driver: &DriverSourceIdentity,
) -> Result<(), String> {
    if validation.product != driver.product
        || validation.driver_sha256 != driver.driver_sha256
        || validation.helper_sha256 != driver.helper_sha256
    {
        return Err("Llama capture differs from its original product driver identity".into());
    }
    Ok(())
}

pub fn configuration() -> BTreeMap<String, BTreeMap<String, String>> {
    [
        ("engine", CacheRole::Engine),
        ("loader", CacheRole::Loader),
        ("shader", CacheRole::ShaderHost),
    ]
    .into_iter()
    .map(|(key, role)| (key.into(), recipe::scalar_configuration(role)))
    .collect()
}

pub fn build_configuration() -> BuildConfiguration {
    let mut flags = vec![
        "build-mode=serial-native-loader-and-engine".into(),
        "shader-generator=native-x64".into(),
    ];
    for (role, values) in configuration() {
        flags.extend(
            values
                .into_iter()
                .map(|(key, value)| format!("{role}:{key}={value}")),
        );
    }
    BuildConfiguration {
        target_triple: "aarch64-pc-windows-msvc".into(),
        profile: "Release".into(),
        flags,
        network_access_denied: true,
    }
}

pub fn configure_arguments(role: &str, work_root: &str) -> Result<Vec<String>, String> {
    windows_path(work_root)?;
    let templates: BTreeMap<String, Vec<String>> =
        serde_json::from_slice(include_bytes!("llama_windows_arm64_arguments.json"))
            .expect("reviewed native configure argument vectors");
    let args = templates
        .get(role)
        .ok_or("unknown ARM Llama configure role")?;
    let root = work_root.replace('\\', "/");
    Ok(args
        .iter()
        .map(|arg| arg.replace("@WORK@", &root))
        .collect())
}

fn expected_commands(work_root: &str) -> Result<Vec<(String, Vec<String>)>, String> {
    let root = work_root.replace('\\', "/");
    Ok(vec![
        (
            "cmake/bin/cmake.exe".into(),
            configure_arguments("loader", work_root)?,
        ),
        (
            "cmake/bin/cmake.exe".into(),
            vec![
                "--build".into(),
                format!("{root}/loader-build"),
                "--config".into(),
                "Release".into(),
                "--target".into(),
                "vulkan".into(),
                "--parallel".into(),
                "1".into(),
            ],
        ),
        (
            "python/python.exe".into(),
            vec![
                format!("{root}/source/loader/scripts/parse_asm_values.py"),
                format!("{root}/offsets-replayed.asm"),
                format!("{root}/loader-build/upstream/loader/Release/asm_offset.asm"),
                "MARMASM".into(),
                "MSVC".into(),
                "arm64".into(),
            ],
        ),
        (
            "cmake/bin/cmake.exe".into(),
            configure_arguments("engine", work_root)?,
        ),
        (
            "cmake/bin/cmake.exe".into(),
            vec![
                "--build".into(),
                format!("{root}/engine-build"),
                "--target".into(),
                "llama-server".into(),
                "--parallel".into(),
                "1".into(),
            ],
        ),
    ])
}

fn sdk_state_unchanged(before: &str, after: &str) -> Result<(), String> {
    let state: BTreeMap<String, serde_json::Value> =
        serde_json::from_str(before).map_err(|e| e.to_string())?;
    let mut keys = BTreeSet::new();
    for scope in ["Machine", "User"] {
        for name in ["PATH", "VULKAN_SDK", "VK_SDK_PATH"] {
            keys.insert(format!("{scope}/{name}"));
        }
    }
    keys.extend([
        "System32\\vulkan-1.dll".into(),
        "SysWOW64\\vulkan-1.dll".into(),
    ]);
    for prefix in [
        "HKLM:\\SOFTWARE\\Khronos\\Vulkan",
        "HKLM:\\SOFTWARE\\WOW6432Node\\Khronos\\Vulkan",
    ] {
        for kind in ["Drivers", "ExplicitLayers", "ImplicitLayers"] {
            keys.insert(format!("{prefix}\\{kind}"));
        }
    }
    if before != after
        || state.keys().cloned().collect::<BTreeSet<_>>() != keys
        || !state["Machine/PATH"].is_string()
    {
        return Err("Llama SDK global state changed or its census is incomplete".into());
    }
    Ok(())
}

fn validate_sources(
    validation: &NativeValidation,
    source: &BTreeMap<String, (u64, String)>,
) -> Result<(), String> {
    let mut patched = source.clone();
    // The same bounded patch and pinned upstream trees used by the x64 sibling.
    for (name, size, digest) in [
        (
            "llama/ggml/src/ggml-vulkan/CMakeLists.txt",
            11150,
            "3fa1145ac58b6d6f20b067054d8539b978c447a90f68855dd1f9fd226511291e",
        ),
        (
            "llama/ggml/src/ggml-vulkan/vulkan-shaders/vulkan-shaders-gen.cpp",
            82912,
            "8ec0c25922cc9f6895cc1fffa033a307cfe57d5f1c9bc8dd01e6c34445e1aa8a",
        ),
    ] {
        if patched.insert(name.into(), (size, digest.into())).is_none() {
            return Err("Llama patch target absent from pinned source".into());
        }
    }
    if validation.source_before != *source
        || validation.source_patched != patched
        || validation.source_after != patched
    {
        return Err(
            "Llama original/patched/final source census differs from the pinned closure".into(),
        );
    }
    Ok(())
}

fn validate_tools(validation: &NativeValidation) -> Result<(), String> {
    let census = ToolCensus {
        archive_tools: &validation.archive_tools,
        system_tools: &validation.system_tools,
        network_denied_tools: &validation.network_denied_tools,
    };
    windows_clang::validate_tool_paths_and_denies(
        &census,
        &validation.work_root,
        &validation.tool_paths,
        &validation.network_denies,
        &recipe::pinned_archive_tools(),
    )?;
    for required in [
        "msvc/armasm64.exe",
        "msvc/host-cl.exe",
        "msvc/host-link.exe",
        "msvc/host-lib.exe",
        "msvc/msbuild.exe",
        "windows/host-cc.exe",
        "windows/host-link.exe",
        "windows/host-lib.exe",
        "windows/shader-generator.exe",
    ] {
        if !validation
            .system_tools
            .iter()
            .any(|tool| tool.label == required)
        {
            return Err(format!("Llama system-tool census lacks {required}"));
        }
    }
    let work = windows_path(&validation.work_root)?;
    for (label, suffix) in [
        ("windows/host-cc.exe", "/host-proxies/host-cc.exe"),
        ("windows/host-link.exe", "/host-proxies/host-link.exe"),
        ("windows/host-lib.exe", "/host-proxies/host-lib.exe"),
        (
            "windows/shader-generator.exe",
            "/engine-build/release/vulkan-shaders-gen.exe",
        ),
    ] {
        if windows_path(&validation.tool_paths[label])? != format!("{work}{suffix}") {
            return Err("Llama generated host-tool path differs".into());
        }
    }
    let mut expected_before: Vec<_> = validation
        .archive_tools
        .iter()
        .chain(&validation.system_tools)
        .filter(|tool| tool.label != "windows/shader-generator.exe")
        .cloned()
        .collect();
    expected_before.sort_by(|left, right| left.label.cmp(&right.label));
    if validation.tools_before != expected_before {
        return Err("Llama native tool bytes changed during the build".into());
    }
    if validation.network_positive_error != 0
        || validation.network_blocked_before_error != 10013
        || validation.network_blocked_after_error != 10013
    {
        return Err("Llama native network positive/denied controls differ".into());
    }
    Ok(())
}

fn member<'a>(files: &'a BTreeMap<String, Vec<u8>>, name: &str) -> Result<&'a [u8], String> {
    files
        .get(name)
        .map(Vec::as_slice)
        .ok_or_else(|| format!("Llama capture member missing: {name}"))
}

pub fn inspect_capture(
    files: &BTreeMap<String, Vec<u8>>,
    source_archive: InputIdentityEntry,
    source_members: &BTreeMap<String, (u64, String)>,
    openmp: &AdmittedOpenMp,
    driver: &DriverSourceIdentity,
) -> Result<BuildEvidence, String> {
    let validation: NativeValidation =
        serde_json::from_slice(member(files, "report/validation.json")?)
            .map_err(|e| e.to_string())?;
    validate_driver_identity(&validation, driver)?;
    if validation.schema != VALIDATION_SCHEMA
        || validation
            .python_path_configuration
            .lines()
            .collect::<Vec<_>>()
            != ["python312.zip", "."]
        || validation.openmp_development != openmp.build_inputs()[4..]
    {
        return Err(
            "Llama capture schema, embedded Python or original OpenMP development cohort differ"
                .into(),
        );
    }
    if source_archive.label != "sources/llama-windows.tar.gz"
        || source_archive.size != crate::llama_windows_source::SOURCE_BYTES
        || source_archive.sha256 != crate::llama_windows_source::SOURCE_SHA256
    {
        return Err("Llama source archive identity differs".into());
    }
    validate_sources(&validation, source_members)?;
    validate_tools(&validation)?;
    let recorder = validation
        .system_tools
        .iter()
        .find(|tool| tool.label == "windows/recorder.exe")
        .ok_or("Llama original recorder identity missing")?;
    if recorder.sha256 != validation.recorder_sha256 {
        return Err("Llama native capture recorder bytes differ".into());
    }
    sdk_state_unchanged(&validation.sdk_state_before, &validation.sdk_state_after)?;
    let expected_cache_keys = BTreeSet::from(["engine", "loader", "shader"]);
    if validation
        .caches
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>()
        != expected_cache_keys
    {
        return Err("Llama cache roles differ".into());
    }
    let known_paths = validation
        .tool_paths
        .values()
        .map(|path| windows_path(path))
        .collect::<Result<BTreeSet<_>, _>>()?;
    for (key, role) in [
        ("engine", CacheRole::Engine),
        ("loader", CacheRole::Loader),
        ("shader", CacheRole::ShaderHost),
    ] {
        let bytes = validation.caches[key].as_bytes();
        recipe::validate_cache_configuration(
            role,
            bytes,
            &validation.work_root,
            &validation.tool_paths["msvc/nmake.exe"],
            &validation.tool_paths["msvc/armasm64.exe"],
        )?;
        let cache = crate::llama_windows_config::cache(bytes)?;
        for name in [
            "CMAKE_LINKER",
            "CMAKE_MAKE_PROGRAM",
            "CMAKE_AR",
            "CMAKE_MT",
            "CMAKE_RC_COMPILER",
            "CMAKE_C_COMPILER_AR",
            "CMAKE_CXX_COMPILER_AR",
            "CMAKE_NM",
            "CMAKE_OBJCOPY",
            "CMAKE_DLLTOOL",
            "CMAKE_VS_MSBUILD_COMMAND",
            "GIT_EXECUTABLE",
        ] {
            if let Some(path) = cache.get(name)
                && !path.is_empty()
                && !path.ends_with("-NOTFOUND")
                && !known_paths.contains(&windows_path(path)?)
            {
                return Err(format!(
                    "Llama cache selects an uncensused tool: {key}/{name}"
                ));
            }
        }
    }
    crate::llama_windows_arm64_generated::validate_generated_configuration(
        &validation
            .generated_configuration
            .iter()
            .map(|(name, text)| (name.clone(), text.as_bytes().to_vec()))
            .collect(),
        &validation.work_root,
    )?;
    if validation.commands.len() != BUILD_STEPS.len() {
        return Err("Llama native command population differs".into());
    }
    let expected = expected_commands(&validation.work_root)?;
    let mut expected_members = BTreeSet::from([
        "report/validation.json".to_owned(),
        "output/bin/llama-server.exe".into(),
        "output/bin/vulkan-1.dll".into(),
        "host/vulkan-shaders-gen.exe".into(),
        "proof/unknown_ext_chain_marmasm.obj".into(),
        "proof/gen_defines.asm".into(),
        "proof/offsets-replayed.asm".into(),
    ]);
    for ((step, command), (program, arguments)) in
        BUILD_STEPS.iter().zip(&validation.commands).zip(expected)
    {
        let path = format!("logs/{step}.log");
        expected_members.insert(path.clone());
        if command.label != *step
            || command.program_label != program
            || command.arguments != arguments
            || command.exit_code != 0
            || windows_path(&command.cwd)? != windows_path(&validation.work_root)?
            || sha256_hex(member(files, &path)?) != command.log_sha256
        {
            return Err(format!("Llama original native command differs: {step}"));
        }
    }
    for (role, label) in [
        ("cc", "windows/host-cc.exe"),
        ("link", "windows/host-link.exe"),
        ("lib", "windows/host-lib.exe"),
    ] {
        let c = format!("proof/host-{role}.c");
        let exe = format!("proof/host-{role}.exe");
        expected_members.insert(c.clone());
        expected_members.insert(exe.clone());
        if member(files, &c)?.is_empty() {
            return Err("Llama generated host proxy source is empty".into());
        }
        let bytes = member(files, &exe)?;
        let info = crate::pe_dependencies::inspect_dependencies(
            bytes,
            solstone_core_installed_payload::WindowsArchitecture::X86_64,
        )?;
        let identity = validation
            .system_tools
            .iter()
            .find(|tool| tool.label == label)
            .ok_or("Llama host proxy identity missing")?;
        if info.is_dll
            || identity.sha256 != sha256_hex(bytes)
            || identity.size != bytes.len() as u64
        {
            return Err("Llama generated host proxy bytes differ".into());
        }
    }
    if files.keys().cloned().collect::<BTreeSet<_>>() != expected_members {
        return Err("Llama original capture membership differs".into());
    }
    let facts = crate::produce::windows_llama_arm64::inspect_artifacts(
        member(files, "output/bin/llama-server.exe")?,
        member(files, "output/bin/vulkan-1.dll")?,
        member(files, "host/vulkan-shaders-gen.exe")?,
        member(files, "proof/unknown_ext_chain_marmasm.obj")?,
        member(files, "proof/gen_defines.asm")?,
        member(files, "proof/offsets-replayed.asm")?,
    )?;
    let shader = validation
        .system_tools
        .iter()
        .find(|tool| tool.label == "windows/shader-generator.exe")
        .ok_or("Llama shader identity missing")?;
    let shader_bytes = member(files, "host/vulkan-shaders-gen.exe")?;
    if shader.sha256 != sha256_hex(shader_bytes) || shader.size != shader_bytes.len() as u64 {
        return Err("Llama built shader generator differs from its fenced tool identity".into());
    }
    Ok(BuildEvidence {
        schema: EVIDENCE_SCHEMA.into(),
        product: validation.product,
        driver_sha256: validation.driver_sha256,
        helper_sha256: validation.helper_sha256,
        source_archive,
        host_inputs: recipe::pinned_host_inputs(),
        openmp_inputs: openmp.build_inputs(),
        configuration: configuration(),
        capture_members: files
            .iter()
            .map(|(label, bytes)| InputIdentityEntry {
                label: label.clone(),
                size: bytes.len() as u64,
                sha256: sha256_hex(bytes),
            })
            .collect(),
        artifact_facts: facts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provenance::Provenance;

    fn empty_capture() -> NativeValidation {
        serde_json::from_value(serde_json::json!({
            "schema": VALIDATION_SCHEMA,
            "product": {"commit": "a".repeat(40), "lock_sha256": "b".repeat(64)},
            "driver_sha256": "c".repeat(64), "helper_sha256": "d".repeat(64),
            "recorder_sha256": "e".repeat(64), "work_root": "C:/work",
            "archive_tools": [], "system_tools": [], "network_denied_tools": [],
            "tool_paths": {}, "network_denies": [], "tools_before": [],
            "source_before": {}, "source_patched": {}, "source_after": {},
            "caches": {}, "generated_configuration": {}, "openmp_development": [],
            "python_path_configuration": "python312.zip\n.\n",
            "sdk_state_before": "", "sdk_state_after": "",
            "network_positive_error": 0, "network_blocked_before_error": 10013,
            "network_blocked_after_error": 10013, "commands": []
        }))
        .unwrap()
    }

    #[test]
    fn original_product_commit_lock_and_driver_cannot_be_restamped() {
        let original = empty_capture();
        let driver = DriverSourceIdentity {
            product: Provenance {
                commit: "a".repeat(40),
                lock_sha256: "b".repeat(64),
            },
            driver_sha256: "c".repeat(64),
            helper_sha256: "d".repeat(64),
        };
        validate_driver_identity(&original, &driver).unwrap();
        for field in ["commit", "lock", "driver", "helper"] {
            let mut changed = original.clone();
            match field {
                "commit" => changed.product.commit = "f".repeat(40),
                "lock" => changed.product.lock_sha256 = "f".repeat(64),
                "driver" => changed.driver_sha256 = "f".repeat(64),
                "helper" => changed.helper_sha256 = "f".repeat(64),
                _ => unreachable!(),
            }
            assert!(
                validate_driver_identity(&changed, &driver).is_err(),
                "{field}"
            );
        }
    }

    fn sdk_state() -> BTreeMap<String, serde_json::Value> {
        let mut state = BTreeMap::new();
        for scope in ["Machine", "User"] {
            for name in ["PATH", "VULKAN_SDK", "VK_SDK_PATH"] {
                state.insert(format!("{scope}/{name}"), serde_json::json!("original"));
            }
        }
        for name in ["System32\\vulkan-1.dll", "SysWOW64\\vulkan-1.dll"] {
            state.insert(name.into(), serde_json::Value::Null);
        }
        for prefix in [
            "HKLM:\\SOFTWARE\\Khronos\\Vulkan",
            "HKLM:\\SOFTWARE\\WOW6432Node\\Khronos\\Vulkan",
        ] {
            for kind in ["Drivers", "ExplicitLayers", "ImplicitLayers"] {
                state.insert(format!("{prefix}\\{kind}"), serde_json::json!({}));
            }
        }
        state
    }

    #[test]
    fn sdk_must_preserve_complete_global_census_and_original_bytes() {
        let state = sdk_state();
        let raw = serde_json::to_string(&state).unwrap();
        sdk_state_unchanged(&raw, &raw).unwrap();
        for key in state.keys() {
            let mut missing = state.clone();
            missing.remove(key);
            let text = serde_json::to_string(&missing).unwrap();
            assert!(sdk_state_unchanged(&text, &text).is_err(), "{key}");
            let mut changed = state.clone();
            changed.insert(key.clone(), serde_json::json!("changed"));
            assert!(
                sdk_state_unchanged(&raw, &serde_json::to_string(&changed).unwrap()).is_err(),
                "{key}"
            );
        }
        assert!(sdk_state_unchanged(&raw, &format!("{raw}\n")).is_err());
        let mut extra = state;
        extra.insert("unexpected".into(), serde_json::Value::Null);
        let text = serde_json::to_string(&extra).unwrap();
        assert!(sdk_state_unchanged(&text, &text).is_err());
    }

    #[test]
    fn source_census_allows_only_the_two_bounded_patch_targets() {
        let source = BTreeMap::from([
            (
                "llama/ggml/src/ggml-vulkan/CMakeLists.txt".into(),
                (1, "a".repeat(64)),
            ),
            (
                "llama/ggml/src/ggml-vulkan/vulkan-shaders/vulkan-shaders-gen.cpp".into(),
                (2, "b".repeat(64)),
            ),
            ("source-manifest.json".into(), (3, "c".repeat(64))),
        ]);
        let mut capture = empty_capture();
        capture.source_before = source.clone();
        capture.source_patched = source.clone();
        capture.source_patched.insert(
            "llama/ggml/src/ggml-vulkan/CMakeLists.txt".into(),
            (
                11150,
                "3fa1145ac58b6d6f20b067054d8539b978c447a90f68855dd1f9fd226511291e".into(),
            ),
        );
        capture.source_patched.insert(
            "llama/ggml/src/ggml-vulkan/vulkan-shaders/vulkan-shaders-gen.cpp".into(),
            (
                82912,
                "8ec0c25922cc9f6895cc1fffa033a307cfe57d5f1c9bc8dd01e6c34445e1aa8a".into(),
            ),
        );
        capture.source_after = capture.source_patched.clone();
        validate_sources(&capture, &source).unwrap();
        for stage in ["before", "patched", "after"] {
            let mut changed = capture.clone();
            let census = match stage {
                "before" => &mut changed.source_before,
                "patched" => &mut changed.source_patched,
                "after" => &mut changed.source_after,
                _ => unreachable!(),
            };
            census.insert("source-manifest.json".into(), (4, "f".repeat(64)));
            assert!(validate_sources(&changed, &source).is_err(), "{stage}");
        }
        capture.source_after = source.clone();
        assert!(validate_sources(&capture, &source).is_err());
        let mut missing_target = source;
        missing_target.remove("llama/ggml/src/ggml-vulkan/CMakeLists.txt");
        assert!(validate_sources(&capture, &missing_target).is_err());
    }

    #[test]
    fn command_plan_keeps_loader_before_engine_and_replays_arm_offsets() {
        let commands = expected_commands("D:\\fresh cell").unwrap();
        assert_eq!(commands.len(), BUILD_STEPS.len());
        assert_eq!(commands[2].0, "python/python.exe");
        assert_eq!(&commands[2].1[3..], ["MARMASM", "MSVC", "arm64"]);
        assert_eq!(
            commands[1].1,
            [
                "--build",
                "D:/fresh cell/loader-build",
                "--config",
                "Release",
                "--target",
                "vulkan",
                "--parallel",
                "1"
            ]
        );
        assert!(commands[0].1.contains(&"ARM64".into()));
        assert!(
            commands[3]
                .1
                .contains(&"-DGGML_CPU_ARM_ARCH=armv8-a".into())
        );
        assert!(
            commands[3].1.contains(
                &"-DVulkan_LIBRARY=D:/fresh cell/loader-build/upstream/loader/Release/vulkan-1.lib"
                    .into()
            )
        );
        assert!(
            commands
                .iter()
                .flat_map(|(_, args)| args)
                .all(|arg| !arg.contains("@WORK@"))
        );
        assert!(configure_arguments("unknown", "D:/work").is_err());
        assert!(expected_commands("D:/work/../elsewhere").is_err());
    }
}
