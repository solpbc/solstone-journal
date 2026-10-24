// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native artifact checks used before ARM Llama receipt admission.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::controlled_build::InputIdentityEntry;
use crate::digest::sha256_hex;
use crate::pe_dependencies::{self, PeDependencies};
use solstone_core_installed_payload::WindowsArchitecture;

fn dependencies(bytes: &[u8], loader: bool) -> Result<(), String> {
    let image = pe_dependencies::inspect_dependencies(bytes, WindowsArchitecture::Aarch64)?;
    if image.is_dll != loader || !image.delay_imports.is_empty() || !image.forwarders.is_empty() {
        return Err("ARM Llama requires ordinary native engine/loader imports".into());
    }
    validate_imports(&image, loader)
}

fn validate_imports(image: &PeDependencies, loader: bool) -> Result<(), String> {
    let names = image
        .imports
        .iter()
        .map(|name| pe_dependencies::dll_name(name))
        .collect::<Result<BTreeSet<_>, _>>()?;
    for name in &names {
        let common = [
            "kernel32.dll",
            "advapi32.dll",
            "vcruntime140.dll",
            "api-ms-win-crt-runtime-l1-1-0.dll",
            "api-ms-win-crt-heap-l1-1-0.dll",
            "api-ms-win-crt-convert-l1-1-0.dll",
            "api-ms-win-crt-stdio-l1-1-0.dll",
            "api-ms-win-crt-string-l1-1-0.dll",
            "api-ms-win-crt-filesystem-l1-1-0.dll",
            "api-ms-win-crt-math-l1-1-0.dll",
        ]
        .contains(&name.as_str());
        let role = if loader {
            name == "cfgmgr32.dll"
        } else {
            [
                "vulkan-1.dll",
                "libomp.dll",
                "shell32.dll",
                "ws2_32.dll",
                "msvcp140.dll",
                "api-ms-win-crt-locale-l1-1-0.dll",
                "api-ms-win-crt-math-l1-1-0.dll",
                "api-ms-win-crt-environment-l1-1-0.dll",
                "api-ms-win-crt-time-l1-1-0.dll",
                "api-ms-win-crt-utility-l1-1-0.dll",
            ]
            .contains(&name.as_str())
        };
        if !common && !role {
            return Err(format!("unaccounted ARM Llama dependency: {name}"));
        }
    }
    let required: &[&str] = if loader {
        &["kernel32.dll", "vcruntime140.dll", "cfgmgr32.dll"]
    } else {
        &[
            "kernel32.dll",
            "vcruntime140.dll",
            "msvcp140.dll",
            "libomp.dll",
            "vulkan-1.dll",
        ]
    };
    if required.iter().any(|name| !names.contains(*name)) {
        return Err("ARM Llama lacks its required native runtime imports".into());
    }
    Ok(())
}

fn word(bytes: &[u8], at: usize) -> Result<u16, String> {
    Ok(u16::from_le_bytes(
        bytes
            .get(at..at + 2)
            .ok_or("truncated ARM COFF word")?
            .try_into()
            .unwrap(),
    ))
}

fn dword(bytes: &[u8], at: usize) -> Result<u32, String> {
    Ok(u32::from_le_bytes(
        bytes
            .get(at..at + 4)
            .ok_or("truncated ARM COFF dword")?
            .try_into()
            .unwrap(),
    ))
}

fn trampoline(bytes: &[u8]) -> Result<(), String> {
    let sections = usize::from(word(bytes, 2)?);
    if word(bytes, 0)? != 0xaa64 || word(bytes, 16)? != 0 || sections == 0 || sections > 96 {
        return Err("Llama trampoline requires an ordinary ARM64 COFF object".into());
    }
    let table_end = 20 + sections * 40;
    if bytes.len() < table_end {
        return Err("truncated ARM trampoline section table".into());
    }
    let mut code = false;
    for at in (20..table_end).step_by(40) {
        let size = dword(bytes, at + 16)? as usize;
        let start = dword(bytes, at + 20)? as usize;
        let flags = dword(bytes, at + 36)?;
        if size != 0
            && (start < table_end || start.checked_add(size).is_none_or(|end| end > bytes.len()))
        {
            return Err("ARM trampoline section exceeds its object bytes".into());
        }
        if size != 0 && flags & 0x2000_0020 == 0x2000_0020 {
            code = true;
        }
    }
    if !code {
        return Err("ARM trampoline object has no executable code".into());
    }
    Ok(())
}

