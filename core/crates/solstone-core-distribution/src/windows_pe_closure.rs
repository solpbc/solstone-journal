// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Static dependency closure of the producer's declared PE bytes.
//!
//! This remains a static closure, not a loaded-module census. CED, ONNX
//! Runtime, the Vulkan loader, and PDFium are DllLoadDir images. Do not claim
//! loaded-module precedence. Directory resolution follows the existing
//! win-dll-load policies, never an ambient PATH or an arbitrary matching
//! package basename.
//! https://learn.microsoft.com/en-us/windows/win32/dlls/dynamic-link-library-search-order

use std::collections::BTreeMap;

use crate::inventory::is_allowed_msvc_repetition;
use crate::pe_dependencies::{PeDependencies, inspect_dependencies};
use crate::windows_payload::{
    WINDOWS_CED_LIBRARY, WINDOWS_ONNXRUNTIME_LIBRARY, WINDOWS_PDFIUM_LIBRARY, WINDOWS_VULKAN_LOADER,
};

// Explicit Win10/11 system contracts observed in the admitted native inputs.
// An API-set prefix is not admission. New names require an observed import and
// platform qualification; CRT/OpenMP/vendor DLLs remain package dependencies.
const SYSTEM_DLLS: &[&str] = &[
    "advapi32.dll",
    "bcrypt.dll",
    "bcryptprimitives.dll",
    // Configuration Manager device-node APIs imported by the source-built
    // Vulkan loader. Part of supported Windows; never supplied by the payload.
    // https://learn.microsoft.com/en-us/windows/win32/api/cfgmgr32/
    "cfgmgr32.dll",
    "cldapi.dll",
    "combase.dll",
    // Common controls, imported by the journal app's window and its folder
    // picker through the shell and the WebView2 loader. Part of every
    // supported Windows; never supplied by the payload.
    "comctl32.dll",
    // Crypto API, imported by nvattest.exe. Part of every supported Windows;
    // never supplied by the payload.
    "crypt32.dll",
    "dbghelp.dll",
    // DWM, for the journal app's native caption appearance. Present on both
    // supported Windows versions; newer caption attributes remain optional.
    // https://learn.microsoft.com/en-us/windows/win32/api/dwmapi/nf-dwmapi-dwmsetwindowattribute
    "dwmapi.dll",
    "dxgi.dll",
    "gdi32.dll",
    // IP Helper. The journal's pair-link discovery calls
    // `GetAdaptersAddresses` here, the way the Unix build calls `getifaddrs`;
    // it is a system DLL on every supported Windows, alongside ws2_32 below.
    "iphlpapi.dll",
    "kernel32.dll",
    "ntdll.dll",
    // COM, for the journal app's folder picker, Start-menu shortcut and
    // WebView2 window. A system DLL on every supported Windows.
    "ole32.dll",
    "oleaut32.dll",
    "secur32.dll",
    "setupapi.dll",
    "shell32.dll",
    // Shell light-weight utilities: the in-memory stream the journal app
    // serves its own pages from. A system DLL on every supported Windows.
    "shlwapi.dll",
    "user32.dll",
    "userenv.dll",
    "ws2_32.dll",
    "api-ms-win-core-path-l1-1-0.dll",
    "api-ms-win-core-synch-l1-2-0.dll",
    "api-ms-win-core-winrt-l1-1-0.dll",
    "api-ms-win-crt-convert-l1-1-0.dll",
    "api-ms-win-crt-environment-l1-1-0.dll",
    "api-ms-win-crt-filesystem-l1-1-0.dll",
    "api-ms-win-crt-heap-l1-1-0.dll",
    "api-ms-win-crt-locale-l1-1-0.dll",
    "api-ms-win-crt-math-l1-1-0.dll",
    "api-ms-win-crt-process-l1-1-0.dll",
    "api-ms-win-crt-runtime-l1-1-0.dll",
    "api-ms-win-crt-stdio-l1-1-0.dll",
    "api-ms-win-crt-string-l1-1-0.dll",
    "api-ms-win-crt-time-l1-1-0.dll",
    "api-ms-win-crt-utility-l1-1-0.dll",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeEdge {
    pub importer: String,
    pub kind: &'static str,
    pub library: String,
    /// None denotes an explicit OS contract, not an inferred system path.
    pub member: Option<String>,
}

/// Inspect every declared PE, including DLLs that no other member imports.
/// The caller supplies the same retained bytes used to write the staging tree.
/// Inventory membership, source admission, and non-PE files remain its concern.
pub fn inspect_runtime_closure<'a>(
    members: impl IntoIterator<Item = (&'a str, &'a [u8])>,
) -> Result<Vec<RuntimeEdge>, String> {
    let mut images = BTreeMap::new();
    let mut basenames: BTreeMap<String, Vec<(&str, &[u8])>> = BTreeMap::new();
    for (path, bytes) in members {
        if !path.is_ascii()
            || path.contains('\\')
            || path.split('/').any(|p| {
                p.is_empty()
                    || matches!(p, "." | "..")
                    || p.ends_with([' ', '.'])
                    || p.contains(':')
            })
        {
            return Err(format!("invalid declared PE path: {path:?}"));
        }
        let lower = path.to_ascii_lowercase();
        let (_, basename) = lower
            .rsplit_once('/')
            .ok_or_else(|| format!("PE must have a declared directory: {path}"))?;
        if SYSTEM_DLLS.contains(&basename) {
            return Err(format!("package PE shadows a system contract: {path}"));
        }
        let seen = basenames.entry(basename.to_owned()).or_default();
        seen.push((path, bytes));
        match seen.len() {
            1 => {}
            2 => {
                let (prior_path, prior_bytes) = seen[0];
                if is_allowed_msvc_repetition(basename, prior_path, path) {
                    if prior_bytes != bytes {
                        return Err(format!(
                            "repeated private CRT bytes differ: {prior_path}, {path}"
                        ));
                    }
                } else {
                    return Err(format!("case-colliding PE basenames: {prior_path}, {path}"));
                }
            }
            _ => {
                let (prior_path, _) = seen[0];
                return Err(format!("case-colliding PE basenames: {prior_path}, {path}"));
            }
        }
        let info = inspect_dependencies(bytes).map_err(|e| format!("{path}: {e}"))?;
        if (basename.ends_with(".dll") && !info.is_dll)
            || (basename.ends_with(".exe") && info.is_dll)
            || !(basename.ends_with(".dll") || basename.ends_with(".exe"))
        {
            return Err(format!("PE kind does not match declared extension: {path}"));
        }
        images.insert(lower, info);
    }
    inspect_edges(&images)
}

