// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Configuration checks for the retained Windows Vulkan build. These checks
//! are necessary evidence, not by themselves build or package admission.

use quick_xml::{Reader, events::Event};
use std::collections::BTreeMap;

type Result<T> = std::result::Result<T, String>;

pub(crate) fn cache(bytes: &[u8]) -> Result<BTreeMap<String, String>> {
    let text = std::str::from_utf8(bytes).map_err(|_| "CMake cache is not UTF-8")?;
    let mut values = BTreeMap::new();
    for line in text
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with(['#', '/']))
    {
        let (key, value) = line.split_once('=').ok_or("malformed CMake cache entry")?;
        let (key, _) = key
            .rsplit_once(':')
            .ok_or("CMake cache entry lacks a type")?;
        if values.insert(key.to_owned(), value.to_owned()).is_some() {
            return Err(format!("duplicate CMake cache key: {key}"));
        }
    }
    Ok(values)
}

fn require(values: &BTreeMap<String, String>, key: &str, expected: &str) -> Result<()> {
    if values.get(key).map(String::as_str) != Some(expected) {
        return Err(format!("{key}: expected {expected:?}"));
    }
    Ok(())
}

fn require_path(values: &BTreeMap<String, String>, key: &str, expected: &str) -> Result<()> {
    if values
        .get(key)
        .map(|v| v.replace('\\', "/").to_ascii_lowercase())
        != Some(expected.replace('\\', "/").to_ascii_lowercase())
    {
        return Err(format!("{key}: expected the retained SDK/source path"));
    }
    Ok(())
}

pub(crate) fn validate_engine_cache(bytes: &[u8], run_root: &str) -> Result<()> {
    let values = cache(bytes)?;
    for (key, value) in [
        ("CMAKE_GENERATOR", "Visual Studio 17 2022"),
        ("CMAKE_GENERATOR_PLATFORM", "x64"),
        ("CMAKE_MSVC_RUNTIME_LIBRARY", "MultiThreadedDLL"),
        ("CMAKE_C_FLAGS", "/DWIN32 /D_WINDOWS"),
        ("CMAKE_CXX_FLAGS", "/DWIN32 /D_WINDOWS /EHsc"),
        ("CMAKE_C_FLAGS_RELEASE", "/O2 /Ob2 /DNDEBUG"),
        ("CMAKE_CXX_FLAGS_RELEASE", "/O2 /Ob2 /DNDEBUG"),
        ("GGML_AVAILABLE_BACKENDS", "ggml-cpu;ggml-vulkan"),
        ("GGML_BACKEND_DIR", ""),
        ("GGML_VULKAN_SHADERS_GEN_TOOLCHAIN", ""),
    ] {
        require(&values, key, value)?;
    }
    for key in [
        "GGML_VULKAN",
        "GGML_CPU",
        "GGML_AVX",
        "GGML_AVX2",
        "GGML_FMA",
        "GGML_F16C",
        "GGML_BMI2",
        "LLAMA_BUILD_SERVER",
        "LLAMA_BUILD_TOOLS",
        "LLAMA_BUILD_COMMON",
    ] {
        require(&values, key, "ON")?;
    }
    for key in [
        "BUILD_SHARED_LIBS",
        "GGML_BACKEND_DL",
        "GGML_CPU_ALL_VARIANTS",
        "GGML_NATIVE",
        "GGML_CCACHE",
        "GGML_LLAMAFILE",
        "GGML_CUDA",
        "GGML_HIP",
        "GGML_METAL",
        "GGML_OPENCL",
        "GGML_SYCL",
        "GGML_RPC",
        "GGML_BLAS",
        "GGML_AVX512",
        "GGML_AVX512_BF16",
        "GGML_AVX512_VBMI",
        "GGML_AVX512_VNNI",
        "GGML_AVX_VNNI",
        "GGML_MUSA",
        "GGML_WEBGPU",
        "GGML_OPENVINO",
        "GGML_HEXAGON",
        "GGML_VIRTGPU",
        "GGML_VIRTGPU_BACKEND",
        "GGML_ZDNN",
        "GGML_ZENDNN",
        "LLAMA_BUILD_APP",
        "LLAMA_BUILD_TESTS",
        "LLAMA_BUILD_EXAMPLES",
        "LLAMA_BUILD_UI",
        "LLAMA_USE_PREBUILT_UI",
        "LLAMA_OPENSSL",
        "LLAMA_BUILD_BORINGSSL",
        "LLAMA_BUILD_LIBRESSL",
        "LLAMA_LLGUIDANCE",
        "LLAMA_USE_SYSTEM_GGML",
    ] {
        require(&values, key, "OFF")?;
    }
    for (key, suffix) in [
        ("Vulkan_GLSLC_EXECUTABLE", "sdk/Bin/glslc.exe"),
        (
            "Vulkan_GLSLANG_VALIDATOR_EXECUTABLE",
            "sdk/Bin/glslangValidator.exe",
        ),
        ("Vulkan_INCLUDE_DIR", "sdk/Include"),
        ("Vulkan_LIBRARY", "sdk/Lib/vulkan-1.lib"),
        ("SPIRV-Headers_DIR", "sdk/Lib/cmake/SPIRV-Headers"),
    ] {
        require_path(&values, key, &format!("{run_root}/{suffix}"))?;
    }
    Ok(())
}

