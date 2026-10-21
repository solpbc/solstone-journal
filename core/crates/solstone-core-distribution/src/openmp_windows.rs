// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Fixed source and host-tool authority for the shared Windows ARM64 OpenMP runtime.
//! Development members belong to its original build evidence, not the payload.

use std::collections::{BTreeMap, BTreeSet};

use crate::controlled_build::{
    BuildConfiguration, BuilderIdentity, ControlledBuildReceiptDraft, DependencySource,
    InputIdentityEntry, SourceIdentity, SupportingArtifactRef,
};
use crate::digest::sha256_hex;
use solstone_core_installed_payload::WindowsArchitecture;

pub const VERSION: &str = "22.1.6";
pub const REVISION: &str = "fc4aad7b5db3fff421df9a9637605b9ca5667881";
pub const SOURCE_URL: &str = "https://github.com/llvm/llvm-project/releases/download/llvmorg-22.1.6/llvm-project-22.1.6.src.tar.xz";
pub const LLVM_URL: &str = "https://github.com/llvm/llvm-project/releases/download/llvmorg-22.1.6/clang%2Bllvm-22.1.6-x86_64-pc-windows-msvc.tar.xz";
pub const CMAKE_URL: &str = "https://cmake.org/files/v3.31/cmake-3.31.12-windows-x86_64.zip";
pub const PYTHON_URL: &str =
    "https://www.python.org/ftp/python/3.12.10/python-3.12.10-embed-amd64.zip";
pub const SOURCE_SHA256: &str = "6e0b376a1f6d9873e7dfb09ae6e04b9c7024400f01733fa4c29be69d5c138bc2";
pub const OUTPUT_LABEL: &str = "libomp.dll";
pub const EVIDENCE_LABEL: &str = "provenance/windows-aarch64/openmp-build-evidence.json";
pub const EVIDENCE_SCHEMA: &str = "solstone.openmp-windows-build-evidence.v1";
pub const LICENSE_BYTES: u64 = 19741;
pub const LICENSE_SHA256: &str = "fdad1758a9e1f9d5a81e18879b3406772115edc92c24bfa36b70c654f325e8e4";
pub const DEVELOPMENT_LABELS: &[&str] = &["include/omp.h", "lib/libomp.lib"];

pub fn identity(label: &str, size: u64, sha256: &str) -> InputIdentityEntry {
    InputIdentityEntry {
        label: label.into(),
        size,
        sha256: sha256.into(),
    }
}

pub fn pinned_inputs() -> Vec<InputIdentityEntry> {
    vec![
        identity(
            "sources/llvm-project-22.1.6.src.tar.xz",
            167043464,
            SOURCE_SHA256,
        ),
        identity(
            "tools/clang+llvm-22.1.6-x86_64-pc-windows-msvc.tar.xz",
            861999212,
            "657343edf361ca463bd642e39c74b251c6338b96cdbd55ff277555298b027696",
        ),
        identity(
            "tools/cmake-3.31.12-windows-x86_64.zip",
            46666397,
            "0c4baa40f28b3f8225eb3fdf6946c987b4fe901403b4eaf2fbbd9378100aaa0c",
        ),
        identity(
            "tools/python-3.12.10-embed-amd64.zip",
            11133606,
            "4acbed6dd1c744b0376e3b1cf57ce906f9dc9e95e68824584c8099a63025a3c3",
        ),
    ]
}

pub fn pinned_tools() -> Vec<InputIdentityEntry> {
    serde_json::from_slice(include_bytes!("openmp_windows_tools.json"))
        .expect("committed OpenMP host-tool census")
}

