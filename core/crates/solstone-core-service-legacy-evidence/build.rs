// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn collect(root: &Path, directory: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory).expect("evidence fixture directory reads") {
        let path = entry.expect("fixture directory entry reads").path();
        if path.is_dir() {
            collect(root, &path, files);
        } else if path.file_name().is_some_and(|name| name != "manifest.json") {
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
    let fixtures = manifest.join("../../fixtures/service_legacy_evidence");
    println!("cargo:rerun-if-changed={}", fixtures.display());
    let mut files = Vec::new();
    collect(&fixtures, &fixtures, &mut files);
    files.sort();
    let mut generated = String::from("pub const EMBEDDED: &[(&str, &[u8])] = &[\n");
    for relative in files {
        let display = relative.to_string_lossy().replace('\\', "/");
        generated.push_str(&format!(
            "    (\"core/fixtures/service_legacy_evidence/{display}\", include_bytes!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/../../fixtures/service_legacy_evidence/{display}\"))),\n"
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
