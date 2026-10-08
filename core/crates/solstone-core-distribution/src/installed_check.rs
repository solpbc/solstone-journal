// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only check of one installed package root.
//!
//! The root is the operator's subject. This command does not select a root
//! for a running journal and does not write.

use std::path::Path;

use solstone_core_installed_payload::{InstalledPayloadRefusal, verify_installed_package};

pub fn check_installed(
    root: &Path,
    version: &str,
    target: &str,
) -> Result<(), InstalledPayloadRefusal> {
    verify_installed_package(root, version, target)
}

#[must_use]
pub fn refusal_line(refusal: &InstalledPayloadRefusal) -> String {
    match &refusal.path {
        Some(path) => format!("{} {path}\n", refusal.code),
        None => format!("{}\n", refusal.code),
    }
}

#[must_use]
pub fn admitted_line() -> &'static str {
    "admitted\n"
}

#[cfg(test)]
mod tests {
    use super::*;
    use solstone_core_installed_payload::{COMPILED_VERSION, TARGET_LINUX_X86_64, code};

    #[test]
    fn check_installed_detects_namespace_flip() {
        let tmp = tempfile::tempdir().expect("scratch");
        let root = tmp.path();
        let payload = root.join("share/solstone-journal");
        std::fs::create_dir_all(&payload).unwrap();
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("solstone"), b"core-bin").unwrap();

        let lib_b = root.join("lib/solstone-b/lib");
        std::fs::create_dir_all(&lib_b).unwrap();
        let liby_bytes = b"\x7fELF-liby-so-body-".to_vec();
        std::fs::write(lib_b.join("liby.so"), &liby_bytes).unwrap();

        let manifest_content = serde_json::json!({
            "schema": "solstone.installed-payload.v1",
            "version": COMPILED_VERSION,
            "target": TARGET_LINUX_X86_64,
            "product": "solstone-journal",
            "source_commit": "0123456789abcdef0123456789abcdef01234567",
            "files": [
                { "path": "bin/solstone", "bytes": 8, "sha256": crate::digest::sha256_hex(b"core-bin") },
                { "path": "lib/solstone-b/lib/liby.so", "bytes": liby_bytes.len() as u64, "sha256": crate::digest::sha256_hex(&liby_bytes) }
            ]
        });
        std::fs::write(
            payload.join("installed-payload.json"),
            serde_json::to_vec(&manifest_content).unwrap(),
        )
        .unwrap();

        let mut flipped = liby_bytes.clone();
        *flipped.last_mut().unwrap() ^= 0xff;
        std::fs::write(lib_b.join("liby.so"), &flipped).unwrap();

        let err = check_installed(root, COMPILED_VERSION, TARGET_LINUX_X86_64).unwrap_err();
        assert_eq!(err.code, code::MEMBER_CHANGED);
        assert_eq!(err.path.as_deref(), Some("lib/solstone-b/lib/liby.so"));
    }
}