fn offsets(original: &[u8], replayed: &[u8]) -> Result<(), String> {
    if original != replayed {
        return Err("ARM loader offset replay differs from the build".into());
    }
    let text = std::str::from_utf8(original).map_err(|_| "ARM loader offsets are not UTF-8")?;
    let lines: Vec<_> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    let [
        "AREA    loader_structs_details, DATA,READONLY",
        definitions @ ..,
        "END",
    ] = lines.as_slice()
    else {
        return Err("ARM loader offsets lack their assembly boundaries".into());
    };
    let mut values = BTreeMap::new();
    for line in definitions {
        let parts: Vec<_> = line.split_whitespace().collect();
        if parts.len() != 3 || parts[1] != "EQU" || values.insert(parts[0], parts[2]).is_some() {
            return Err("malformed or duplicate ARM loader offset".into());
        }
    }
    let expected = BTreeMap::from([
        ("AARCH_64", "1"),
        ("VULKAN_LOADER_ERROR_BIT", "8"),
        ("PTR_SIZE", "8"),
        ("CHAR_PTR_SIZE", "8"),
        ("FUNCTION_OFFSET_INSTANCE", "2944"),
        ("PHYS_DEV_OFFSET_INST_DISPATCH", "808"),
        ("PHYS_DEV_OFFSET_PHYS_DEV_TRAMP", "24"),
        ("ICD_TERM_OFFSET_PHYS_DEV_TERM", "8"),
        ("PHYS_DEV_OFFSET_PHYS_DEV_TERM", "16"),
        ("INSTANCE_OFFSET_ICD_TERM", "8"),
        ("DISPATCH_OFFSET_ICD_TERM", "832"),
        ("EXT_OFFSET_DEVICE_DISPATCH", "5624"),
    ]);
    if values != expected {
        return Err("ARM loader offsets differ from the pinned source layout".into());
    }
    Ok(())
}

/// These are artifact facts, not a controlled-build receipt. The caller still
/// binds the source, tools, OpenMP cohort, commands and complete native capture.
pub fn inspect_artifacts(
    engine: &[u8],
    loader: &[u8],
    shader: &[u8],
    object: &[u8],
    original_offsets: &[u8],
    replayed_offsets: &[u8],
) -> Result<Vec<InputIdentityEntry>, String> {
    dependencies(engine, false)?;
    dependencies(loader, true)?;
    let shader_info = pe_dependencies::inspect_dependencies(shader, WindowsArchitecture::X86_64)?;
    if shader_info.is_dll {
        return Err("Llama shader generator must be an x64 executable".into());
    }
    trampoline(object)?;
    offsets(original_offsets, replayed_offsets)?;
    Ok([
        ("bin/llama-server.exe", engine),
        ("bin/vulkan-1.dll", loader),
        ("host/vulkan-shaders-gen.exe", shader),
        ("proof/unknown_ext_chain_marmasm.obj", object),
        ("proof/gen_defines.asm", original_offsets),
        ("proof/offsets-replayed.asm", replayed_offsets),
    ]
    .into_iter()
    .map(|(label, bytes)| InputIdentityEntry {
        label: label.into(),
        size: bytes.len() as u64,
        sha256: sha256_hex(bytes),
    })
    .collect())
}

