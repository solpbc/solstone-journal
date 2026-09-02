// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

fn main() {
    if std::env::var_os("CARGO_FEATURE_RUNTIME").is_none() {
        return;
    }
    let target_os =
        std::env::var("CARGO_CFG_TARGET_OS").expect("CARGO_CFG_TARGET_OS is set by cargo");
    let rpath = match target_os.as_str() {
        "linux" => "$ORIGIN/../lib/solstone-core-speakers-analyze",
        "macos" => "@loader_path/../lib/solstone-core-speakers-analyze",
        other => panic!(
            "unsupported solstone-core-speakers-analyze target OS {other:?}; expected linux or macos"
        ),
    };
    // Keep the helper's bundled ONNX Runtime lookup relative to the installed
    // helper binary on every supported target.
    println!("cargo:rustc-link-arg-bin=solstone-core-speakers-analyze=-Wl,-rpath,{rpath}");
}