fn search_directory(path: &str, is_dll: bool) -> Result<&str, String> {
    let (parent, _) = path.rsplit_once('/').ok_or("PE has no package directory")?;
    if parent == "bin" {
        return Ok("bin");
    }
    let pdf_dir = WINDOWS_PDFIUM_LIBRARY.rsplit_once('/').unwrap().0;
    if is_dll && parent == pdf_dir {
        return Ok(parent);
    }
    if parent.starts_with("lib/solstone-") && !parent["lib/solstone-".len()..].contains('/') {
        return Ok(parent);
    }
    Err(format!(
        "no existing loader policy admits PE placement: {path}"
    ))
}

fn inspect_edges(images: &BTreeMap<String, PeDependencies>) -> Result<Vec<RuntimeEdge>, String> {
    if images.is_empty() {
        return Err("Windows payload has no declared PE images".into());
    }
    let mut edges = Vec::new();
    for (path, info) in images {
        let directory = search_directory(path, info.is_dll)?;
        for (kind, names) in [
            ("import", &info.imports),
            ("delay-import", &info.delay_imports),
            ("forwarder", &info.forwarders),
        ] {
            for name in names {
                let library = crate::pe_dependencies::dll_name(name)?;
                if (path == WINDOWS_CED_LIBRARY
                    || path == WINDOWS_ONNXRUNTIME_LIBRARY
                    || path == WINDOWS_VULKAN_LOADER
                    || path == WINDOWS_PDFIUM_LIBRARY)
                    && (kind == "delay-import" || kind == "forwarder")
                    && !SYSTEM_DLLS.contains(&library.as_str())
                {
                    return Err(format!(
                        "{path}: unsupported {kind} {library} under DllLoadDir"
                    ));
                }
                let member = if SYSTEM_DLLS.contains(&library.as_str()) {
                    None
                } else {
                    let member = format!("{directory}/{library}");
                    if !images.get(&member).is_some_and(|image| image.is_dll) {
                        return Err(format!(
                            "{path}: unresolved {kind} {library} in {directory}"
                        ));
                    }
                    Some(member)
                };
                edges.push(RuntimeEdge {
                    importer: path.clone(),
                    kind,
                    library,
                    member,
                });
            }
        }
    }
    Ok(edges)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(
        is_dll: bool,
        ordinary: &[&str],
        delayed: &[&str],
        forwarded: &[&str],
    ) -> PeDependencies {
        PeDependencies {
            is_dll,
            imports: ordinary.iter().map(|s| (*s).into()).collect(),
            delay_imports: delayed.iter().map(|s| (*s).into()).collect(),
            forwarders: forwarded.iter().map(|s| (*s).into()).collect(),
        }
    }

    #[test]
    fn every_dependency_kind_requires_a_declared_dll() {
        for kind in 0..3 {
            let mut dependencies = [Vec::new(), Vec::new(), Vec::new()];
            dependencies[kind].push("engine.dll");
            let mut images = BTreeMap::from([(
                "bin/app.exe".into(),
                image(false, &dependencies[0], &dependencies[1], &dependencies[2]),
            )]);
            assert!(inspect_edges(&images).unwrap_err().contains("unresolved"));
            images.insert(
                "bin/engine.dll".into(),
                image(true, &["kernel32.dll"], &[], &[]),
            );
            let edges = inspect_edges(&images).unwrap();
            assert!(
                edges
                    .iter()
                    .any(|e| e.member.as_deref() == Some("bin/engine.dll"))
            );
            // An executable with the same key is not a DLL closure.
            images.get_mut("bin/engine.dll").unwrap().is_dll = false;
            assert!(inspect_edges(&images).is_err());
        }
    }

    #[test]
    fn transitive_and_unreferenced_dlls_are_checked_without_recursion() {
        let mut images = BTreeMap::from([
            ("bin/app.exe".into(), image(false, &["a.dll"], &[], &[])),
            ("bin/a.dll".into(), image(true, &["b.dll"], &[], &[])),
            ("bin/b.dll".into(), image(true, &["a.dll"], &[], &[])),
        ]);
        assert!(inspect_edges(&images).is_ok()); // A cycle is finite because each PE is inspected once.
        images.insert(
            "bin/unused.dll".into(),
            image(true, &["missing.dll"], &[], &[]),
        );
        assert!(inspect_edges(&images).unwrap_err().contains("unused.dll"));
    }

    #[test]
    fn private_library_policies_do_not_search_arbitrary_package_directories() {
        let mut images = BTreeMap::from([
            (
                WINDOWS_ONNXRUNTIME_LIBRARY.into(),
                image(true, &["vcruntime140.dll"], &[], &[]),
            ),
            (
                "lib/solstone-native/vcruntime140.dll".into(),
                image(true, &["kernel32.dll"], &[], &[]),
            ),
        ]);
        assert!(inspect_edges(&images).is_ok());
        images.insert(
            WINDOWS_PDFIUM_LIBRARY.into(),
            image(true, &["vcruntime140.dll"], &[], &[]),
        );
        assert!(
            inspect_edges(&images)
                .unwrap_err()
                .contains("lib/solstone-core-pdf")
        );
        images.remove("lib/solstone-native/vcruntime140.dll");
        images.insert(
            "lib/elsewhere/vcruntime140.dll".into(),
            image(true, &[], &[], &[]),
        );
        assert!(inspect_edges(&images).is_err());
    }

    #[test]
    fn vulkan_loader_is_app_local_while_configuration_manager_is_system() {
        let mut images = BTreeMap::from([
            (
                "lib/solstone-native/llama-server.exe".into(),
                image(false, &["vulkan-1.dll"], &[], &[]),
            ),
            (
                "lib/solstone-native/vulkan-1.dll".into(),
                image(true, &["cfgmgr32.dll", "vcruntime140.dll"], &[], &[]),
            ),
            (
                "lib/solstone-native/vcruntime140.dll".into(),
                image(true, &["kernel32.dll"], &[], &[]),
            ),
        ]);
        let edges = inspect_edges(&images).unwrap();
        assert!(
            edges
                .iter()
                .any(|e| e.member.as_deref() == Some("lib/solstone-native/vulkan-1.dll"))
        );
        assert!(
            edges
                .iter()
                .any(|e| e.library == "cfgmgr32.dll" && e.member.is_none())
        );
        images.remove("lib/solstone-native/vulkan-1.dll");
        assert!(inspect_edges(&images).is_err());
    }

    #[test]
    fn unknown_api_sets_and_vendor_runtime_names_are_never_system() {
        for name in [
            "api-ms-win-invented-l1-1-0.dll",
            "ext-ms-win-invented-l1-1-0.dll",
            "vcruntime140.dll",
            "vcomp140.dll",
            "opencl.dll",
            "vulkan-1.dll",
            "directml.dll",
        ] {
            let images = BTreeMap::from([("bin/app.exe".into(), image(false, &[name], &[], &[]))]);
            assert!(
                inspect_edges(&images).unwrap_err().contains("unresolved"),
                "{name}"
            );
        }
    }

    #[test]
    fn malformed_image_os_shadow_and_duplicate_basename_refuse() {
        assert!(inspect_runtime_closure([("bin/app.exe", b"MZ".as_slice())]).is_err());
        assert!(
            inspect_runtime_closure([("bin/kernel32.dll", b"MZ".as_slice())])
                .unwrap_err()
                .contains("shadows")
        );
        let bytes = crate::pe_dependencies::tests::image();
        assert!(
            inspect_runtime_closure([
                ("bin/worker.dll", bytes.as_slice()),
                ("lib/elsewhere/WORKER.DLL", bytes.as_slice())
            ])
            .unwrap_err()
            .contains("case-colliding")
        );
        assert!(inspect_runtime_closure([("bin/worker.dll", bytes.as_slice())]).is_ok());
        assert!(
            inspect_runtime_closure([("bin/worker.exe", bytes.as_slice())])
                .unwrap_err()
                .contains("kind")
        );
    }

    #[test]
    fn component_directory_searches_itself() {
        let tool = "lib/solstone-x/tool.exe";
        let images = BTreeMap::from([(tool.into(), image(false, &["kernel32.dll"], &[], &[]))]);
        assert!(inspect_edges(&images).is_ok());

        let images_bad = BTreeMap::from([(tool.into(), image(false, &["custom.dll"], &[], &[]))]);
        assert!(inspect_edges(&images_bad).is_err());
    }

    #[test]
    fn identical_byte_crt_pair_and_edge_resolution() {
        let bytes = crate::pe_dependencies::tests::image();
        let closure = inspect_runtime_closure([
            ("lib/solstone-native/vcruntime140.dll", bytes.as_slice()),
            ("lib/solstone-nvattest/vcruntime140.dll", bytes.as_slice()),
        ])
        .unwrap();
        assert!(closure.is_empty());

        let images = BTreeMap::from([
            (
                "lib/solstone-native/llama-server.exe".into(),
                image(false, &["vcruntime140.dll"], &[], &[]),
            ),
            (
                "lib/solstone-native/vcruntime140.dll".into(),
                image(true, &["kernel32.dll"], &[], &[]),
            ),
            (
                "lib/solstone-nvattest/vcruntime140.dll".into(),
                image(true, &["kernel32.dll"], &[], &[]),
            ),
        ]);
        let edges = inspect_edges(&images).unwrap();
        assert!(edges.iter().any(|e| {
            e.importer == "lib/solstone-native/llama-server.exe"
                && e.member.as_deref() == Some("lib/solstone-native/vcruntime140.dll")
        }));
    }

    #[test]
    fn differing_crt_bytes_refuse() {
        let bytes = crate::pe_dependencies::tests::image();
        let mut diff_bytes = bytes.clone();
        diff_bytes.push(0);
        let err = inspect_runtime_closure([
            ("lib/solstone-native/vcruntime140.dll", bytes.as_slice()),
            (
                "lib/solstone-nvattest/vcruntime140.dll",
                diff_bytes.as_slice(),
            ),
        ])
        .unwrap_err();
        assert_eq!(
            err,
            "repeated private CRT bytes differ: lib/solstone-native/vcruntime140.dll, lib/solstone-nvattest/vcruntime140.dll"
        );
    }

    #[test]
    fn duplicate_basename_in_other_private_directory_refuses() {
        let bytes = crate::pe_dependencies::tests::image();
        let err = inspect_runtime_closure([
            ("lib/solstone-native/vcruntime140.dll", bytes.as_slice()),
            ("lib/solstone-other/vcruntime140.dll", bytes.as_slice()),
        ])
        .unwrap_err();
        assert_eq!(
            err,
            "case-colliding PE basenames: lib/solstone-native/vcruntime140.dll, lib/solstone-other/vcruntime140.dll"
        );
    }

    #[test]
    fn third_path_crt_collision_refuses() {
        let bytes = crate::pe_dependencies::tests::image();
        let err = inspect_runtime_closure([
            ("lib/solstone-native/vcruntime140.dll", bytes.as_slice()),
            ("lib/solstone-nvattest/vcruntime140.dll", bytes.as_slice()),
            ("lib/solstone-other/vcruntime140.dll", bytes.as_slice()),
        ])
        .unwrap_err();
        assert_eq!(
            err,
            "case-colliding PE basenames: lib/solstone-native/vcruntime140.dll, lib/solstone-other/vcruntime140.dll"
        );
    }

    #[test]
    fn dllloaddir_refuses_non_system_delay_import_even_if_sibling_present() {
        let images = BTreeMap::from([
            (
                "lib/solstone-native/ced.dll".into(),
                image(true, &[], &["sibling.dll"], &[]),
            ),
            (
                "lib/solstone-native/sibling.dll".into(),
                image(true, &[], &[], &[]),
            ),
        ]);
        let err = inspect_edges(&images).unwrap_err();
        assert_eq!(
            err,
            "lib/solstone-native/ced.dll: unsupported delay-import sibling.dll under DllLoadDir"
        );
    }

    #[test]
    fn private_exe_resolves_sibling_delay_import() {
        let images = BTreeMap::from([
            (
                "lib/solstone-native/llama-server.exe".into(),
                image(false, &[], &["sibling.dll"], &[]),
            ),
            (
                "lib/solstone-native/sibling.dll".into(),
                image(true, &["kernel32.dll"], &[], &[]),
            ),
        ]);
        let edges = inspect_edges(&images).unwrap();
        assert!(edges.iter().any(|e| {
            e.importer == "lib/solstone-native/llama-server.exe"
                && e.kind == "delay-import"
                && e.member.as_deref() == Some("lib/solstone-native/sibling.dll")
        }));
    }
}
