// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! ARM ONNX Runtime production and retained-byte admission. The original x64
//! source/mirror validators and recorder keep their original schema and target.

use super::windows_build::{capture_source, read_bounded};
use super::windows_inputs::{
    AdmittedControlledInput, snapshot_file, verify_receipt_document_binding, with_snapshot,
};
use crate::controlled_build::{
    BuildConfiguration, BuilderIdentity, ControlledBuildReceipt, DependencySource,
    InputIdentityEntry, SourceIdentity, SupportingArtifactRef, ValidationReference, census_outputs,
    decode_controlled_build_receipt,
};
use crate::digest::sha256_hex;
use crate::onnx_windows_source as source;
use crate::windows_clang::{NetworkDeny, parse_cache, windows_path};
use solstone_core_installed_payload::WindowsArchitecture;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};

const DOCUMENT_LIMIT: u64 = 4 * 1024 * 1024;
const VALIDATION_LIMIT: u64 = 64 * 1024 * 1024;
const EVIDENCE_LABEL: &str = "provenance/windows-aarch64/onnxruntime-build-evidence.json";
const EVIDENCE_SCHEMA: &str = "solstone.onnx-windows-arm64-build-evidence.v1";
const VALIDATION_SCHEMA: &str = "solstone.onnx-windows-arm64-validation.v1";
const OUTPUT: &str = "bin/onnxruntime.dll";
const SOURCE_PATCH: &str = "core/distribution/patches/onnxruntime-arm64-fp16-activation.patch";
const SOURCE_PATCH_BYTES: &[u8] =
    include_bytes!("../../../../distribution/patches/onnxruntime-arm64-fp16-activation.patch");
const PATCHED_PROVIDER_CMAKE_SHA256: &str =
    "f3ecaa6f0a9d61b22a36a7fd71c05f1c96649aff145e9da4380967e339a4c0d4";
pub const KLEIDIAI_MIRROR: &str =
    "github.com/ARM-software/kleidiai/archive/refs/tags/v1.20.0.tar.gz";

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OnnxArm64Files {
    pub source_archive: PathBuf,
    pub mirror_archive: PathBuf,
    pub kleidiai_archive: PathBuf,
    pub cmake_archive: PathBuf,
    pub python_archive: PathBuf,
    pub protoc_archive: PathBuf,
}

pub struct OnnxArm64InputPaths<'a> {
    pub files: &'a OnnxArm64Files,
    pub receipt: &'a Path,
    pub evidence: &'a Path,
    pub validation: &'a Path,
    pub output_root: &'a Path,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Capture {
    schema: String,
    archive_tools: Vec<InputIdentityEntry>,
    system_tools: Vec<InputIdentityEntry>,
    supporting_inputs: Vec<InputIdentityEntry>,
    network_denied_tools: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Evidence {
    schema: String,
    inputs: Vec<InputIdentityEntry>,
    capture: Capture,
    cache_sha256: String,
    build_log_sha256: String,
    projects_sha256: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Validation {
    schema: String,
    work_root: String,
    tool_paths: BTreeMap<String, String>,
    network_denies: Vec<NetworkDeny>,
    cache: String,
    build_log: String,
    python_path_configuration: String,
    projects: BTreeMap<String, String>,
    build_arguments: Vec<String>,
    provider_sources_cmake: String,
}

pub fn pinned_tools() -> Vec<InputIdentityEntry> {
    serde_json::from_slice(include_bytes!("../onnx_windows_arm64_tools.json"))
        .expect("committed ONNX host tools")
}

fn exact(bytes: &[u8], label: &str, size: u64, sha256: &str) -> Result<InputIdentityEntry, String> {
    if bytes.len() as u64 != size || sha256_hex(bytes) != sha256 {
        return Err(format!("ONNX ARM pinned input differs: {label}"));
    }
    Ok(InputIdentityEntry {
        label: label.into(),
        size,
        sha256: sha256.into(),
    })
}

type InspectedInputs = (Vec<InputIdentityEntry>, BTreeMap<String, Vec<u8>>);

fn source_inputs(repo: &Path, files: &OnnxArm64Files) -> Result<InspectedInputs, String> {
    with_snapshot(|root| {
        let src = snapshot_file(
            root,
            "source.tar.gz",
            &files.source_archive,
            512 * 1024 * 1024,
        )?;
        let mirror = snapshot_file(
            root,
            "mirror.tar.gz",
            &files.mirror_archive,
            128 * 1024 * 1024,
        )?;
        let cmake = snapshot_file(root, "cmake.zip", &files.cmake_archive, 128 * 1024 * 1024)?;
        let python = snapshot_file(root, "python.zip", &files.python_archive, 32 * 1024 * 1024)?;
        let protoc = snapshot_file(root, "protoc.zip", &files.protoc_archive, 16 * 1024 * 1024)?;
        let kleidiai = read_bounded(&files.kleidiai_archive, 8 * 1024 * 1024)?;
        // This ARM profile consumes the original sealed, deterministic source
        // and CPU mirror, plus one separately pinned ARM mirror member.
        exact(
            &read_bounded(&src, 512 * 1024 * 1024)?,
            "sources/onnxruntime-with-submodules.tar.gz",
            299042275,
            "e02777f4216104f8700321f3fdda9cebe5b7596f616dc916bc45fce943b21c56",
        )?;
        let inspected =
            source::inspect_onnx_windows_source_archive(&src).map_err(|e| e.to_string())?;
        let mirror = source::inspect_onnx_windows_mirror_archive(repo, &mirror)
            .map_err(|e| e.to_string())?;
        let mut notices = super::windows_inputs::onnx_source_notices(&src)?;
        let patch = read_bounded(&repo.join(SOURCE_PATCH), 16 * 1024)?;
        let patch = exact(
            &patch,
            "patches/onnxruntime-arm64-fp16-activation.patch",
            SOURCE_PATCH_BYTES.len() as u64,
            &sha256_hex(SOURCE_PATCH_BYTES),
        )?;
        let kleidi = exact(
            &kleidiai,
            "sources/kleidiai-v1.20.0.tar.gz",
            2372189,
            "e4b84c369f0f39af1660ac71c30f74038d4b89e1f20dde070bb96398382a111f",
        )?;
        let mut archive = tar::Archive::new(
            flate2::read::GzDecoder::new(kleidiai.as_slice()).take(64 * 1024 * 1024),
        );
        let mut notice = None;
        for entry in archive.entries().map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            if entry.path().map_err(|e| e.to_string())?
                == Path::new("kleidiai-1.20.0/LICENSES/Apache-2.0.txt")
            {
                if notice.is_some()
                    || !entry.header().entry_type().is_file()
                    || entry.size() != 10280
                {
                    return Err("ONNX ARM KleidiAI notice type/population differs".into());
                }
                let mut bytes = Vec::new();
                entry
                    .take(10281)
                    .read_to_end(&mut bytes)
                    .map_err(|e| e.to_string())?;
                exact(
                    &bytes,
                    "notices/kleidiai-LICENSE.txt",
                    10280,
                    "074e6e32c86a4c0ef8b3ed25b721ca23aca83df277cd88106ef7177c354615ff",
                )?;
                notice = Some(bytes);
            }
        }
        notices.insert(
            "notices/kleidiai-LICENSE.txt".into(),
            notice.ok_or("ONNX ARM KleidiAI notice missing")?,
        );
        Ok((
            vec![
                inspected.archive,
                mirror.archive,
                kleidi,
                source::inspect_cmake_archive(repo, &cmake).map_err(|e| e.to_string())?,
                source::inspect_python_archive(repo, &python).map_err(|e| e.to_string())?,
                source::inspect_protoc_archive(repo, &protoc).map_err(|e| e.to_string())?,
                patch,
            ],
            notices,
        ))
    })
}

