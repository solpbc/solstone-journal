// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

fn main() {
    let target_os =
        std::env::var("CARGO_CFG_TARGET_OS").expect("CARGO_CFG_TARGET_OS is set by cargo");
    // Linux-only wave: the helper ships with a bundled ONNX Runtime beside the
    // installed binary and no other target is provisioned yet.
    // The helper ships with a bundled ONNX Runtime beside the installed binary.
    // Each platform spells "next to me" differently: ELF uses $ORIGIN, Mach-O
    // uses @loader_path. Both resolve against the binary itself, not the cwd.
    let rpath = match target_os.as_str() {
        "linux" => "$ORIGIN/../lib/solstone-core-vad-analyze",
        "macos" => "@loader_path/../lib/solstone-core-vad-analyze",
        other => {
            panic!(
                "unsupported solstone-core-vad-analyze target OS {other:?}; expected linux or macos"
            )
        }
    };
    println!("cargo:rustc-link-arg-bin=solstone-core-vad-analyze=-Wl,-rpath,{rpath}");
}