pub fn run_cli(args: &[String]) -> Result<String, String> {
    const USAGE: &str = "llama-windows-arm64 inspect-artifacts --engine ABS --loader ABS --shader-host ABS --trampoline ABS --offsets ABS --replayed-offsets ABS";
    let Some((operation, rest)) = args.split_first() else {
        return Err(USAGE.into());
    };
    if operation != "inspect-artifacts" || rest.len() != 12 {
        return Err(USAGE.into());
    }
    let mut flags = BTreeMap::new();
    for pair in rest.chunks_exact(2) {
        if ![
            "--engine",
            "--loader",
            "--shader-host",
            "--trampoline",
            "--offsets",
            "--replayed-offsets",
        ]
        .contains(&pair[0].as_str())
            || !Path::new(&pair[1]).is_absolute()
            || flags.insert(pair[0].as_str(), pair[1].as_str()).is_some()
        {
            return Err("unknown, duplicate or non-absolute ARM Llama artifact flag".into());
        }
    }
    let read = |key: &str, bound| super::windows_build::read_bounded(Path::new(flags[key]), bound);
    let facts = inspect_artifacts(
        &read("--engine", 128 * 1024 * 1024)?,
        &read("--loader", 16 * 1024 * 1024)?,
        &read("--shader-host", 16 * 1024 * 1024)?,
        &read("--trampoline", 4 * 1024 * 1024)?,
        &read("--offsets", 64 * 1024)?,
        &read("--replayed-offsets", 64 * 1024)?,
    )?;
    serde_json::to_string(&serde_json::json!({
        "schema": "solstone.llama-windows-arm64-artifact-inspection.v1",
        "artifacts": facts, "controlled_receipt_admitted": false,
        "native_arm_execution": false,
    }))
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(loader: bool) -> PeDependencies {
        PeDependencies {
            is_dll: loader,
            imports: if loader {
                vec!["KERNEL32.dll", "VCRUNTIME140.dll", "CFGMGR32.dll"]
            } else {
                vec![
                    "KERNEL32.dll",
                    "VCRUNTIME140.dll",
                    "MSVCP140.dll",
                    "libomp.dll",
                    "vulkan-1.dll",
                ]
            }
            .into_iter()
            .map(str::to_owned)
            .collect(),
            delay_imports: vec![],
            forwarders: vec![],
        }
    }

    #[test]
    fn native_openmp_and_loader_imports_are_required_and_foreign_backends_refused() {
        for loader in [false, true] {
            let good = image(loader);
            validate_imports(&good, loader).unwrap();
            let mut with_crt_math = good.clone();
            with_crt_math
                .imports
                .push("api-ms-win-crt-math-l1-1-0.dll".into());
            validate_imports(&with_crt_math, loader).unwrap();
            for index in 0..good.imports.len() {
                let mut bad = good.clone();
                bad.imports.remove(index);
                assert!(validate_imports(&bad, loader).is_err());
            }
            for foreign in [
                "vcomp140.dll",
                "vcruntime140_1.dll",
                "nvoglv64.dll",
                "ggml-cuda.dll",
                "libompd.dll",
            ] {
                let mut bad = good.clone();
                bad.imports.push(foreign.into());
                assert!(validate_imports(&bad, loader).is_err());
            }
        }
    }

    #[test]
    fn output_architecture_does_not_come_from_the_filename() {
        let mut bytes = crate::pe_dependencies::tests::with_import(false);
        for machine in [0x8664u16, 0xa641, 0xa64e, 0x14c] {
            bytes[0x84..0x86].copy_from_slice(&machine.to_le_bytes());
            assert!(dependencies(&bytes, true).is_err());
            assert!(dependencies(&bytes, false).is_err());
        }
    }

    #[test]
    fn trampoline_requires_native_code_and_bounded_section_bytes() {
        let mut good = vec![0; 64];
        good[..2].copy_from_slice(&0xaa64u16.to_le_bytes());
        good[2..4].copy_from_slice(&1u16.to_le_bytes());
        good[36..40].copy_from_slice(&4u32.to_le_bytes());
        good[40..44].copy_from_slice(&60u32.to_le_bytes());
        good[56..60].copy_from_slice(&0x6000_0020u32.to_le_bytes());
        trampoline(&good).unwrap();
        for machine in [0x8664u16, 0xa641, 0xa64e, 0x14c] {
            let mut bad = good.clone();
            bad[..2].copy_from_slice(&machine.to_le_bytes());
            assert!(trampoline(&bad).is_err());
        }
        for length in [0, 19, 59, 63] {
            assert!(trampoline(&good[..length]).is_err());
        }
        let mut bad = good;
        bad[56..60].fill(0);
        assert!(trampoline(&bad).is_err());
    }

    #[test]
    fn offsets_require_the_pinned_arm_layout_and_identical_replay() {
        let good = include_bytes!("../../tests/fixtures/llama-windows-arm64/loader-offsets.asm");
        offsets(good, good).unwrap();
        let text = std::str::from_utf8(good).unwrap();
        for line in text.lines().filter(|line| line.contains(" EQU ")) {
            let mut parts: Vec<_> = line.split_whitespace().collect();
            parts[2] = "999";
            let bad = text.replace(line, &parts.join(" "));
            assert!(offsets(bad.as_bytes(), bad.as_bytes()).is_err());
        }
        let duplicate = format!("{text}\nPTR_SIZE EQU 8\n");
        assert!(offsets(duplicate.as_bytes(), duplicate.as_bytes()).is_err());
        for omitted in ["AREA    loader_structs_details, DATA,READONLY", "END"] {
            let bad = text.replace(omitted, "");
            assert!(offsets(bad.as_bytes(), bad.as_bytes()).is_err());
        }
        assert!(
            offsets(
                good,
                text.replace("AARCH_64 EQU 1", "AARCH_64 EQU 0").as_bytes()
            )
            .is_err()
        );
    }

    #[test]
    fn artifact_cli_refuses_missing_duplicate_and_relative_inputs() {
        assert!(run_cli(&[]).is_err());
        assert!(run_cli(&["record".into()]).is_err());
        let mut args = vec!["inspect-artifacts".into()];
        for key in [
            "--engine",
            "--loader",
            "--shader-host",
            "--trampoline",
            "--offsets",
            "--replayed-offsets",
        ] {
            args.extend([key.into(), "relative".into()]);
        }
        assert!(run_cli(&args).is_err());
        #[cfg(unix)]
        let absolute = "/capture";
        #[cfg(windows)]
        let absolute = "C:/capture";
        for value in args.iter_mut().skip(2).step_by(2) {
            *value = absolute.into();
        }
        args[11] = args[1].clone();
        assert!(run_cli(&args).unwrap_err().contains("duplicate"));
    }
}