pub fn cache_configuration() -> BTreeMap<String, String> {
    [
        ("CMAKE_GENERATOR", "Visual Studio 17 2022"),
        ("CMAKE_GENERATOR_PLATFORM", "ARM64"),
        ("CMAKE_MSVC_RUNTIME_LIBRARY", "MultiThreadedDLL"),
        ("onnxruntime_BUILD_SHARED_LIB", "ON"),
        ("onnxruntime_REDUCED_OPS_BUILD", "ON"),
        ("onnxruntime_DISABLE_CONTRIB_OPS", "ON"),
        ("onnxruntime_DISABLE_ML_OPS", "ON"),
        ("onnxruntime_USE_TELEMETRY", "OFF"),
        ("onnxruntime_BUILD_UNIT_TESTS", "OFF"),
        ("onnxruntime_USE_KLEIDIAI", "ON"),
        ("onnxruntime_USE_QMX_KLEIDIAI_COEXIST", "OFF"),
        ("onnxruntime_USE_CUDA", "OFF"),
        ("onnxruntime_USE_TENSORRT", "OFF"),
        ("onnxruntime_USE_DML", "OFF"),
        ("onnxruntime_USE_ROCM", "OFF"),
        ("onnxruntime_USE_OPENVINO", "OFF"),
        ("onnxruntime_USE_XNNPACK", "OFF"),
    ]
    .into_iter()
    .map(|(k, v)| (k.into(), v.into()))
    .collect()
}

fn configuration() -> BuildConfiguration {
    let mut flags = source::onnx_controlled_build_configuration().flags;
    flags.push("--arm64".into());
    flags.push(format!(
        "source-patch-sha256={}",
        sha256_hex(SOURCE_PATCH_BYTES)
    ));
    flags.extend(
        cache_configuration()
            .into_iter()
            .map(|(k, v)| format!("{k}={v}")),
    );
    BuildConfiguration {
        target_triple: WindowsArchitecture::Aarch64.rust_triple().into(),
        profile: "Release".into(),
        flags,
        network_access_denied: true,
    }
}

