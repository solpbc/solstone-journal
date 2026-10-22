// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

pub const SCREEN_OCR_BUNDLE_DIR: &str = "lib/solstone-screen-ocr";
pub const TARGET_LINUX_X86_64: &str = "linux-x86_64";
pub const TARGET_LINUX_AARCH64: &str = "linux-aarch64";
pub const TARGET_MACOS_ARM64: &str = "macos-arm64";
pub const TARGET_WINDOWS_X86_64: &str = "windows-x86_64";

pub struct ScreenOcrLayout {
    pub helper: &'static str,
    pub recipe: &'static str,
    pub detector: &'static str,
    pub recognizer: &'static str,
    pub dictionary: &'static str,
    pub runtime_library: &'static str,
    pub runtime_dependency: Option<&'static str>,
}

static LINUX_LAYOUT: ScreenOcrLayout = ScreenOcrLayout {
    helper: "bin/solstone-screen-ocr-helper",
    recipe: "lib/solstone-screen-ocr/recipe.toml",
    detector: "lib/solstone-screen-ocr/models/detector.onnx",
    recognizer: "lib/solstone-screen-ocr/models/recognizer.onnx",
    dictionary: "lib/solstone-screen-ocr/models/dictionary.txt",
    runtime_library: "lib/solstone-screen-ocr/runtime/libonnxruntime.so.1.31.0",
    runtime_dependency: Some("lib/solstone-screen-ocr/runtime/libonnxruntime_providers_shared.so"),
};

static MACOS_ARM64_LAYOUT: ScreenOcrLayout = ScreenOcrLayout {
    helper: "bin/solstone-screen-ocr-helper",
    recipe: "lib/solstone-screen-ocr/recipe.toml",
    detector: "lib/solstone-screen-ocr/models/detector.onnx",
    recognizer: "lib/solstone-screen-ocr/models/recognizer.onnx",
    dictionary: "lib/solstone-screen-ocr/models/dictionary.txt",
    runtime_library: "lib/solstone-screen-ocr/runtime/libonnxruntime.1.31.0.dylib",
    runtime_dependency: None,
};

static WINDOWS_X86_64_LAYOUT: ScreenOcrLayout = ScreenOcrLayout {
    helper: "lib/solstone-native/solstone-screen-ocr-helper.exe",
    recipe: "lib/solstone-screen-ocr/recipe.toml",
    detector: "lib/solstone-screen-ocr/models/detector.onnx",
    recognizer: "lib/solstone-screen-ocr/models/recognizer.onnx",
    dictionary: "lib/solstone-screen-ocr/models/dictionary.txt",
    runtime_library: "lib/solstone-native/solstone-screen-ocr-onnxruntime.dll",
    runtime_dependency: Some("lib/solstone-native/onnxruntime_providers_shared.dll"),
};

#[must_use]
pub fn screen_ocr_layout(target: &str) -> Option<&'static ScreenOcrLayout> {
    match target {
        TARGET_LINUX_X86_64 | TARGET_LINUX_AARCH64 => Some(&LINUX_LAYOUT),
        TARGET_MACOS_ARM64 => Some(&MACOS_ARM64_LAYOUT),
        TARGET_WINDOWS_X86_64 => Some(&WINDOWS_X86_64_LAYOUT),
        _ => None,
    }
}

#[must_use]
pub fn admit_screen_ocr(target: &str, gnu: bool) -> Option<&'static ScreenOcrLayout> {
    match (target, gnu) {
        (TARGET_LINUX_X86_64, true) | (TARGET_LINUX_AARCH64, true) => Some(&LINUX_LAYOUT),
        (TARGET_MACOS_ARM64, false) => Some(&MACOS_ARM64_LAYOUT),
        (TARGET_WINDOWS_X86_64, _) => Some(&WINDOWS_X86_64_LAYOUT),
        _ => None,
    }
}