pub fn cache_configuration() -> BTreeMap<String, String> {
    [
        ("CMAKE_GENERATOR", "NMake Makefiles"),
        ("CMAKE_SYSTEM_NAME", "Windows"),
        ("CMAKE_SYSTEM_PROCESSOR", "ARM64"),
        ("CMAKE_C_COMPILER_TARGET", "aarch64-pc-windows-msvc"),
        ("CMAKE_CXX_COMPILER_TARGET", "aarch64-pc-windows-msvc"),
        ("CMAKE_ASM_COMPILER_TARGET", "aarch64-pc-windows-msvc"),
        ("CMAKE_BUILD_TYPE", "Release"),
        ("CMAKE_MSVC_RUNTIME_LIBRARY", "MultiThreaded"),
        ("LIBOMP_ARCH", "aarch64"),
        ("LIBOMP_LIB_TYPE", "normal"),
        ("LIBOMP_ENABLE_SHARED", "ON"),
        ("LIBOMP_ENABLE_ASSERTIONS", "OFF"),
        ("OPENMP_ENABLE_LIBOMPTARGET", "OFF"),
        ("OPENMP_ENABLE_OMPT_TOOLS", "OFF"),
        ("LIBOMP_FORTRAN_MODULES", "OFF"),
    ]
    .into_iter()
    .map(|(k, v)| (k.into(), v.into()))
    .collect()
}