fn validate_capture(capture: &Capture, validation: &Validation) -> Result<(), String> {
    if capture.schema != "solstone.onnx-windows-arm64-build-capture.v1"
        || validation.schema != VALIDATION_SCHEMA
        || capture.archive_tools != pinned_tools()
        || !validation
            .build_log
            .lines()
            .any(|s| s == "ONNX_ARM64_WINDOWS_BUILD_EXIT=0")
    {
        return Err("ONNX ARM capture/schema/archive tools/success marker differs".into());
    }
    let work = windows_path(&validation.work_root)?;
    let cache = parse_cache(&validation.cache)?;
    for (key, value) in cache_configuration() {
        if cache.get(key.as_str()).copied() != Some(value.as_str()) {
            return Err(format!("ONNX ARM configuration differs at {key}"));
        }
    }
    for (key, suffix) in [
        ("CMAKE_HOME_DIRECTORY", "/source/cmake"),
        ("Python3_EXECUTABLE", "/python/python.exe"),
        ("ONNX_CUSTOM_PROTOC_EXECUTABLE", "/protoc/bin/protoc.exe"),
        ("onnxruntime_CMAKE_DEPS_MIRROR_DIR", "/mirror"),
    ] {
        if windows_path(
            cache
                .get(key)
                .ok_or_else(|| format!("ONNX ARM cache path missing: {key}"))?,
        )? != format!("{work}{suffix}")
        {
            return Err(format!("ONNX ARM foreign input path: {key}"));
        }
    }
    if normalize_arguments(&validation.build_arguments)
        != build_arguments(&work, &validation.tool_paths)?
    {
        return Err("ONNX ARM build arguments differ".into());
    }
    let expected_python = vec![
        "python312.zip".into(),
        ".".into(),
        format!("{work}/source/tools/ci_build"),
        format!("{work}/source/tools/python"),
        format!("{work}/flatbuffers/flatbuffers-23.5.26/python"),
    ];
    let actual_python = validation
        .python_path_configuration
        .lines()
        .map(|s| s.replace('\\', "/").to_ascii_lowercase())
        .collect::<Vec<_>>();
    if actual_python != expected_python {
        return Err("ONNX ARM embedded Python import closure differs".into());
    }
    let mut identities = BTreeMap::new();
    for tool in capture.archive_tools.iter().chain(&capture.system_tools) {
        if tool.size == 0
            || tool.sha256.len() != 64
            || !tool
                .sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || tool.label.split('/').count() != 2
                && !tool.label.starts_with("cmake/bin/")
                && !tool.label.starts_with("protoc/bin/")
            || !tool.label.ends_with(".exe")
            || tool.label.contains(['\\', ':'])
            || identities.insert(tool.label.as_str(), tool).is_some()
        {
            return Err("ONNX ARM invalid/duplicate tool identity".into());
        }
    }
    for label in [
        "msvc/cl.exe",
        "msvc/link.exe",
        "msvc/lib.exe",
        "msvc/armasm64.exe",
        "msbuild/MSBuild.exe",
        "sdk/rc.exe",
        "sdk/mt.exe",
        "windows/cmd.exe",
        "windows/powershell.exe",
        "windows/recorder.exe",
        "windows/git.exe",
        "windows/git-backend.exe",
        "windows/git-core.exe",
        "windows/git-http.exe",
        "windows/patch.exe",
    ] {
        if !identities.contains_key(label) {
            return Err(format!("ONNX ARM missing censused tool: {label}"));
        }
    }

    let backend = windows_path(
        validation
            .tool_paths
            .get("windows/git-backend.exe")
            .ok_or("ONNX ARM Git backend path missing")?,
    )?;
    let prefix = backend
        .strip_suffix("/bin/git.exe")
        .ok_or("ONNX ARM Git backend layout differs")?;
    let root = prefix
        .rsplit_once('/')
        .ok_or("ONNX ARM Git root missing")?
        .0;
    for (label, expected) in [
        (
            "windows/git-core.exe",
            format!("{prefix}/libexec/git-core/git.exe"),
        ),
        (
            "windows/git-http.exe",
            format!("{prefix}/libexec/git-core/git-remote-https.exe"),
        ),
        ("windows/patch.exe", format!("{root}/usr/bin/patch.exe")),
    ] {
        if windows_path(
            validation
                .tool_paths
                .get(label)
                .ok_or("ONNX ARM installed tool path missing")?,
        )? != expected
        {
            return Err("ONNX ARM installed Git/patch layout differs".into());
        }
    }
    let labels = identities.keys().copied().collect::<Vec<_>>();
    if labels
        != capture
            .network_denied_tools
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
        || labels
            != validation
                .tool_paths
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>()
        || labels.len() != validation.network_denies.len()
        || labels.len() > 4096
    {
        return Err("ONNX ARM incomplete network-deny census".into());
    }
    let mut support = BTreeSet::new();
    for item in &capture.supporting_inputs {
        if !matches!(
            item.label.as_str(),
            "windows/msys-2.0.dll" | "windows/msys-intl-8.dll" | "windows/msys-iconv-2.dll"
        ) || !support.insert(item.label.as_str())
            || item.size == 0
            || item.sha256.len() != 64
        {
            return Err("ONNX ARM patch DLL identity differs".into());
        }
    }
    if !support.contains("windows/msys-2.0.dll") || !support.contains("windows/msys-iconv-2.dll") {
        return Err("ONNX ARM patch DLL closure missing".into());
    }
    for tool in &capture.archive_tools {
        if windows_path(&validation.tool_paths[&tool.label])?
            != format!("{work}/{}", tool.label.to_ascii_lowercase())
        {
            return Err("ONNX ARM foreign extracted tool".into());
        }
    }
    for (rule, label) in validation.network_denies.iter().zip(labels) {
        if rule.label != label
            || !rule.enabled
            || rule.direction != "Outbound"
            || rule.action != "Block"
            || rule.profile != "Any"
            || rule.protocol != "Any"
            || rule.local_address != ["Any"]
            || rule.remote_address != ["Any"]
            || rule.local_port != ["Any"]
            || rule.remote_port != ["Any"]
            || windows_path(&rule.program)? != windows_path(&validation.tool_paths[label])?
        {
            return Err("ONNX ARM incomplete outbound block rule".into());
        }
    }
    for (key, label) in [
        ("CMAKE_C_COMPILER", "msvc/cl.exe"),
        ("CMAKE_CXX_COMPILER", "msvc/cl.exe"),
        ("CMAKE_LINKER", "msvc/link.exe"),
        ("CMAKE_AR", "msvc/lib.exe"),
        ("CMAKE_MAKE_PROGRAM", "msbuild/MSBuild.exe"),
        ("Patch_EXECUTABLE", "windows/patch.exe"),
        ("GIT_EXECUTABLE", "windows/git-backend.exe"),
    ] {
        if windows_path(
            cache
                .get(key)
                .ok_or_else(|| format!("ONNX ARM cache tool missing: {key}"))?,
        )? != windows_path(&validation.tool_paths[label])?
        {
            return Err(format!("ONNX ARM foreign selected tool: {key}"));
        }
    }
    if validation
        .projects
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>()
        != [
            "kleidiai",
            "onnxruntime",
            "onnxruntime_mlas",
            "onnxruntime_providers",
        ]
    {
        return Err("ONNX ARM generated target population differs".into());
    }
    for project in validation.projects.values() {
        validate_project(project)?;
    }
    if sha256_hex(validation.provider_sources_cmake.as_bytes()) != PATCHED_PROVIDER_CMAKE_SHA256 {
        return Err("ONNX ARM patched provider source configuration differs".into());
    }
    validate_provider_helper(
        &validation.projects["onnxruntime_providers"],
        &validation.work_root,
    )?;
    Ok(())
}

