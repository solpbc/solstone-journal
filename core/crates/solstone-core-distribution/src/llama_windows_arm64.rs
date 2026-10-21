// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Configuration boundaries for the ARM CPU/OpenMP/Vulkan engine, ARM loader
//! and x64 shader generator. These checks alone do not admit a build or receipt.

use std::collections::BTreeMap;

use crate::llama_windows_config::cache;
use crate::windows_clang::windows_path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CacheRole {
    Engine,
    Loader,
    ShaderHost,
}

impl CacheRole {
    fn key(self) -> &'static str {
        match self {
            Self::Engine => "engine",
            Self::Loader => "loader",
            Self::ShaderHost => "shader",
        }
    }
}

pub fn scalar_configuration(role: CacheRole) -> BTreeMap<String, String> {
    let mut profiles: BTreeMap<String, BTreeMap<String, String>> =
        serde_json::from_slice(include_bytes!("llama_windows_arm64_profile.json"))
            .expect("reviewed ARM Llama scalar configuration");
    profiles
        .remove(role.key())
        .expect("closed Llama cache role")
}

fn require_path(
    values: &BTreeMap<String, String>,
    key: &str,
    expected: &str,
) -> Result<(), String> {
    let value = values
        .get(key)
        .ok_or_else(|| format!("Llama ARM cache is missing {key}"))?;
    if windows_path(value)? != windows_path(expected)? {
        return Err(format!("Llama ARM cache uses a foreign input at {key}"));
    }
    Ok(())
}

