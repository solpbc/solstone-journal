// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::{env, fs, path::Path};

fn main() {
    println!("cargo:rerun-if-env-changed=RFDETR_LIB_DIR");
    println!("cargo:rerun-if-env-changed=PARAKEET_LIB_DIR");
    if env::var_os("CARGO_FEATURE_RFDETR_NATIVE").is_none() {
        return;
    }
    let target = env::var("CARGO_CFG_TARGET_OS").unwrap();
    let Some(root) = env::var_os("RFDETR_LIB_DIR") else {
        assert!(
            !matches!(target.as_str(), "ios" | "android"),
            "RFDETR_LIB_DIR is required for the native mobile build"
        );
        return;
    };
    link_archive(&find_named(Path::new(&root), "librfdetr.a"));
    // Parakeet already contributes its canonical GGML family when audio-native is on.
    if env::var_os("CARGO_FEATURE_AUDIO_NATIVE").is_none() {
        link_canonical_ggml(&target);
    }
    if target == "ios" {
        for name in ["Metal", "MetalKit", "Foundation", "Accelerate"] {
            println!("cargo:rustc-link-lib=framework={name}");
        }
        println!("cargo:rustc-link-lib=c++");
    } else if target == "android" {
        println!("cargo:rustc-link-lib=static=c++_static");
        println!("cargo:rustc-link-lib=static=c++abi");
        for name in ["log", "dl", "m", "atomic"] {
            println!("cargo:rustc-link-lib={name}");
        }
    } else {
        println!("cargo:rustc-link-lib=stdc++");
    }
}

fn link_canonical_ggml(target: &str) {
    let Some(root) = env::var_os("PARAKEET_LIB_DIR") else {
        assert!(
            !matches!(target, "ios" | "android"),
            "PARAKEET_LIB_DIR is unset; the canonical GGML set is required"
        );
        return;
    };
    let mut inputs = Vec::new();
    collect_ggml(Path::new(&root), &mut inputs);
    inputs.sort();
    assert!(
        !inputs.is_empty(),
        "PARAKEET_LIB_DIR is missing the canonical GGML set"
    );
    for path in inputs {
        link_archive(&path);
    }
}

fn find_named(root: &Path, file_name: &str) -> std::path::PathBuf {
    let mut found = Vec::new();
    collect_named(root, file_name, &mut found);
    assert!(
        found.len() == 1,
        "RFDETR_LIB_DIR must contain exactly one {file_name}"
    );
    found.pop().unwrap()
}

fn collect_named(root: &Path, file_name: &str, output: &mut Vec<std::path::PathBuf>) {
    for entry in fs::read_dir(root).expect("read RFDETR_LIB_DIR") {
        let path = entry.expect("read native archive entry").path();
        if path.is_dir() {
            collect_named(&path, file_name, output);
        } else if path.file_name().is_some_and(|name| name == file_name) {
            output.push(path);
        }
    }
}

fn collect_ggml(root: &Path, output: &mut Vec<std::path::PathBuf>) {
    for entry in fs::read_dir(root).expect("read PARAKEET_LIB_DIR") {
        let path = entry.expect("read native archive entry").path();
        if path.is_dir() {
            collect_ggml(&path, output);
        } else if path.extension().is_some_and(|extension| extension == "a")
            && path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("libggml"))
        {
            output.push(path);
        }
    }
}

fn link_archive(path: &Path) {
    println!("cargo:rerun-if-changed={}", path.display());
    println!(
        "cargo:rustc-link-search=native={}",
        path.parent().unwrap().display()
    );
    let name = path
        .file_stem()
        .unwrap()
        .to_str()
        .unwrap()
        .strip_prefix("lib")
        .unwrap();
    println!("cargo:rustc-link-lib=static={name}");
}