pub(crate) fn validate_loader_cache(bytes: &[u8], run_root: &str) -> Result<()> {
    let values = cache(bytes)?;
    for (key, value) in [
        ("CMAKE_GENERATOR", "Visual Studio 17 2022"),
        ("CMAKE_GENERATOR_PLATFORM", "x64"),
        ("USE_MASM", "ON"),
        ("USE_GAS", "OFF"),
        ("UPDATE_DEPS", "OFF"),
        ("BUILD_TESTS", "OFF"),
        ("LOADER_CODEGEN", "OFF"),
        ("CODE_COVERAGE", "OFF"),
        ("ENABLE_WIN10_ONECORE", "OFF"),
        ("LOADER_USE_UNSAFE_FILE_SEARCH", "OFF"),
        ("CMAKE_C_FLAGS", "/DWIN32 /D_WINDOWS"),
        ("CMAKE_C_FLAGS_RELEASE", "/O2 /Ob2 /DNDEBUG"),
    ] {
        require(&values, key, value)?;
    }
    require_path(
        &values,
        "VulkanHeaders_DIR",
        &format!("{run_root}/source/vulkan-headers"),
    )
}

pub(crate) fn validate_shader_cache(bytes: &[u8]) -> Result<()> {
    let values = cache(bytes)?;
    for (key, value) in [
        ("CMAKE_GENERATOR", "Visual Studio 17 2022"),
        ("CMAKE_GENERATOR_PLATFORM", "x64"),
        ("CMAKE_MSVC_RUNTIME_LIBRARY", "MultiThreadedDLL"),
        ("CMAKE_CXX_FLAGS", "/DWIN32 /D_WINDOWS /GR /EHsc"),
        ("CMAKE_CXX_FLAGS_RELEASE", "/O2 /Ob2 /DNDEBUG"),
    ] {
        require(&values, key, value)?;
    }
    Ok(())
}

