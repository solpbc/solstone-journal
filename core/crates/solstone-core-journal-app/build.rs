// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

include!("../../build-support/windows_version_resource.rs");

fn main() {
    let resources =
        std::path::Path::new("../../../i18n/generated/windows-app/rust/journal-windows-app");
    println!("cargo:rerun-if-changed={}", resources.display());
    let mut tags: Vec<String> = std::fs::read_dir(resources)
        .expect("generated Windows catalog")
        .map(|entry| entry.expect("catalog entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "ftl"))
        .map(|path| path.file_stem().unwrap().to_str().unwrap().to_owned())
        .collect();
    tags.sort();
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    std::fs::write(out.join("languages.rs"), format!("&{tags:?}")).unwrap();
    // The windowed program: Windows starts it without a console.
    windows_app_resource(
        "journal-app",
        "journal-app.exe",
        std::path::Path::new("assets/journal-generic.ico"),
    );
}
