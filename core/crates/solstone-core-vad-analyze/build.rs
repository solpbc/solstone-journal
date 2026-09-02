// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

fn main() {
    if std::env::var_os("CARGO_FEATURE_RUNTIME").is_none() {
        return;
    }
    let target_os =
        std::env::var("CARGO_CFG_TARGET_OS").expect("CARGO_CFG_TARGET_OS is set by cargo");
    // Same bundled ONNX Runtime dest as speakers-analyze. The two helpers
    // share the pinned CPU runtime, and the producer inspects both against
    // that one rpath. Each platform spells "next to me" differently: ELF
    // uses $ORIGIN, Mach-O uses @loader_path. Both resolve against the
    // binary itself, not the cwd.
    let rpath = match target_os.as_str() {
        "linux" => "$ORIGIN/../lib/solstone-core-speakers-analyze",
        "macos" => "@loader_path/../lib/solstone-core-speakers-analyze",
        other => {
            panic!(
                "unsupported solstone-core-vad-analyze target OS {other:?}; expected linux or macos"
            )
        }
    };
    println!("cargo:rustc-link-arg-bin=solstone-core-vad-analyze=-Wl,-rpath,{rpath}");
}
