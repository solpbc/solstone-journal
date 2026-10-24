// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Reviewed generated compiler configuration for the pinned ARM Llama recipe.
//! This complements cache and artifact checks; it does not admit a receipt.

use std::collections::BTreeMap;

use crate::digest::sha256_hex;
use crate::windows_clang::windows_path;

fn replace_root(text: &str, prefix: &str) -> String {
    let folded = text.to_ascii_lowercase();
    let prefix = prefix.to_ascii_lowercase();
    let mut result = String::new();
    let mut at = 0;
    while let Some(next) = folded[at..].find(&prefix) {
        let next = at + next;
        result.push_str(&text[at..next]);
        result.push_str("${WORK}");
        at = next + prefix.len();
    }
    result.push_str(&text[at..]);
    result
}

fn normalized(
    bytes: &[u8],
    work_root: &str,
    guids: &mut BTreeMap<String, usize>,
) -> Result<Vec<u8>, String> {
    let mut text = std::str::from_utf8(bytes)
        .map_err(|_| "Llama generated configuration is not UTF-8")?
        .to_owned();
    if text.contains("${WORK}") || text.contains("${GUID_") {
        return Err("Llama generated configuration contains reserved comparison tokens".into());
    }
    windows_path(work_root)?;
    let forward = work_root.replace('\\', "/");
    let backward = forward.replace('/', "\\");
    // Replace only the caller's bound root, including CMake's escaped form.
    // Preserve all remaining path separators, compiler flags and quoting.
    for prefix in [backward.replace('\\', "\\\\"), backward, forward] {
        text = replace_root(&text, &prefix);
    }
    let bytes = text.as_bytes();
    let mut result = String::new();
    let mut last = 0;
    let mut at = 0;
    while at + 38 <= bytes.len() {
        let token = &bytes[at..at + 38];
        let guid = token[0] == b'{'
            && token[37] == b'}'
            && (1..37).all(|n| {
                if [9, 14, 19, 24].contains(&n) {
                    token[n] == b'-'
                } else {
                    token[n].is_ascii_hexdigit()
                }
            });
        if guid {
            result.push_str(&text[last..at]);
            let key = text[at..at + 38].to_ascii_lowercase();
            let next = guids.len();
            let number = *guids.entry(key).or_insert(next);
            result.push_str(&format!("${{GUID_{number}}}"));
            at += 38;
            last = at;
        } else {
            at += 1;
        }
    }
    result.push_str(&text[last..]);
    Ok(result.into_bytes())
}

pub fn reviewed_members() -> Vec<String> {
    let manifest: BTreeMap<String, String> =
        serde_json::from_slice(include_bytes!("llama_windows_arm64_generated.json"))
            .expect("reviewed ARM Llama generated configuration");
    manifest.into_keys().collect()
}

