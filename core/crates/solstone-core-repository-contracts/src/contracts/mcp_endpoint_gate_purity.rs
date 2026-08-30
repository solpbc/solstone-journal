// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("core crate has repository parent")
        .to_path_buf()
}

fn crate_manifests(root: &Path) -> Vec<PathBuf> {
    let crates = root.join("core/crates");
    let mut manifests: Vec<_> = fs::read_dir(crates)
        .expect("core crates directory reads")
        .map(|entry| entry.expect("crate directory entry reads"))
        .filter(|entry| {
            entry
                .file_type()
                .expect("crate directory entry type reads")
                .is_dir()
        })
        .map(|entry| entry.path().join("Cargo.toml"))
        .filter(|manifest| manifest.is_file())
        .collect();
    manifests.sort();
    manifests
}

fn collect_source_files(directory: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory).expect("source directory reads") {
        let path = entry.expect("source entry reads").path();
        if path.is_dir() {
            collect_source_files(&path, files);
        } else if path.extension().and_then(|extension| extension.to_str()) == Some("rs") {
            files.push(path);
        }
    }
}

fn source_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for manifest in crate_manifests(root) {
        let source = manifest
            .parent()
            .expect("crate manifest has parent")
            .join("src");
        if source.is_dir() {
            collect_source_files(&source, &mut files);
        }
    }
    files.sort();
    files
}

fn count_occurrences(files: &[PathBuf], needle: &str) -> usize {
    files
        .iter()
        .map(|path| {
            fs::read_to_string(path)
                .expect("production source reads")
                .matches(needle)
                .count()
        })
        .sum()
}

#[test]
fn mcp_endpoint_gate_is_single_pure_default_off_config_surface() {
    let root = repository_root();
    let manifests = crate_manifests(&root);
    let feature = "journal-mcp-endpoint";
    let expected_manifest = root.join("core/crates/solstone-core/Cargo.toml");
    let mut feature_manifests = Vec::new();
    let mut feature_count = 0;

    for manifest in &manifests {
        let text = fs::read_to_string(manifest).expect("crate manifest reads");
        let occurrences = text.matches(feature).count();
        if occurrences != 0 {
            feature_manifests.push(manifest.clone());
            feature_count += occurrences;
        }

        let mut remaining = text.as_str();
        while let Some(start) = remaining.find("default = [") {
            remaining = &remaining[start..];
            let end = remaining
                .find(']')
                .expect("default feature array is closed");
            assert!(
                !remaining[..=end].contains(feature),
                "default feature array in {} must not enable {feature}",
                manifest.display()
            );
            remaining = &remaining[end + 1..];
        }
    }

    assert_eq!(
        feature_count, 1,
        "{feature} must occur in one crate manifest"
    );
    assert_eq!(feature_manifests, vec![expected_manifest]);

    let sources = source_files(&root);
    let expected_module = root.join("core/crates/solstone-core-journal-config/src/mcp_endpoint.rs");
    let endpoint_files: Vec<_> = sources
        .iter()
        .filter(|path| path.file_name().and_then(|name| name.to_str()) == Some("mcp_endpoint.rs"))
        .collect();
    assert_eq!(endpoint_files.len(), 1);
    assert_eq!(endpoint_files[0], &expected_module);

    let capability_function = ["fn mcp_", "endpoint_capability"].concat();
    assert_eq!(count_occurrences(&sources, &capability_function), 1);

    let port_literal = ["76", "58"].concat();
    let port_assignment = format!("= {port_literal};");
    assert_eq!(count_occurrences(&sources, &port_assignment), 1);
    assert!(
        fs::read_to_string(&expected_module)
            .expect("MCP endpoint module reads")
            .contains(&port_literal)
    );

    let config_lib = root.join("core/crates/solstone-core-journal-config/src/lib.rs");
    assert_eq!(
        fs::read_to_string(config_lib)
            .expect("journal config library reads")
            .matches("mcp_endpoint::")
            .count(),
        1
    );
}
