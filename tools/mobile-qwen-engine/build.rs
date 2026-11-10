// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::{env, path::PathBuf};

fn main() {
    println!("cargo:rustc-check-cfg=cfg(journal_mobile_qwen_native)");
    println!("cargo:rerun-if-env-changed=QWEN_ENGINE_LIB_DIR");
    if env::var_os("CARGO_FEATURE_NATIVE").is_none() {
        return;
    }
    let target = env::var("CARGO_CFG_TARGET_OS").expect("target OS");
    let Some(directory) = env::var_os("QWEN_ENGINE_LIB_DIR") else {
        assert!(
            !matches!(target.as_str(), "ios" | "android"),
            "QWEN_ENGINE_LIB_DIR is required for the bundled mobile engine"
        );
        return;
    };
    let directory = PathBuf::from(directory);
    // Keep Qwen's GGML in its own image. The audio adapter links a different
    // GGML family into the app; static linkage here would mix their symbols.
    if target == "ios" {
        let framework = directory.join("JournalMobileQwen.framework/JournalMobileQwen");
        assert!(framework.is_file(), "missing JournalMobileQwen framework");
        println!("cargo:rerun-if-changed={}", framework.display());
        println!("cargo:rustc-link-search=framework={}", directory.display());
        println!("cargo:rustc-link-lib=framework=JournalMobileQwen");
    } else {
        let library = directory.join("libjournal_mobile_qwen.so");
        assert!(library.is_file(), "missing embedded Qwen shared library");
        println!("cargo:rerun-if-changed={}", library.display());
        println!("cargo:rustc-link-search=native={}", directory.display());
        println!("cargo:rustc-link-lib=dylib=journal_mobile_qwen");
    }
    println!("cargo:rustc-cfg=journal_mobile_qwen_native");
}
