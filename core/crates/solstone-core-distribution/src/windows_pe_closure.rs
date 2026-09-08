// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Static dependency closure of the producer's declared PE bytes.
//!
//! This is not a loaded-module or dynamic LoadLibrary census. Runtime feature
//! tests still establish the actual loaded paths. Directory resolution follows
//! the existing win-dll-load policies (ApplicationDir for ORT/CED, DllLoadDir
//! for PDFium), never an ambient PATH or an arbitrary matching package basename.
//! https://learn.microsoft.com/en-us/windows/win32/dlls/dynamic-link-library-search-order

use std::collections::BTreeMap;

use crate::pe_dependencies::{PeDependencies, inspect_dependencies};
use crate::windows_payload::{WINDOWS_ONNXRUNTIME_LIBRARY, WINDOWS_PDFIUM_LIBRARY};

// Explicit Win10/11 system contracts observed in the admitted native inputs.
// An API-set prefix is not admission. New names require an observed import and
// platform qualification; CRT/OpenMP/vendor DLLs remain package dependencies.
const SYSTEM_DLLS: &[&str] = &[
    "advapi32.dll",
    "dbghelp.dll",
    "dxgi.dll",
    "gdi32.dll",
    "kernel32.dll",
    "setupapi.dll",
    "user32.dll",
    "ws2_32.dll",
    "api-ms-win-core-path-l1-1-0.dll",
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
    let mut basenames = BTreeMap::new();
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
        if let Some(prior) = basenames.insert(basename.to_owned(), path.to_owned()) {
            return Err(format!("case-colliding PE basenames: {prior}, {path}"));
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
    if parent == "bin" || path == WINDOWS_ONNXRUNTIME_LIBRARY {
        return Ok("bin");
    }
    let pdf_dir = WINDOWS_PDFIUM_LIBRARY.rsplit_once('/').unwrap().0;
    if is_dll && parent == pdf_dir {
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
                "bin/vcruntime140.dll".into(),
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
        images.remove("bin/vcruntime140.dll");
        images.insert(
            "lib/elsewhere/vcruntime140.dll".into(),
            image(true, &[], &[], &[]),
        );
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
}