/// `work_root` and the native MSVC program paths come from the caller's bound
/// source/tool capture. Engine and shader compilers deliberately have different
/// paths, while both configurations select the same censused host NMake.
pub fn validate_cache_configuration(
    role: CacheRole,
    bytes: &[u8],
    work_root: &str,
    nmake: &str,
    armasm: &str,
) -> Result<(), String> {
    let work = windows_path(work_root)?;
    let values = cache(bytes)?;
    for (key, expected) in scalar_configuration(role) {
        if values.get(&key) != Some(&expected) {
            return Err(format!(
                "Llama ARM {} configuration differs at {key}",
                role.key()
            ));
        }
    }
    match role {
        CacheRole::Engine => {
            for (key, suffix) in [
                ("CMAKE_HOME_DIRECTORY", "/source/llama"),
                ("CMAKE_C_COMPILER", "/llvm/bin/clang-cl.exe"),
                ("CMAKE_CXX_COMPILER", "/llvm/bin/clang-cl.exe"),
                ("Vulkan_GLSLC_EXECUTABLE", "/sdk/Bin/glslc.exe"),
                ("Vulkan_INCLUDE_DIR", "/sdk/Include"),
                (
                    "Vulkan_LIBRARY",
                    "/loader-build/upstream/loader/Release/vulkan-1.lib",
                ),
                ("SPIRV-Headers_DIR", "/sdk/Lib/cmake/SPIRV-Headers"),
                ("OpenMP_libomp_LIBRARY", "/openmp/lib/libomp.lib"),
                ("OpenMP_C_INCLUDE_DIR", "/openmp/include"),
                ("OpenMP_CXX_INCLUDE_DIR", "/openmp/include"),
                (
                    "GGML_VULKAN_SHADERS_GEN_TOOLCHAIN",
                    "/shader-host-toolchain.cmake",
                ),
            ] {
                require_path(&values, key, &format!("{work}{suffix}"))?;
            }
            require_path(&values, "CMAKE_MAKE_PROGRAM", nmake)?;
        }
        CacheRole::Loader => {
            require_path(
                &values,
                "CMAKE_HOME_DIRECTORY",
                &format!("{work}/source/loader-wrapper"),
            )?;
            require_path(
                &values,
                "VulkanHeaders_DIR",
                &format!("{work}/source/vulkan-headers"),
            )?;
            require_path(
                &values,
                "Python3_EXECUTABLE",
                &format!("{work}/python/python.exe"),
            )?;
            require_path(&values, "CMAKE_ASM_MARMASM_COMPILER", armasm)?;
        }
        CacheRole::ShaderHost => {
            for (key, suffix) in [
                (
                    "CMAKE_HOME_DIRECTORY",
                    "/source/llama/ggml/src/ggml-vulkan/vulkan-shaders",
                ),
                ("CMAKE_C_COMPILER", "/host-proxies/host-cc.exe"),
                ("CMAKE_CXX_COMPILER", "/host-proxies/host-cc.exe"),
                ("CMAKE_LINKER", "/host-proxies/host-link.exe"),
                ("CMAKE_AR", "/host-proxies/host-lib.exe"),
                ("CMAKE_TOOLCHAIN_FILE", "/shader-host-toolchain.cmake"),
            ] {
                require_path(&values, key, &format!("{work}{suffix}"))?;
            }
            require_path(&values, "CMAKE_MAKE_PROGRAM", nmake)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORK: &str = "C:/capture";
    const NMAKE: &str = "C:/BuildTools/VC/Tools/MSVC/14.44.35207/bin/Hostx64/x64/nmake.exe";
    const ARMASM: &str = "C:/BuildTools/VC/Tools/MSVC/14.44.35207/bin/Hostx64/arm64/armasm64.exe";

    fn original(role: CacheRole) -> &'static str {
        match role {
            CacheRole::Engine => {
                include_str!("../tests/fixtures/llama-windows-arm64/engine-cache.txt")
            }
            CacheRole::Loader => {
                include_str!("../tests/fixtures/llama-windows-arm64/loader-cache.txt")
            }
            CacheRole::ShaderHost => {
                include_str!("../tests/fixtures/llama-windows-arm64/shader-cache.txt")
            }
        }
    }

    fn relocated(role: CacheRole) -> String {
        let mut text = original(role).replace('\\', "/");
        for (old, new) in [
            (
                "C:/llama-arm-spike-a2/llvm-host/clang+llvm-22.1.6-x86_64-pc-windows-msvc",
                "C:/capture/llvm",
            ),
            (
                "C:/llama-arm-spike-a2/openmp-original/development",
                "C:/capture/openmp",
            ),
            ("C:/llama-arm-spike-a2/python-host", "C:/capture/python"),
            ("C:/llama-arm-spike-a2/source", "C:/capture/source"),
            ("C:/llama-arm-spike-a2/sdk", "C:/capture/sdk"),
            ("C:/llama-arm-spike-a4", "C:/capture"),
        ] {
            text = text.replace(old, new);
        }
        text
    }

    #[test]
    fn arm_profile_preserves_cpu_openmp_vulkan_and_portable_instruction_baseline() {
        let engine = scalar_configuration(CacheRole::Engine);
        for key in [
            "GGML_CPU",
            "GGML_OPENMP",
            "GGML_VULKAN",
            "LLAMA_BUILD_SERVER",
        ] {
            assert_eq!(engine[key], "ON", "{key}");
        }
        for key in [
            "GGML_NATIVE",
            "GGML_CPU_ALL_VARIANTS",
            "GGML_BACKEND_DL",
            "BUILD_SHARED_LIBS",
            "GGML_CCACHE",
            "GGML_OPENMP_FETCH",
            "GGML_AVX",
            "GGML_AVX2",
            "GGML_AVX512",
            "GGML_FMA",
            "GGML_F16C",
            "GGML_BMI2",
            "GGML_CUDA",
            "GGML_HIP",
            "GGML_METAL",
            "GGML_OPENCL",
            "GGML_SYCL",
            "GGML_RPC",
            "LLAMA_OPENSSL",
        ] {
            assert_eq!(engine[key], "OFF", "{key}");
        }
        assert_eq!(engine["GGML_CPU_ARM_ARCH"], "armv8-a");
        assert_eq!(engine["CMAKE_MSVC_RUNTIME_LIBRARY"], "MultiThreadedDLL");
        assert_eq!(engine["CMAKE_C_COMPILER_TARGET"], "aarch64-pc-windows-msvc");
        assert_eq!(
            scalar_configuration(CacheRole::Loader)["CMAKE_GENERATOR_PLATFORM"],
            "ARM64"
        );
    }

    #[test]
    fn native_caches_keep_all_scalar_controls_and_distinct_host_target_paths() {
        for role in [CacheRole::Engine, CacheRole::Loader, CacheRole::ShaderHost] {
            let text = relocated(role);
            validate_cache_configuration(role, text.as_bytes(), WORK, NMAKE, ARMASM).unwrap();
            for (key, expected) in scalar_configuration(role) {
                let mut values = cache(text.as_bytes()).unwrap();
                values.insert(key.clone(), format!("{expected}-foreign"));
                let changed = values
                    .into_iter()
                    .map(|(key, value)| format!("{key}:STRING={value}\n"))
                    .collect::<String>();
                assert!(
                    validate_cache_configuration(role, changed.as_bytes(), WORK, NMAKE, ARMASM)
                        .is_err(),
                    "accepted altered {key}"
                );
            }
            assert!(
                validate_cache_configuration(role, text.as_bytes(), "C:/foreign", NMAKE, ARMASM)
                    .is_err()
            );
            let duplicate = format!("{text}\nCMAKE_GENERATOR:STRING=NMake Makefiles\n");
            assert!(
                validate_cache_configuration(role, duplicate.as_bytes(), WORK, NMAKE, ARMASM)
                    .is_err()
            );
        }
    }

    #[test]
    fn compiler_and_openmp_path_substitutions_refuse() {
        for (role, old, new) in [
            (
                CacheRole::Engine,
                "C:/capture/llvm/bin/clang-cl.exe",
                "C:/foreign/clang-cl.exe",
            ),
            (
                CacheRole::Engine,
                "C:/capture/openmp/lib/libomp.lib",
                "C:/foreign/libomp.lib",
            ),
            (
                CacheRole::Engine,
                "C:/capture/loader-build/upstream/loader/Release/vulkan-1.lib",
                "C:/capture/sdk/Lib/vulkan-1.lib",
            ),
            (
                CacheRole::ShaderHost,
                "C:/capture/host-proxies/host-cc.exe",
                "C:/capture/llvm/bin/clang-cl.exe",
            ),
            (CacheRole::Loader, ARMASM, "C:/foreign/armasm64.exe"),
        ] {
            let original = relocated(role);
            assert!(original.contains(old), "missing mutation control: {old}");
            let changed = original.replace(old, new);
            assert!(
                validate_cache_configuration(role, changed.as_bytes(), WORK, NMAKE, ARMASM)
                    .is_err()
            );
        }
    }
}