/// Every reviewed member is required. Root relocation and consistent GUID
/// renaming preserve the generated project graph; all other bytes stay closed.
pub fn validate_generated_configuration(
    files: &BTreeMap<String, Vec<u8>>,
    work_root: &str,
) -> Result<(), String> {
    let expected: BTreeMap<String, String> =
        serde_json::from_slice(include_bytes!("llama_windows_arm64_generated.json"))
            .expect("reviewed ARM Llama generated configuration");
    if !files.keys().eq(expected.keys()) {
        return Err("Llama generated compiler configuration membership differs".into());
    }
    let mut guids = BTreeMap::new();
    for (name, bytes) in files {
        if sha256_hex(&normalized(bytes, work_root, &mut guids)?) != expected[name] {
            return Err(format!(
                "Llama generated compiler configuration differs: {name}"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn original() -> BTreeMap<String, String> {
        serde_json::from_slice(include_bytes!(
            "../tests/fixtures/llama-windows-arm64/generated-configuration.json"
        ))
        .unwrap()
    }

    fn relocated(root: &str) -> BTreeMap<String, Vec<u8>> {
        original()
            .into_iter()
            .map(|(name, mut text)| {
                for (old, suffix) in [
                    (
                        "C:/llama-arm-spike-a2/llvm-host/clang+llvm-22.1.6-x86_64-pc-windows-msvc",
                        "/llvm",
                    ),
                    (
                        "C:/llama-arm-spike-a2/openmp-original/development",
                        "/openmp",
                    ),
                    (
                        "C:/llama-arm-spike-a2/cmake-host/cmake-3.31.12-windows-x86_64",
                        "/cmake",
                    ),
                    ("C:/llama-arm-spike-a2/python-host", "/python"),
                    ("C:/llama-arm-spike-a2/source", "/source"),
                    ("C:/llama-arm-spike-a2/sdk", "/sdk"),
                    ("C:/llama-arm-spike-a4", ""),
                ] {
                    let replacement = format!("{root}{suffix}");
                    text = text.replace(
                        &old.replace('/', "\\").replace('\\', "\\\\"),
                        &replacement.replace('/', "\\").replace('\\', "\\\\"),
                    );
                    text = text.replace(&old.replace('/', "\\"), &replacement.replace('/', "\\"));
                    text = text.replace(old, &replacement);
                }
                (name, text.into_bytes())
            })
            .collect()
    }

    #[test]
    fn original_native_compiler_configuration_relocates_without_changing_flags() {
        for root in ["C:/capture", "D:/fresh build/llama"] {
            let files = relocated(root);
            validate_generated_configuration(&files, root).unwrap();
            assert!(validate_generated_configuration(&files, "C:/foreign").is_err());
        }
        let files = original();
        let cpu = &files["engine-build/ggml/src/CMakeFiles/ggml-cpu.dir/flags.make"];
        for required in ["-march=armv8-a", "/openmp", "-MD", "-DGGML_USE_OPENMP"] {
            assert!(cpu.contains(required));
        }
        assert!(!cpu.contains("/arch:AVX"));
        let loader = &files["loader-build/upstream/loader/vulkan.vcxproj"];
        assert!(loader.contains("Release|ARM64") && loader.contains("/machine:ARM64"));
    }

    #[test]
    fn missing_extra_or_changed_generated_compiler_configuration_is_refused() {
        let good = relocated("C:/capture");
        for name in good.keys() {
            let mut bad = good.clone();
            bad.remove(name);
            assert!(validate_generated_configuration(&bad, "C:/capture").is_err());
            let mut bad = good.clone();
            bad.get_mut(name).unwrap().extend_from_slice(b"foreign");
            assert!(validate_generated_configuration(&bad, "C:/capture").is_err());
        }
        let mut bad = good;
        bad.insert("engine-build/foreign.flags".into(), vec![]);
        assert!(validate_generated_configuration(&bad, "C:/capture").is_err());
    }

    #[test]
    fn guid_renaming_preserves_identity_relationships_and_compiler_quoting() {
        let one = b"{355D7DA1-2747-32B6-A031-E008F7EFAA18}";
        let other = b"{6111C22E-943C-373B-9FBD-ABCE6AE46CFA}";
        let mut guids = BTreeMap::new();
        assert_eq!(
            normalized(one, "C:/capture", &mut guids).unwrap(),
            b"${GUID_0}"
        );
        assert_eq!(
            normalized(one, "C:/capture", &mut guids).unwrap(),
            b"${GUID_0}"
        );
        assert_eq!(
            normalized(other, "C:/capture", &mut guids).unwrap(),
            b"${GUID_1}"
        );
        let quoted = b"-DMACRO=\\\"value\\\" /MD C:\\capture\\source";
        assert_eq!(
            normalized(quoted, "C:/capture", &mut guids).unwrap(),
            b"-DMACRO=\\\"value\\\" /MD ${WORK}\\source"
        );
        for reserved in [b"${WORK}/source".as_slice(), b"${GUID_0}".as_slice()] {
            assert!(normalized(reserved, "C:/capture", &mut guids).is_err());
        }
    }
}