pub fn build_configuration() -> BuildConfiguration {
    let mut flags: Vec<_> = cache_configuration()
        .into_iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect();
    flags.extend(
        [
            "CMAKE_C_COMPILER=llvm/bin/clang-cl.exe",
            "CMAKE_CXX_COMPILER=llvm/bin/clang-cl.exe",
            "CMAKE_ASM_COMPILER=llvm/bin/clang-cl.exe",
            "Python3_EXECUTABLE=python/python.exe",
            "build-target=install",
            "build-mode=serial-nmake",
        ]
        .into_iter()
        .map(str::to_owned),
    );
    BuildConfiguration {
        target_triple: WindowsArchitecture::Aarch64.rust_triple().into(),
        profile: "release".into(),
        flags,
        network_access_denied: true,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenMpBuildEvidence {
    pub schema: String,
    pub inputs: Vec<InputIdentityEntry>,
    pub archive_tools: Vec<InputIdentityEntry>,
    /// Observed unowned build-host tools, retained separately from archive pins.
    pub system_tools: Vec<InputIdentityEntry>,
    pub network_denied_tools: Vec<String>,
    pub configuration: BTreeMap<String, String>,
    pub development: Vec<InputIdentityEntry>,
    pub cache_sha256: String,
    pub build_log_sha256: String,
    pub install_manifest_sha256: String,
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl OpenMpBuildEvidence {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema != EVIDENCE_SCHEMA
            || self.inputs != pinned_inputs()
            || self.archive_tools != pinned_tools()
            || self.configuration != cache_configuration()
            || !valid_digest(&self.cache_sha256)
            || !valid_digest(&self.build_log_sha256)
            || !valid_digest(&self.install_manifest_sha256)
        {
            return Err("OpenMP source, host tools or ARM configuration differ".into());
        }
        let mut tools = BTreeSet::new();
        for tool in &self.system_tools {
            let Some((prefix, name)) = tool.label.split_once('/') else {
                return Err("invalid OpenMP system-tool label".into());
            };
            if !matches!(prefix, "msvc" | "sdk" | "windows")
                || !name.ends_with(".exe")
                || !name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
                || tool.size == 0
                || !valid_digest(&tool.sha256)
                || !tools.insert(tool.label.clone())
            {
                return Err("invalid or duplicate OpenMP system tool".into());
            }
        }
        for required in [
            "msvc/link.exe",
            "msvc/lib.exe",
            "msvc/nmake.exe",
            "sdk/rc.exe",
            "sdk/mt.exe",
            "windows/cmd.exe",
            "windows/powershell.exe",
        ] {
            if !tools.contains(required) {
                return Err(format!("OpenMP system-tool census is missing {required}"));
            }
        }
        tools.extend(self.archive_tools.iter().map(|t| t.label.clone()));
        if self.network_denied_tools != tools.into_iter().collect::<Vec<_>>() {
            return Err("OpenMP outbound denial does not cover its complete tool census".into());
        }
        if self.development.len() != DEVELOPMENT_LABELS.len()
            || self
                .development
                .iter()
                .zip(DEVELOPMENT_LABELS)
                .any(|(file, label)| {
                    file.label != *label
                        || file.size == 0
                        || file.size > 16 * 1024 * 1024
                        || !valid_digest(&file.sha256)
                })
        {
            return Err("OpenMP development members differ from the fixed output set".into());
        }
        Ok(())
    }
}

pub fn source_identity(product: crate::provenance::Provenance) -> SourceIdentity {
    SourceIdentity {
        product,
        windows_dependency: DependencySource {
            repository: "https://github.com/llvm/llvm-project.git".into(),
            revision: REVISION.into(),
            content_sha256: SOURCE_SHA256.into(),
        },
    }
}

pub fn assemble_receipt_draft(
    source: SourceIdentity,
    evidence: &OpenMpBuildEvidence,
    builder: BuilderIdentity,
    output_bytes: &[u8],
) -> Result<ControlledBuildReceiptDraft, String> {
    evidence.validate()?;
    if source.windows_dependency != source_identity(source.product.clone()).windows_dependency {
        return Err("OpenMP controlled source identity differs".into());
    }
    let pe =
        crate::pe_dependencies::inspect_dependencies(output_bytes, WindowsArchitecture::Aarch64)?;
    let exports = crate::pe::parse_pe(output_bytes)
        .map_err(|e| e.to_string())?
        .exports;
    let imports: BTreeSet<_> = pe.imports.iter().map(|s| s.to_ascii_lowercase()).collect();
    if !pe.is_dll || imports != BTreeSet::from(["kernel32.dll".into(), "psapi.dll".into()])
        || !pe.delay_imports.is_empty() || !pe.forwarders.is_empty()
        || ["__kmpc_fork_call", "__kmpc_global_thread_num", "omp_get_max_threads", "omp_get_num_threads"].iter()
            .any(|name| !exports.iter().any(|export| matches!(export, crate::pe::PeSymbol::Named(symbol) if symbol.as_str() == *name)))
    {
        return Err("OpenMP requires the native ARM64 runtime with its CPU API and OS-only imports".into());
    }
    let outputs = crate::controlled_build::census_outputs(&[(OUTPUT_LABEL, output_bytes)])
        .map_err(|e| e.to_string())?;
    let bytes = serde_json::to_vec(evidence).map_err(|e| e.to_string())?;
    Ok(ControlledBuildReceiptDraft {
        source: Some(source),
        inputs: Some(pinned_inputs()),
        builder: Some(builder),
        configuration: Some(build_configuration()),
        outputs: Some(outputs),
        supporting: Some(vec![SupportingArtifactRef {
            label: EVIDENCE_LABEL.into(),
            sha256: sha256_hex(&bytes),
        }]),
        ..ControlledBuildReceiptDraft::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn evidence() -> OpenMpBuildEvidence {
        let system_tools: Vec<_> = [
            "msvc/link.exe",
            "msvc/lib.exe",
            "msvc/nmake.exe",
            "sdk/rc.exe",
            "sdk/mt.exe",
            "windows/cmd.exe",
            "windows/powershell.exe",
        ]
        .into_iter()
        .map(|name| identity(name, 1, &"a".repeat(64)))
        .collect();
        let archive_tools = pinned_tools();
        let denied: BTreeSet<_> = archive_tools
            .iter()
            .chain(&system_tools)
            .map(|t| t.label.clone())
            .collect();
        OpenMpBuildEvidence {
            schema: EVIDENCE_SCHEMA.into(),
            inputs: pinned_inputs(),
            archive_tools,
            system_tools,
            network_denied_tools: denied.into_iter().collect(),
            configuration: cache_configuration(),
            development: DEVELOPMENT_LABELS
                .iter()
                .map(|label| identity(label, 1, &"b".repeat(64)))
                .collect(),
            cache_sha256: "c".repeat(64),
            build_log_sha256: "d".repeat(64),
            install_manifest_sha256: "e".repeat(64),
        }
    }

    #[test]
    fn report_refuses_architecture_stub_crt_development_and_fence_changes() {
        let good = evidence();
        good.validate().unwrap();
        for (key, value) in [
            ("CMAKE_C_COMPILER_TARGET", "x86_64-pc-windows-msvc"),
            ("CMAKE_ASM_COMPILER_TARGET", "x86_64-pc-windows-msvc"),
            ("LIBOMP_LIB_TYPE", "stubs"),
            ("CMAKE_MSVC_RUNTIME_LIBRARY", "MultiThreadedDLL"),
        ] {
            let mut bad = good.clone();
            bad.configuration.insert(key.into(), value.into());
            assert!(bad.validate().is_err(), "{key}");
        }
        let mut bad = good.clone();
        bad.network_denied_tools.pop();
        assert!(bad.validate().is_err());
        let mut bad = good.clone();
        bad.archive_tools[0].sha256 = "f".repeat(64);
        assert!(bad.validate().is_err());
        let mut bad = good.clone();
        bad.system_tools.pop();
        assert!(bad.validate().is_err());
        let mut bad = good;
        bad.development[0].label = "include/foreign.h".into();
        assert!(bad.validate().is_err());
    }

    fn dll(machine: u16, command: bool, api: bool, extra_crt: bool) -> Vec<u8> {
        use crate::pe::{FixtureSpec, ImportSpec, PeSymbolSpec};
        let mut imports = vec![
            ImportSpec {
                name: "KERNEL32.dll",
                symbols: &[],
            },
            ImportSpec {
                name: "PSAPI.DLL",
                symbols: &[],
            },
        ];
        if extra_crt {
            imports.push(ImportSpec {
                name: "VCRUNTIME140.dll",
                symbols: &[],
            });
        }
        let exports = if api {
            vec![
                PeSymbolSpec::Named("__kmpc_fork_call"),
                PeSymbolSpec::Named("__kmpc_global_thread_num"),
                PeSymbolSpec::Named("omp_get_max_threads"),
                PeSymbolSpec::Named("omp_get_num_threads"),
            ]
        } else {
            vec![PeSymbolSpec::Named("stub")]
        };
        let mut bytes = crate::pe::fixture(&FixtureSpec {
            machine,
            dll: !command,
            imports: &imports,
            exports: &exports,
            debug: None,
        });
        // Independent PE fixture coordinates. The general census fixture has
        // zero SizeOfHeaders and export bodies inside its export directory.
        // Supply valid file-backed function RVAs outside that directory for
        // the stricter dependency/forwarder parser used by production.
        bytes[0x94..0x98].copy_from_slice(&0x170u32.to_le_bytes());
        let table_rva = u32::from_le_bytes(bytes[0x18c..0x190].try_into().unwrap());
        let table = 0x170 + (table_rva - 0x1000) as usize;
        let function = 0x1000 + (bytes.len() - 0x170 - 1) as u32;
        for index in 0..exports.len() {
            bytes[table + index * 4..table + index * 4 + 4]
                .copy_from_slice(&function.to_le_bytes());
        }
        bytes
    }

    #[test]
    fn receipt_factory_refuses_foreign_pe_missing_cpu_api_and_dynamic_crt() {
        let source = source_identity(crate::provenance::Provenance {
            commit: "1".repeat(40),
            lock_sha256: "2".repeat(64),
        });
        let builder = BuilderIdentity {
            host: "fixture".into(),
            toolchain: "fixture".into(),
        };
        let evidence = evidence();
        let good = assemble_receipt_draft(
            source.clone(),
            &evidence,
            builder.clone(),
            &dll(0xaa64, false, true, false),
        )
        .unwrap();
        assert_eq!(good.outputs.unwrap()[0].census.machine, 0xaa64);
        for bytes in [
            dll(0x8664, false, true, false),
            dll(0xaa64, true, true, false),
            dll(0xaa64, false, false, false),
            dll(0xaa64, false, true, true),
        ] {
            assert!(
                assemble_receipt_draft(source.clone(), &evidence, builder.clone(), &bytes).is_err()
            );
        }
        let mut foreign = source;
        foreign.windows_dependency.revision = "3".repeat(40);
        assert!(
            assemble_receipt_draft(
                foreign,
                &evidence,
                builder,
                &dll(0xaa64, false, true, false)
            )
            .is_err()
        );
    }
}