fn validate_provider_helper(text: &str, work_root: &str) -> Result<(), String> {
    use quick_xml::{Reader, events::Event};
    let mut reader = Reader::from_str(text);
    reader.config_mut().expand_empty_elements = true;
    let mut stack = Vec::new();
    let mut conditional = Vec::new();
    let mut count = 0;
    let mut helper_depth = None;
    let expected = windows_path(&format!(
        "{work_root}/source/onnxruntime/contrib_ops/cpu/fused_activation.cc"
    ))?;
    loop {
        match reader.read_event().map_err(|e| e.to_string())? {
            Event::Start(tag) => {
                if helper_depth.is_some() && tag.name().as_ref() == b"ExcludedFromBuild" {
                    return Err("ONNX ARM FP16 activation helper is excluded".into());
                }
                let mut condition = false;
                let mut include = None;
                for attribute in tag.attributes() {
                    let attribute = attribute.map_err(|e| e.to_string())?;
                    if attribute.key.as_ref() == b"Condition" {
                        condition = true;
                    }
                    if attribute.key.as_ref() == b"Include" {
                        include = Some(
                            attribute
                                .decoded_and_normalized_value(
                                    quick_xml::XmlVersion::Explicit1_0,
                                    reader.decoder(),
                                )
                                .map_err(|e| e.to_string())?
                                .into_owned(),
                        );
                    }
                }
                // CMake's generated vcxproj doubles Windows separators.
                if tag.name().as_ref() == b"ClCompile"
                    && let Some(include) = include
                    && windows_path(&include.replace("\\\\", "\\"))? == expected
                {
                    if condition
                        || conditional.contains(&true)
                        || stack.as_slice() != [b"Project".to_vec(), b"ItemGroup".to_vec()]
                    {
                        return Err(
                            "ONNX ARM FP16 activation helper is conditional or misplaced".into(),
                        );
                    }
                    count += 1;
                    helper_depth = Some(stack.len() + 1);
                }
                stack.push(tag.name().as_ref().to_vec());
                conditional.push(condition);
            }
            Event::End(_) => {
                if helper_depth == Some(stack.len()) {
                    helper_depth = None;
                }
                stack.pop();
                conditional.pop();
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if count != 1 {
        return Err("ONNX ARM FP16 activation helper population differs".into());
    }
    Ok(())
}

fn normalize_arguments(args: &[String]) -> Vec<String> {
    args.iter()
        .map(|arg| {
            if let Some((key, value)) = arg.split_once('=')
                && value.as_bytes().get(1) == Some(&b':')
            {
                return format!("{key}={}", value.replace('\\', "/").to_ascii_lowercase());
            }
            if arg.as_bytes().get(1) == Some(&b':') {
                arg.replace('\\', "/").to_ascii_lowercase()
            } else {
                arg.clone()
            }
        })
        .collect()
}

fn build_arguments(work: &str, paths: &BTreeMap<String, String>) -> Result<Vec<String>, String> {
    let mut args = vec![
        format!("{work}/source/tools/ci_build/build.py"),
        "--build_dir".into(),
        format!("{work}/source/build/Windows"),
        "--config".into(),
        "Release".into(),
        "--update".into(),
        "--build".into(),
        "--skip_tests".into(),
        "--skip_submodule_sync".into(),
        "--parallel".into(),
        "2".into(),
        "--arm64".into(),
        "--build_shared_lib".into(),
        "--include_ops_by_config".into(),
        format!("{work}/source/required-operators.config"),
        "--disable_contrib_ops".into(),
        "--disable_ml_ops".into(),
        "--path_to_protoc_exe".into(),
        format!("{work}/protoc/bin/protoc.exe"),
        "--cmake_path".into(),
        format!("{work}/cmake/bin/cmake.exe"),
        "--ctest_path".into(),
        format!("{work}/cmake/bin/ctest.exe"),
        "--cmake_generator".into(),
        "Visual Studio 17 2022".into(),
        "--cmake_deps_mirror_dir".into(),
        format!("{work}/mirror"),
        "--cmake_extra_defines".into(),
    ];
    for (key, value) in cache_configuration() {
        if !matches!(
            key.as_str(),
            "CMAKE_GENERATOR" | "CMAKE_GENERATOR_PLATFORM" | "onnxruntime_REDUCED_OPS_BUILD"
        ) {
            args.push(format!("{key}={value}"));
        }
    }
    args.push(format!("Python3_EXECUTABLE={work}/python/python.exe"));
    for (key, label) in [
        ("GIT_EXECUTABLE", "windows/git-backend.exe"),
        ("Patch_EXECUTABLE", "windows/patch.exe"),
        ("CMAKE_MAKE_PROGRAM", "msbuild/MSBuild.exe"),
    ] {
        args.push(format!(
            "{key}={}",
            windows_path(
                paths
                    .get(label)
                    .ok_or("ONNX ARM build argument tool missing")?
            )?
        ));
    }
    Ok(args)
}

fn validate_project(text: &str) -> Result<(), String> {
    let bytes = text.as_bytes();
    use quick_xml::{Reader, events::Event};
    let refuse = |reason: &str| format!("ONNX ARM generated project: {reason}");
    if bytes.len() > 8 * 1024 * 1024 {
        return Err(refuse("vcxproj exceeds evidence limit"));
    }
    let text =
        std::str::from_utf8(bytes).map_err(|_| refuse("vcxproj evidence is not valid UTF-8"))?;
    let mut reader = Reader::from_str(text);
    reader.config_mut().expand_empty_elements = true;
    let mut stack = Vec::<String>::new();
    let mut group = None;
    let mut active_field: Option<(String, String)> = None;
    let mut runtime_count = 0;
    let mut root_count = 0;
    loop {
        match reader
            .read_event()
            .map_err(|_| refuse("malformed vcxproj XML"))?
        {
            Event::Start(tag) => {
                let name = std::str::from_utf8(tag.name().as_ref())
                    .map_err(|_| refuse("invalid vcxproj element name"))?
                    .to_owned();
                if stack.is_empty() {
                    root_count += 1;
                    if name != "Project" || root_count != 1 {
                        return Err(refuse("vcxproj must have one Project root"));
                    }
                }
                if active_field.is_some() {
                    return Err(refuse("nested compiler setting is not supported"));
                }
                let mut condition = None;
                for attribute in tag.attributes() {
                    let attribute = attribute.map_err(|_| refuse("invalid vcxproj attribute"))?;
                    if attribute.key.as_ref() == b"Condition" {
                        condition = Some(
                            attribute
                                .decoded_and_normalized_value(
                                    quick_xml::XmlVersion::Explicit1_0,
                                    reader.decoder(),
                                )
                                .map_err(|_| refuse("invalid vcxproj condition"))?
                                .into_owned(),
                        );
                    }
                }
                if name == "ItemDefinitionGroup" {
                    if group.is_some() || stack.as_slice() != ["Project"] {
                        return Err(refuse("unexpected compiler group location"));
                    }
                    let condition = condition
                        .as_ref()
                        .ok_or_else(|| refuse("unconditional compiler group is not supported"))?;
                    group = Some(match condition.as_str() {
                        "'$(Configuration)|$(Platform)'=='Release|ARM64'" => true,
                        "'$(Configuration)|$(Platform)'=='Debug|ARM64'"
                        | "'$(Configuration)|$(Platform)'=='RelWithDebInfo|ARM64'"
                        | "'$(Configuration)|$(Platform)'=='MinSizeRel|ARM64'" => false,
                        _ => return Err(refuse("unsupported compiler group condition")),
                    });
                } else if name == "ClCompile" && group == Some(true) && condition.is_some() {
                    return Err(refuse(
                        "conditional Release compiler settings are not supported",
                    ));
                }
                if matches!(
                    name.as_str(),
                    "RuntimeLibrary" | "EnableEnhancedInstructionSet"
                ) && stack.last().is_some_and(|value| value == "ClCompile")
                {
                    if group.is_none() {
                        return Err(refuse("compiler override outside a configuration group"));
                    }
                    if group == Some(true) {
                        if condition.is_some() {
                            return Err(refuse(
                                "conditional Release compiler setting is not supported",
                            ));
                        }
                        active_field = Some((name.clone(), String::new()));
                    }
                }
                stack.push(name);
            }
            Event::Text(value) => {
                let value = value.decode().map_err(|_| refuse("invalid vcxproj text"))?;
                if let Some((_, content)) = active_field.as_mut() {
                    content.push_str(&value);
                } else if stack.is_empty() && !value.trim().is_empty() {
                    return Err(refuse("text outside vcxproj root"));
                }
            }
            Event::GeneralRef(_) | Event::CData(_) if active_field.is_some() => {
                return Err(refuse("encoded compiler setting is not supported"));
            }
            Event::End(tag) => {
                if let Some((name, value)) = active_field.take() {
                    let value = value.trim();
                    match name.as_str() {
                        "RuntimeLibrary" => {
                            runtime_count += 1;
                            if value != "MultiThreadedDLL" || runtime_count != 1 {
                                return Err(refuse(
                                    "Release must declare one dynamic CRT MultiThreadedDLL; static CRT or debug CRT refused",
                                ));
                            }
                        }
                        "EnableEnhancedInstructionSet" if value != "NotSet" => {
                            return Err(refuse("Release specifies a non-baseline ISA"));
                        }
                        _ => {}
                    }
                }
                if tag.name().as_ref() == b"ItemDefinitionGroup" {
                    group = None;
                }
                stack
                    .pop()
                    .ok_or_else(|| refuse("unbalanced vcxproj elements"))?;
            }
            Event::Decl(declaration) => {
                if declaration
                    .version()
                    .map_err(|_| refuse("invalid XML declaration"))?
                    .as_ref()
                    != b"1.0"
                {
                    return Err(refuse("unsupported XML version"));
                }
            }
            Event::DocType(_) => return Err(refuse("vcxproj DTD is not supported")),
            Event::Eof => break,
            _ => {}
        }
    }
    if root_count != 1 || !stack.is_empty() || runtime_count != 1 {
        return Err(refuse(
            "vcxproj lacks one complete Release/ARM64 dynamic CRT declaration",
        ));
    }
    Ok(())
}

fn evidence(
    inputs: Vec<InputIdentityEntry>,
    capture: Capture,
    v: &Validation,
) -> Result<Evidence, String> {
    validate_capture(&capture, v)?;
    Ok(Evidence {
        schema: EVIDENCE_SCHEMA.into(),
        inputs,
        capture,
        cache_sha256: sha256_hex(v.cache.as_bytes()),
        build_log_sha256: sha256_hex(v.build_log.as_bytes()),
        projects_sha256: sha256_hex(&serde_json::to_vec(&v.projects).map_err(|e| e.to_string())?),
    })
}

fn read_output(root: &Path) -> Result<Vec<u8>, String> {
    for (dir, name, directory) in [
        (root, "bin", true),
        (&root.join("bin"), "onnxruntime.dll", false),
    ] {
        let mut entries = std::fs::read_dir(dir).map_err(|e| e.to_string())?;
        let item = entries
            .next()
            .transpose()
            .map_err(|e| e.to_string())?
            .ok_or("ONNX ARM output missing")?;
        let kind = item.file_type().map_err(|e| e.to_string())?;
        if item.file_name() != std::ffi::OsStr::new(name)
            || entries.next().is_some()
            || kind.is_dir() != directory
            || !directory && !kind.is_file()
        {
            return Err("ONNX ARM output population differs".into());
        }
    }
    read_bounded(&root.join(OUTPUT), 128 * 1024 * 1024)
}

fn receipt(
    source: SourceIdentity,
    builder: BuilderIdentity,
    e: &Evidence,
    v: &[u8],
    output: &[u8],
) -> Result<ControlledBuildReceipt, String> {
    if source.windows_dependency
        != (DependencySource {
            repository: crate::onnx_windows::ONNX_RUNTIME_REPOSITORY.into(),
            revision: crate::onnx_windows::ONNX_RUNTIME_COMMIT.into(),
            content_sha256: e.inputs[0].sha256.clone(),
        })
    {
        return Err("ONNX ARM receipt source differs".into());
    }
    let pe = crate::pe_dependencies::inspect_dependencies(output, WindowsArchitecture::Aarch64)?;
    let census = crate::pe::parse_pe(output).map_err(|e| e.to_string())?;
    crate::rfdetr_windows::verify_cpu_only_imports(&census).map_err(|e| e.to_string())?;
    if !pe.is_dll
        || !pe.delay_imports.is_empty()
        || !pe.forwarders.is_empty()
        || pe.imports.iter().any(|s| {
            matches!(
                s.to_ascii_lowercase().as_str(),
                "vcruntime140_1.dll" | "vcomp140.dll"
            )
        })
        || !census
            .exports
            .iter()
            .any(|s| matches!(s,crate::pe::PeSymbol::Named(n) if n=="OrtGetApiBase"))
    {
        return Err("ONNX ARM output API/CPU/native CRT closure differs".into());
    }
    let mut inputs = e.inputs.clone();
    inputs.extend(e.capture.archive_tools.clone());
    inputs.extend(e.capture.system_tools.clone());
    inputs.extend(e.capture.supporting_inputs.clone());
    let result = ControlledBuildReceipt {
        schema: crate::controlled_build::CONTROLLED_BUILD_RECEIPT_SCHEMA_V1.into(),
        source,
        builder,
        inputs,
        configuration: configuration(),
        outputs: census_outputs(&[(OUTPUT, output)]).map_err(|e| e.to_string())?,
        supporting: vec![SupportingArtifactRef {
            label: EVIDENCE_LABEL.into(),
            sha256: sha256_hex(&serde_json::to_vec(e).map_err(|e| e.to_string())?),
        }],
        validation: ValidationReference {
            description: "ONNX Runtime Windows ARM64 reduced CPU build; network-denied original diagnostics and projects".into(),
            sha256: sha256_hex(v),
        },
    };
    result.validate().map_err(|e| e.to_string())?;
    Ok(result)
}

pub fn admit(
    repo: &Path,
    paths: OnnxArm64InputPaths<'_>,
) -> Result<AdmittedControlledInput, String> {
    let (inputs, mut notices) = source_inputs(repo, paths.files)?;
    let r = read_bounded(paths.receipt, DOCUMENT_LIMIT)?;
    let receipt_doc = decode_controlled_build_receipt(&r).map_err(|e| e.to_string())?;
    let e = read_bounded(paths.evidence, DOCUMENT_LIMIT)?;
    let original: Evidence = serde_json::from_slice(&e).map_err(|e| e.to_string())?;
    let v = read_bounded(paths.validation, VALIDATION_LIMIT)?;
    let validation: Validation = serde_json::from_slice(&v).map_err(|e| e.to_string())?;
    verify_receipt_document_binding(&receipt_doc, &e, &v, EVIDENCE_LABEL)?;
    let expected = evidence(inputs, original.capture.clone(), &validation)?;
    if serde_json::to_vec(&expected).map_err(|e| e.to_string())?
        != serde_json::to_vec(&original).map_err(|e| e.to_string())?
    {
        return Err("ONNX ARM evidence differs from original pinned inputs/capture".into());
    }
    let output = read_output(paths.output_root)?;
    if receipt(
        receipt_doc.source.clone(),
        receipt_doc.builder.clone(),
        &expected,
        &v,
        &output,
    )? != receipt_doc
    {
        return Err("ONNX ARM receipt differs from original build".into());
    }
    notices.insert(OUTPUT.into(), output);
    Ok(AdmittedControlledInput::new(receipt_doc, r, e, v, notices))
}

fn path(values: &BTreeMap<String, String>, name: &str) -> Result<PathBuf, String> {
    let p = PathBuf::from(&values[name]);
    if !p.is_absolute() || p.components().any(|c| c == std::path::Component::ParentDir) {
        return Err(format!("ONNX ARM ordinary absolute path required: {name}"));
    }
    Ok(p)
}

pub fn run_cli(start: &Path, args: &[String]) -> Result<String, String> {
    let (op, rest) = args
        .split_first()
        .ok_or("onnx-windows-arm64 verify-inputs|record|verify --inputs ABS_JSON [FLAG VALUE]")?;
    let mut allowed = vec!["--inputs"];
    match op.as_str() {
        "verify-inputs" => {}
        "verify" => allowed.extend(["--receipt", "--evidence", "--validation", "--output-root"]),
        "record" => allowed.extend([
            "--capture",
            "--receipt",
            "--evidence",
            "--validation",
            "--output-root",
            "--product-commit",
            "--cargo-lock-sha256",
            "--builder-host",
        ]),
        _ => return Err("unknown ONNX ARM operation".into()),
    }
    if !rest.len().is_multiple_of(2) {
        return Err("ONNX ARM expects flag/value pairs".into());
    }
    let mut values = BTreeMap::new();
    for pair in rest.chunks_exact(2) {
        if !allowed.contains(&pair[0].as_str())
            || pair[1].is_empty()
            || values.insert(pair[0].clone(), pair[1].clone()).is_some()
        {
            return Err("unknown/empty/duplicate ONNX ARM flag".into());
        }
    }
    if values.len() != allowed.len() {
        return Err("missing ONNX ARM flag".into());
    }
    let inventory = crate::inventory::repository_inventory_path(start)
        .ok_or("ONNX ARM needs its source checkout")?;
    let repo = inventory
        .ancestors()
        .nth(3)
        .ok_or("ONNX ARM missing product root")?;
    let files: OnnxArm64Files =
        serde_json::from_slice(&read_bounded(&path(&values, "--inputs")?, DOCUMENT_LIMIT)?)
            .map_err(|e| e.to_string())?;
    for p in [
        &files.source_archive,
        &files.mirror_archive,
        &files.kleidiai_archive,
        &files.cmake_archive,
        &files.python_archive,
        &files.protoc_archive,
    ] {
        if !p.is_absolute() || p.components().any(|c| c == std::path::Component::ParentDir) {
            return Err("ONNX ARM absolute input member required".into());
        }
    }
    if op == "verify" {
        let result = admit(
            repo,
            OnnxArm64InputPaths {
                files: &files,
                receipt: &path(&values, "--receipt")?,
                evidence: &path(&values, "--evidence")?,
                validation: &path(&values, "--validation")?,
                output_root: &path(&values, "--output-root")?,
            },
        )?;
        return Ok(format!(
            "ONNX_ARM64_WINDOWS_VERIFY_PASS output_sha256={}",
            result.receipt().outputs[0].pre_signing_sha256
        ));
    }
    let (inputs, _) = source_inputs(repo, &files)?;
    if op == "verify-inputs" {
        return serde_json::to_string(&serde_json::json!({"inputs":inputs,"tools":pinned_tools(),"configuration":cache_configuration(),"source_patch":SOURCE_PATCH,"patched_provider_cmake_sha256":PATCHED_PROVIDER_CMAKE_SHA256,"kleidiai_mirror":KLEIDIAI_MIRROR,"capture_schema":"solstone.onnx-windows-arm64-build-capture.v1","validation_schema":VALIDATION_SCHEMA})).map_err(|e|e.to_string());
    }
    let before = capture_source(repo)?;
    if before.commit != values["--product-commit"]
        || before.lock_sha256 != values["--cargo-lock-sha256"]
    {
        return Err("ONNX ARM recorder source differs from driver".into());
    }
    let capture: Capture =
        serde_json::from_slice(&read_bounded(&path(&values, "--capture")?, DOCUMENT_LIMIT)?)
            .map_err(|e| e.to_string())?;
    let v = read_bounded(&path(&values, "--validation")?, VALIDATION_LIMIT)?;
    let validation: Validation = serde_json::from_slice(&v).map_err(|e| e.to_string())?;
    let e = evidence(inputs, capture, &validation)?;
    let root = path(&values, "--output-root")?;
    let output = read_output(&root)?;
    let r = receipt(
        SourceIdentity {
            product: before.clone(),
            windows_dependency: DependencySource {
                repository: crate::onnx_windows::ONNX_RUNTIME_REPOSITORY.into(),
                revision: crate::onnx_windows::ONNX_RUNTIME_COMMIT.into(),
                content_sha256: e.inputs[0].sha256.clone(),
            },
        },
        BuilderIdentity {
            host: values["--builder-host"].clone(),
            toolchain:
                "MSVC x64 -> native ARM64; Visual Studio 17 2022; pinned x64 CMake/Python/protoc"
                    .into(),
        },
        &e,
        &v,
        &output,
    )?;
    if capture_source(repo)? != before {
        return Err("ONNX ARM source changed during recording".into());
    }
    super::windows_openmp::publish(
        &path(&values, "--evidence")?,
        &serde_json::to_vec(&e).map_err(|e| e.to_string())?,
    )?;
    let published = crate::controlled_build::write_controlled_build_receipt_exclusive(
        &path(&values, "--receipt")?,
        &r,
    )
    .map_err(|e| e.to_string())?;
    if matches!(
        published,
        crate::controlled_build::ControlledBuildReceiptPublication::PublicationUnconfirmed { .. }
    ) {
        return Err("ONNX ARM receipt publication unconfirmed".into());
    }
    Ok(format!(
        "ONNX_ARM64_WINDOWS_RECORD_PASS output_sha256={} publication={published:?}",
        r.outputs[0].pre_signing_sha256
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn provider_helper_refuses_missing_commented_conditional_and_excluded_items() {
        let item = r#"<ClCompile Include="C:\\work\\source\\onnxruntime\\contrib_ops\\cpu\\fused_activation.cc"/>"#;
        let wrap = |item: &str| format!("<Project><ItemGroup>{item}</ItemGroup></Project>");
        assert!(validate_provider_helper(&wrap(item), "C:/work").is_ok());
        assert!(validate_provider_helper(&wrap(""), "C:/work").is_err());
        assert!(validate_provider_helper(&wrap(&format!("<!-- {item} -->")), "C:/work").is_err());
        assert!(validate_provider_helper(&wrap(&format!("{item}{item}")), "C:/work").is_err());
        assert!(validate_provider_helper(&wrap(item), "C:/foreign").is_err());
        let conditional = item.replace(" Include=", " Condition=\"false\" Include=");
        assert!(validate_provider_helper(&wrap(&conditional), "C:/work").is_err());
        let excluded = item.replace(
            "/>",
            "><ExcludedFromBuild>true</ExcludedFromBuild></ClCompile>",
        );
        assert!(validate_provider_helper(&wrap(&excluded), "C:/work").is_err());
        let parent = wrap(item).replace("<ItemGroup>", "<ItemGroup Condition=\"false\">");
        assert!(validate_provider_helper(&parent, "C:/work").is_err());
    }

    #[test]
    fn generated_crt_evidence_refuses_comment_static_debug_and_wrong_target() {
        let valid = "<Project><ItemDefinitionGroup Condition=\"'$(Configuration)|$(Platform)'=='Release|ARM64'\"><ClCompile><RuntimeLibrary>MultiThreadedDLL</RuntimeLibrary></ClCompile></ItemDefinitionGroup></Project>";
        validate_project(valid).unwrap();
        for bad in [
            valid.replace("MultiThreadedDLL", "MultiThreaded"),
            valid.replace("MultiThreadedDLL", "MultiThreadedDebugDLL"),
            valid.replace("ARM64", "ARM64EC"),
            valid.replace(
                "<RuntimeLibrary>MultiThreadedDLL</RuntimeLibrary>",
                "<!-- <RuntimeLibrary>MultiThreadedDLL</RuntimeLibrary> -->",
            ),
            valid.replace(
                "</ClCompile>",
                "<RuntimeLibrary>MultiThreadedDLL</RuntimeLibrary></ClCompile>",
            ),
        ] {
            assert!(validate_project(&bad).is_err());
        }
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod source_tests {
    use super::*;
    #[test]
    #[ignore = "requires retained source, mirrors, and host archives"]
    fn real_onnx_inputs_preserve_models_operators_and_three_notices() {
        let file =
            PathBuf::from(std::env::var_os("SOLSTONE_ONNX_ARM_INPUTS").expect("ONNX input map"));
        let files: OnnxArm64Files = serde_json::from_slice(&std::fs::read(file).unwrap()).unwrap();
        let inventory =
            crate::inventory::repository_inventory_path(Path::new(env!("CARGO_MANIFEST_DIR")))
                .unwrap();
        let repo = inventory.ancestors().nth(3).unwrap();
        let (inputs, notices) = source_inputs(repo, &files).unwrap();
        assert_eq!(inputs.len(), 7);
        assert_eq!(
            notices.keys().map(String::as_str).collect::<Vec<_>>(),
            [
                "LICENSE",
                "ThirdPartyNotices.txt",
                "notices/kleidiai-LICENSE.txt"
            ]
        );
        let mut swapped = files.clone();
        swapped.protoc_archive = files.python_archive;
        assert!(source_inputs(repo, &swapped).is_err());
    }
}
