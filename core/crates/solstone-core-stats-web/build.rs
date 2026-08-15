// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Embed the stats app's static directory as a MAP, not as literal paths.
//!
//! The reference publishes a static directory. Walking it at build time keeps
//! the native route broad when an asset is added without narrowing it to a
//! hand-maintained list of filenames.

use std::env;
use std::fs;
use std::io::Write;
use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let static_root = manifest.join("assets/static");
    println!("cargo:rerun-if-changed=assets/workspace.html");
    println!("cargo:rerun-if-changed={}", static_root.display());
    let mut entries: Vec<_> = fs::read_dir(&static_root)
        .expect("stats static asset directory is readable")
        .map(|entry| entry.expect("stats static entry is readable").path())
        .filter(|path| path.is_file())
        .collect();
    entries.sort();
    let mut generated =
        String::from("pub(super) static STATIC_ASSETS: &[(&str, &str, &[u8])] = &[\n");
    for path in entries {
        println!("cargo:rerun-if-changed={}", path.display());
        let name = path.file_name().expect("asset name").to_string_lossy();
        let content_type = match path.extension().and_then(|value| value.to_str()) {
            Some("js") => "text/javascript; charset=utf-8",
            Some("css") => "text/css; charset=utf-8",
            Some("html") => "text/html; charset=utf-8",
            Some("json") => "application/json",
            Some("svg") => "image/svg+xml",
            _ => "application/octet-stream",
        };
        generated.push_str(&format!(
            "    ({name:?}, {content_type:?}, include_bytes!({:?})),\n",
            path
        ));
    }
    generated.push_str("];\n");
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("output dir"));
    let mut output = fs::File::options()
        .create(true)
        .truncate(true)
        .write(true)
        .open(out.join("static_assets.rs"))
        .expect("generated stats asset source opens");
    output
        .write_all(generated.as_bytes())
        .expect("generated stats asset source writes");
}
