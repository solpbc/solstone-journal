// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn collect(root: &Path, directory: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory).expect("evidence fixture directory reads") {
        let entry = entry.expect("fixture directory entry reads");
        let path = entry.path();
        let file_type = entry
            .file_type()
            .expect("fixture directory entry type reads");
        assert!(!file_type.is_symlink(), "evidence tree contains a symlink");
        if file_type.is_dir() {
            collect(root, &path, files);
        } else {
            assert!(file_type.is_file(), "evidence tree contains a special file");
            if path.file_name().is_some_and(|name| name == "manifest.json") {
                continue;
            }
            files.push(
                path.strip_prefix(root)
                    .expect("fixture stays under root")
                    .to_path_buf(),
            );
        }
    }
}

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("manifest directory"));
    println!("cargo:rerun-if-env-changed=SERVICE_LEGACY_EVIDENCE_ROOT");
    println!("cargo:rerun-if-env-changed=SERVICE_LEGACY_BUILD_RS_POISON");
    assert_ne!(
        env::var("SERVICE_LEGACY_BUILD_RS_POISON").as_deref(),
        Ok("1"),
        "injected service-evidence build-script poison"
    );
    let fixtures = env::var_os("SERVICE_LEGACY_EVIDENCE_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest.join("../../fixtures/service_legacy_evidence"));
    let fixtures = fixtures
        .canonicalize()
        .expect("evidence fixture directory canonicalizes");
    println!("cargo:rerun-if-changed={}", fixtures.display());
    let mut files = Vec::new();
    collect(&fixtures, &fixtures, &mut files);
    files.sort();
    let manifest_path = fixtures.join("manifest.json");
    let mut generated = format!(
        "pub const MANIFEST_BYTES: &[u8] = include_bytes!({manifest_path:?});\npub const EMBEDDED: &[(&str, &[u8])] = &[\n"
    );
    for relative in files {
        let display = relative.to_string_lossy().replace('\\', "/");
        let absolute = fixtures.join(&relative);
        generated.push_str(&format!(
            "    (\"core/fixtures/service_legacy_evidence/{display}\", include_bytes!({absolute:?})),\n"
        ));
        println!(
            "cargo:rerun-if-changed={}",
            fixtures.join(relative).display()
        );
    }
    generated.push_str("];\n");
    fs::write(
        PathBuf::from(env::var("OUT_DIR").expect("output directory")).join("embedded.rs"),
        generated,
    )
    .expect("generated embedded fixture index writes");
}
