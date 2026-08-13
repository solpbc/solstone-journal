// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! This integration test runs under `ci-full`, not `make ci`: the ordinary
//! `--lib --bins` unit selection intentionally does not execute `tests/`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("core crate has repository parent")
        .to_path_buf()
}

#[test]
fn ac16_settings_adds_no_native_linkage() {
    let root = repository_root();
    let manifest = root.join("core/Cargo.toml");
    let tree = Command::new("cargo")
        .args([
            "tree",
            "--manifest-path",
            manifest.to_str().expect("manifest path is UTF-8"),
            "-p",
            "solstone-core",
            "-e",
            "normal",
            "--prefix",
            "none",
            "--locked",
        ])
        .current_dir(&root)
        .output()
        .expect("cargo tree runs");
    assert!(
        tree.status.success(),
        "cargo tree failed: {}",
        String::from_utf8_lossy(&tree.stderr)
    );
    let closure = String::from_utf8(tree.stdout)
        .expect("cargo tree output is UTF-8")
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .map(ToOwned::to_owned)
        .collect::<BTreeSet<_>>();

    let metadata = Command::new("cargo")
        .args([
            "metadata",
            "--manifest-path",
            manifest.to_str().expect("manifest path is UTF-8"),
            "--locked",
            "--format-version",
            "1",
        ])
        .current_dir(&root)
        .output()
        .expect("cargo metadata runs");
    assert!(
        metadata.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&metadata.stderr)
    );
    let metadata: Value = serde_json::from_slice(&metadata.stdout).expect("metadata JSON parses");
    let linked = metadata["packages"]
        .as_array()
        .expect("metadata packages")
        .iter()
        .filter_map(|package| {
            let name = package["name"].as_str()?;
            (closure.contains(name) && !package["links"].is_null()).then_some(name.to_owned())
        })
        .collect::<BTreeSet<_>>();
    let expected = BTreeSet::from([
        "ffmpeg-sys-next".to_owned(),
        "libsqlite3-sys".to_owned(),
        "ring".to_owned(),
    ]);
    assert_eq!(linked, expected);

    // `wasm-bindgen-shared` uses a `links` key only as wasm-bindgen's
    // coordination marker; it is not native linkage and is absent from this
    // host normal closure. `solstone-core-ced-sys` is intentionally outside
    // this mechanical predicate too: it has no Cargo `links` key and loads its
    // ced.cpp shared object at runtime through libloading.
}