/// Extract only unconditionally declared Release compiler settings. Refuse
/// per-file or conditional overrides even when a harmless earlier value exists.
pub(crate) fn release_compiler(bytes: &[u8]) -> Result<BTreeMap<String, String>> {
    let text = std::str::from_utf8(bytes).map_err(|_| "vcxproj is not UTF-8")?;
    let mut reader = Reader::from_str(text);
    reader.config_mut().expand_empty_elements = true;
    let mut stack = Vec::<String>::new();
    let mut group = None;
    let mut active: Option<(String, String)> = None;
    let mut fields = BTreeMap::new();
    let mut roots = 0;
    let mut release_groups = 0;
    loop {
        match reader
            .read_event()
            .map_err(|e| format!("vcxproj XML: {e}"))?
        {
            Event::Start(tag) => {
                let name = String::from_utf8(tag.name().as_ref().to_vec())
                    .map_err(|_| "invalid XML name")?;
                if stack.is_empty() {
                    roots += 1;
                    if roots != 1 || name != "Project" {
                        return Err("expected one Project root".into());
                    }
                }
                if active.is_some() {
                    return Err("nested compiler setting".into());
                }
                let mut condition = None;
                for attr in tag.attributes() {
                    let attr = attr.map_err(|_| "invalid XML attribute")?;
                    if attr.key.as_ref() == b"Condition" {
                        condition = Some(
                            attr.decoded_and_normalized_value(
                                quick_xml::XmlVersion::Explicit1_0,
                                reader.decoder(),
                            )
                            .map_err(|_| "invalid condition")?
                            .into_owned(),
                        );
                    }
                }
                if name == "ItemDefinitionGroup" {
                    if group.is_some() || stack.as_slice() != ["Project"] {
                        return Err("unexpected compiler group".into());
                    }
                    group = Some(match condition.as_deref() {
                        Some("'$(Configuration)|$(Platform)'=='Release|x64'") => {
                            release_groups += 1;
                            true
                        }
                        Some(
                            "'$(Configuration)|$(Platform)'=='Debug|x64'"
                            | "'$(Configuration)|$(Platform)'=='RelWithDebInfo|x64'"
                            | "'$(Configuration)|$(Platform)'=='MinSizeRel|x64'",
                        ) => false,
                        _ => return Err("unsupported compiler group condition".into()),
                    });
                }
                if name == "ClCompile" && group == Some(true) && condition.is_some() {
                    return Err("conditional Release compiler".into());
                }
                if stack.last().is_some_and(|s| s == "ClCompile") {
                    if group.is_none() {
                        if name != "ObjectFileName"
                            || condition.is_some()
                            || stack.as_slice() != ["Project", "ItemGroup", "ClCompile"]
                        {
                            return Err("per-file compiler override".into());
                        }
                        let raw = reader.read_text(tag.name()).map_err(|e| e.to_string())?;
                        let value = raw.decode().map_err(|e| e.to_string())?;
                        let relative = value
                            .strip_prefix("$(IntDir)/")
                            .ok_or("unreviewed object path")?;
                        if !relative.ends_with(".obj")
                            || relative.contains(['$', ';', ':', '\\'])
                            || relative
                                .split('/')
                                .any(|p| p.is_empty() || matches!(p, "." | ".."))
                        {
                            return Err("unreviewed object path".into());
                        }
                        continue;
                    }
                    if group == Some(true) {
                        if condition.is_some() {
                            return Err("conditional Release setting".into());
                        }
                        active = Some((name.clone(), String::new()));
                    }
                }
                stack.push(name);
            }
            Event::Text(text) => {
                let text = text.decode().map_err(|_| "invalid XML text")?;
                if let Some((_, value)) = active.as_mut() {
                    value.push_str(&text);
                } else if stack.is_empty() && !text.trim().is_empty() {
                    return Err("text outside Project".into());
                }
            }
            Event::GeneralRef(reference) => {
                if let Some((_, value)) = active.as_mut() {
                    let name = reference.decode().map_err(|_| "invalid XML reference")?;
                    value.push_str(match name.as_ref() {
                        "quot" => "\"",
                        "apos" => "'",
                        "amp" => "&",
                        "lt" => "<",
                        "gt" => ">",
                        _ => return Err("unsupported XML reference".into()),
                    });
                }
            }
            Event::CData(_) | Event::DocType(_) => {
                return Err("unsupported XML declaration or CDATA".into());
            }
            Event::End(tag) => {
                if let Some((key, value)) = active.take()
                    && fields.insert(key, value.trim().to_owned()).is_some()
                {
                    return Err("duplicate Release compiler setting".into());
                }
                if tag.name().as_ref() == b"ItemDefinitionGroup" {
                    group = None;
                }
                stack.pop().ok_or("unbalanced XML")?;
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if !stack.is_empty() || roots != 1 || release_groups != 1 {
        return Err("incomplete or duplicate Release compiler group".into());
    }
    require(&fields, "RuntimeLibrary", "MultiThreadedDLL")?;
    Ok(fields)
}

pub(crate) fn validate_compiler(
    bytes: &[u8],
    additional_options: &str,
    bounded_vulkan: bool,
) -> Result<()> {
    let fields = release_compiler(bytes)?;
    if fields
        .get("AdditionalOptions")
        .map(String::as_str)
        .unwrap_or("")
        != additional_options
    {
        return Err("unreviewed Release AdditionalOptions".into());
    }
    if fields
        .get("EnableEnhancedInstructionSet")
        .is_some_and(|v| v != "NotSet")
    {
        return Err("unreviewed Release instruction set".into());
    }
    if bounded_vulkan {
        require(&fields, "MultiProcessorCompilation", "true")?;
        require(&fields, "ProcessorNumber", "1")?;
    } else if fields.contains_key("MultiProcessorCompilation")
        || fields.contains_key("ProcessorNumber")
    {
        return Err("unreviewed compiler parallelism".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(fields: &str) -> String {
        format!(
            "<Project><ItemDefinitionGroup Condition=\"'$(Configuration)|$(Platform)'=='Release|x64'\"><ClCompile><RuntimeLibrary>MultiThreadedDLL</RuntimeLibrary>{fields}</ClCompile></ItemDefinitionGroup></Project>"
        )
    }
    #[test]
    fn captured_caches_prove_backend_paths_runtime_and_disabled_fetches() {
        let engine = include_str!("../tests/fixtures/llama-windows/engine-cache.txt");
        let loader = include_str!("../tests/fixtures/llama-windows/loader-cache.txt");
        let shader = include_str!("../tests/fixtures/llama-windows/shader-cache.txt");
        validate_engine_cache(engine.as_bytes(), "C:/capture").unwrap();
        validate_loader_cache(loader.as_bytes(), "C:/capture").unwrap();
        validate_shader_cache(shader.as_bytes()).unwrap();
        for key in [
            "GGML_VULKAN",
            "BUILD_SHARED_LIBS",
            "GGML_BACKEND_DL",
            "GGML_NATIVE",
            "GGML_CUDA",
            "GGML_HIP",
            "LLAMA_BUILD_UI",
            "LLAMA_OPENSSL",
            "LLAMA_BUILD_BORINGSSL",
            "CMAKE_MSVC_RUNTIME_LIBRARY",
            "Vulkan_GLSLC_EXECUTABLE",
            "Vulkan_INCLUDE_DIR",
            "Vulkan_LIBRARY",
            "SPIRV-Headers_DIR",
        ] {
            let line = engine
                .lines()
                .find(|l| l.starts_with(&format!("{key}:")))
                .unwrap();
            let (typed_key, _) = line.split_once('=').unwrap();
            let changed = engine.replace(line, &format!("{typed_key}=unapproved"));
            assert!(
                validate_engine_cache(changed.as_bytes(), "C:/capture").is_err(),
                "{key}"
            );
        }
        for key in [
            "UPDATE_DEPS",
            "LOADER_CODEGEN",
            "BUILD_TESTS",
            "USE_MASM",
            "LOADER_USE_UNSAFE_FILE_SEARCH",
            "VulkanHeaders_DIR",
        ] {
            let line = loader
                .lines()
                .find(|l| l.starts_with(&format!("{key}:")))
                .unwrap();
            let (typed_key, _) = line.split_once('=').unwrap();
            assert!(
                validate_loader_cache(
                    loader
                        .replace(line, &format!("{typed_key}=unapproved"))
                        .as_bytes(),
                    "C:/capture"
                )
                .is_err(),
                "{key}"
            );
        }
        assert!(
            validate_shader_cache(
                shader
                    .replace("MultiThreadedDLL", "MultiThreaded")
                    .as_bytes()
            )
            .is_err()
        );
    }

    #[test]
    fn nested_shader_command_requires_one_actual_serial_release_build() {
        let valid = include_str!("../tests/fixtures/llama-windows/shader-project.xml");
        let cmake = "C:/capture/cmake/cmake-3.31.12-windows-x86_64/bin/cmake.exe";
        let build = "C:/capture/engine-build/ggml/src/ggml-vulkan/vulkan-shaders-gen-prefix/src/vulkan-shaders-gen-build";
        validate_shader_commands(valid.as_bytes(), cmake, build).unwrap();
        for changed in [
            valid.replace("--parallel 1", "--parallel 16"),
            valid.replace("Release --parallel", "Debug --parallel"),
            valid.replace("cd C:", "cd D:"),
            valid.replace("setlocal", "setlocal\nMSBuild.exe /m"),
            valid.replace(
                "</Project>",
                "<BuildInParallel>true</BuildInParallel></Project>",
            ),
        ] {
            assert!(validate_shader_commands(changed.as_bytes(), cmake, build).is_err());
        }
        let fanout = include_str!("../tests/fixtures/llama-windows/vulkan-custom.xml");
        validate_custom_serialization(fanout.as_bytes()).unwrap();
        let changed = fanout.replace(
            "</CustomBuild>",
            "<BuildInParallel>true</BuildInParallel></CustomBuild>",
        );
        assert!(validate_custom_serialization(changed.as_bytes()).is_err());
    }

    #[test]
    fn all_caches_bind_source_build_and_denied_toolchain_roots() {
        let vs = "C:/Program Files (x86)/Microsoft Visual Studio/2022/BuildTools";
        for (bytes, source, build) in [
            (
                include_str!("../tests/fixtures/llama-windows/engine-cache.txt"),
                "source/llama",
                "engine-build",
            ),
            (
                include_str!("../tests/fixtures/llama-windows/loader-cache.txt"),
                "source/loader-wrapper",
                "loader-build",
            ),
            (
                include_str!("../tests/fixtures/llama-windows/shader-cache.txt"),
                "source/llama/ggml/src/ggml-vulkan/vulkan-shaders",
                "engine-build/ggml/src/ggml-vulkan/vulkan-shaders-gen-prefix/src/vulkan-shaders-gen-build",
            ),
        ] {
            validate_cache_binding(bytes.as_bytes(), "C:/capture", source, build, vs).unwrap();
            for key in [
                "CMAKE_HOME_DIRECTORY",
                "CMAKE_CACHEFILE_DIR",
                "CMAKE_GENERATOR_INSTANCE",
                "CMAKE_GENERATOR_TOOLSET",
            ] {
                let line = bytes
                    .lines()
                    .find(|l| l.starts_with(&format!("{key}:")))
                    .unwrap();
                let typed = line.split_once('=').unwrap().0;
                let changed = bytes.replace(line, &format!("{typed}=elsewhere"));
                assert!(
                    validate_cache_binding(changed.as_bytes(), "C:/capture", source, build, vs)
                        .is_err(),
                    "{key}"
                );
            }
        }
    }

    #[test]
    fn cache_rejects_duplicate_keys_even_with_different_types() {
        assert!(cache(b"GGML_VULKAN:BOOL=ON\nGGML_VULKAN:STRING=OFF\n").is_err());
    }
    #[test]
    fn release_settings_require_actual_dll_runtime_and_fixed_parallelism() {
        let valid = project(
            "<MultiProcessorCompilation>true</MultiProcessorCompilation><ProcessorNumber>1</ProcessorNumber>",
        );
        validate_compiler(valid.as_bytes(), "", true).unwrap();
        for changed in [valid.replace("MultiThreadedDLL", "MultiThreaded"), valid.replace(">1<", ">8<"), valid.replace("<ProcessorNumber>1</ProcessorNumber>", "<!-- <ProcessorNumber>1</ProcessorNumber> -->"), valid.replace("<RuntimeLibrary>", "<RuntimeLibrary Condition=\"false\">"), valid.replace("</ClCompile>", "<RuntimeLibrary>MultiThreadedDLL</RuntimeLibrary></ClCompile>"), valid.replace("</Project>", "<ItemGroup><ClCompile Include=\"x.cpp\"><RuntimeLibrary>MultiThreaded</RuntimeLibrary></ClCompile></ItemGroup></Project>")] {
            assert!(validate_compiler(changed.as_bytes(), "", true).is_err());
        }
    }
    #[test]
    fn compiler_options_cannot_override_runtime_or_instruction_contract() {
        let valid =
            project("<AdditionalOptions>%(AdditionalOptions) /utf-8 /bigobj</AdditionalOptions>");
        validate_compiler(
            valid.as_bytes(),
            "%(AdditionalOptions) /utf-8 /bigobj",
            false,
        )
        .unwrap();
        for option in ["/MT", "/arch:AVX512", "/MP8"] {
            assert!(
                validate_compiler(
                    valid.replace("/bigobj", option).as_bytes(),
                    "%(AdditionalOptions) /utf-8 /bigobj",
                    false
                )
                .is_err()
            );
        }
    }
}

pub(crate) fn validate_shader_commands(bytes: &[u8], cmake: &str, build_dir: &str) -> Result<()> {
    let mut reader =
        Reader::from_str(std::str::from_utf8(bytes).map_err(|_| "shader project is not UTF-8")?);
    reader.config_mut().expand_empty_elements = true;
    let mut builds = 0;
    loop {
        match reader.read_event().map_err(|e| e.to_string())? {
            Event::Start(tag) if tag.name().as_ref() == b"Command" => {
                let mut release = false;
                for attr in tag.attributes() {
                    let attr = attr.map_err(|e| e.to_string())?;
                    if attr.key.as_ref() == b"Condition" {
                        release = attr
                            .decoded_and_normalized_value(
                                quick_xml::XmlVersion::Explicit1_0,
                                reader.decoder(),
                            )
                            .map_err(|e| e.to_string())?
                            == "'$(Configuration)|$(Platform)'=='Release|x64'";
                    }
                }
                let raw = reader.read_text(tag.name()).map_err(|e| e.to_string())?;
                let decoded = raw.decode().map_err(|e| e.to_string())?;
                let value = quick_xml::escape::unescape(&decoded).map_err(|e| e.to_string())?;
                if release && value.contains("--build") {
                    builds += 1;
                    let normalized = value.replace('\\', "/").replace("\r\n", "\n");
                    let build_dir = build_dir.replace('\\', "/");
                    let drive = build_dir.get(..2).ok_or("missing build drive")?;
                    let expected = format!(
                        "setlocal\ncd {build_dir}\nif %errorlevel% neq 0 goto :cmEnd\n{drive}\nif %errorlevel% neq 0 goto :cmEnd\n{} --build . --config Release --parallel 1\nif %errorlevel% neq 0 goto :cmEnd\n:cmEnd\nendlocal & call :cmErrorLevel %errorlevel% & goto :cmDone\n:cmErrorLevel\nexit /b %1\n:cmDone\nif %errorlevel% neq 0 goto :VCEnd",
                        cmake.replace('\\', "/")
                    );
                    if normalized != expected {
                        return Err("nested shader build command block disagrees with serial Release recipe".into());
                    }
                }
            }
            Event::Start(tag) if tag.name().as_ref() == b"BuildInParallel" => {
                if reader
                    .read_text(tag.name())
                    .map_err(|e| e.to_string())?
                    .decode()
                    .map_err(|e| e.to_string())?
                    .trim()
                    != "false"
                {
                    return Err("parallel custom builds refused".into());
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if builds != 1 {
        return Err("expected exactly one nested Release build command".into());
    }
    Ok(())
}

pub(crate) fn validate_custom_serialization(bytes: &[u8]) -> Result<()> {
    let mut reader =
        Reader::from_str(std::str::from_utf8(bytes).map_err(|_| "project is not UTF-8")?);
    reader.config_mut().expand_empty_elements = true;
    loop {
        match reader.read_event().map_err(|e| e.to_string())? {
            Event::Start(tag) if tag.name().as_ref() == b"BuildInParallel" => {
                let raw = reader.read_text(tag.name()).map_err(|e| e.to_string())?;
                if raw.decode().map_err(|e| e.to_string())?.trim() != "false" {
                    return Err("parallel shader custom builds refused".into());
                }
            }
            Event::Eof => return Ok(()),
            _ => {}
        }
    }
}

pub(crate) fn validate_cache_binding(
    bytes: &[u8],
    root: &str,
    source: &str,
    build: &str,
    vs_root: &str,
) -> Result<()> {
    let values = cache(bytes)?;
    require_path(&values, "CMAKE_HOME_DIRECTORY", &format!("{root}/{source}"))?;
    require_path(&values, "CMAKE_CACHEFILE_DIR", &format!("{root}/{build}"))?;
    require_path(&values, "CMAKE_GENERATOR_INSTANCE", vs_root)?;
    require(
        &values,
        "CMAKE_GENERATOR_TOOLSET",
        "host=x64,version=14.44.35207",
    )
}
