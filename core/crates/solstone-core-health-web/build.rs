// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Embed the health app's static directory as a MAP, not as named constants.
//!
//! 🔴 The reference registers a static DIRECTORY (`static_folder="static"`,
//! `static_url_path="/static"`), so it serves any file under it. A port that
//! registers one literal path narrows a contract the reference publishes, and
//! the narrowing is silent: a second asset 404s natively while working in
//! Python, and nothing reads the directory to notice. Emitting a map here means
//! adding an asset needs no route change and no build change.

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let static_root = manifest.join("assets/static");
    println!("cargo:rerun-if-changed=assets/workspace.html");
    println!("cargo:rerun-if-changed={}", static_root.display());

    let mut entries: Vec<_> = fs::read_dir(&static_root)
        .expect("health static asset directory is readable")
        .map(|entry| entry.expect("health static entry is readable").path())
        .filter(|path| path.is_file())
        .collect();
    entries.sort();

    let mut generated =
        String::from("pub(super) static STATIC_ASSETS: &[(&str, &str, &[u8])] = &[\n");
    for path in &entries {
        println!("cargo:rerun-if-changed={}", path.display());
        let name = path
            .file_name()
            .expect("health static asset has a file name")
            .to_string_lossy()
            .to_string();
        let content_type = match path.extension().and_then(|e| e.to_str()) {
            Some("js") => "text/javascript; charset=utf-8",
            Some("css") => "text/css; charset=utf-8",
            Some("html") => "text/html; charset=utf-8",
            Some("json") => "application/json",
            Some("svg") => "image/svg+xml",
            Some("png") => "image/png",
            _ => "application/octet-stream",
        };
        generated.push_str(&format!(
            "    ({name:?}, {content_type:?}, include_bytes!({:?})),\n",
            path,
        ));
    }
    generated.push_str("];\n");

    let out = PathBuf::from(env::var_os("OUT_DIR").expect("output dir"));
    fs::write(out.join("static_assets.rs"), generated)
        .expect("generated health static asset source writes");
}
