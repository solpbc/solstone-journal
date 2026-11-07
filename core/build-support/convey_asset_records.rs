// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// Build-script helper for a crate that serves dashboard assets. The crate's
// build script declares what it serves, next to the files, and this writes the
// list for the dashboard shell's build script, which hashes and rewrites them.
//
// The crate's Cargo.toml needs a `links` key so the list reaches the shell's
// build script as `DEP_<links>_RECORDS`.
//
// One record per line: kind, route, content type and source file, tab-separated.
// A kind is `asset` (the bytes are the file's), `template` (the crate fills in
// build-time values at response time) or `dynamic` (the response depends on the
// request or this computer, so it is never cached or rewritten).

#[allow(dead_code)]
pub struct AssetRecords {
    manifest: std::path::PathBuf,
    lines: Vec<String>,
}

impl Default for AssetRecords {
    fn default() -> Self {
        Self::new()
    }
}

#[allow(dead_code)]
impl AssetRecords {
    pub fn new() -> Self {
        let manifest = std::path::PathBuf::from(
            std::env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir"),
        );
        Self {
            manifest,
            lines: Vec::new(),
        }
    }

    fn push(&mut self, kind: &str, route: &str, relative: &str) {
        let path = self.manifest.join(relative);
        assert!(path.is_file(), "asset record {route} has no file {relative}");
        println!("cargo:rerun-if-changed={}", path.display());
        let path = path.to_string_lossy().replace('\\', "/");
        assert!(
            !route.contains('\t') && !path.contains('\t'),
            "asset record {route} has a tab"
        );
        let content_type = asset_content_type(&path);
        self.lines
            .push(format!("{kind}\t{route}\t{content_type}\t{path}"));
    }

    /// A file whose response is exactly its bytes.
    pub fn file(&mut self, route: &str, relative: &str) -> &mut Self {
        self.push("asset", route, relative);
        self
    }

    /// A file the crate fills in at response time with values fixed for this build.
    pub fn template(&mut self, route: &str, relative: &str) -> &mut Self {
        self.push("template", route, relative);
        self
    }

    /// A file whose response depends on the request or this computer.
    pub fn dynamic(&mut self, route: &str, relative: &str) -> &mut Self {
        self.push("dynamic", route, relative);
        self
    }

    /// Every file under `directory`, served at `route_root/<path under directory>`,
    /// except the listed relative paths (an entry ending in `/` leaves out that
    /// whole directory). A new file is recorded without an edit here.
    pub fn tree(&mut self, route_root: &str, directory: &str, except: &[&str]) -> &mut Self {
        let root = self.manifest.join(directory);
        println!("cargo:rerun-if-changed={}", root.display());
        let mut files = Vec::new();
        collect_asset_files(&root, &mut files);
        files.sort();
        for path in files {
            let relative = path
                .strip_prefix(&root)
                .expect("asset under its tree")
                .to_string_lossy()
                .replace('\\', "/");
            if except.iter().any(|left_out| {
                *left_out == relative
                    || (left_out.ends_with('/') && relative.starts_with(left_out))
            }) {
                continue;
            }
            let file = format!("{directory}/{relative}");
            self.push("asset", &format!("{route_root}/{relative}"), &file);
        }
        self
    }

    /// Write the list and hand its path to the shell's build script.
    pub fn emit(&mut self) {
        self.lines.sort();
        let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("output dir"));
        let path = out.join("convey_asset_records.tsv");
        let mut body = self.lines.join("\n");
        body.push('\n');
        std::fs::write(&path, body).expect("asset records write");
        println!("cargo::metadata=records={}", path.display());
    }
}

#[allow(dead_code)]
fn collect_asset_files(directory: &std::path::Path, files: &mut Vec<std::path::PathBuf>) {
    println!("cargo:rerun-if-changed={}", directory.display());
    for entry in std::fs::read_dir(directory).expect("asset directory is readable") {
        let path = entry.expect("asset directory entry is readable").path();
        if path.is_dir() {
            collect_asset_files(&path, files);
        } else {
            files.push(path);
        }
    }
}

#[allow(dead_code)]
pub fn asset_content_type(path: &str) -> &'static str {
    match path.rsplit_once('.').map(|(_, extension)| extension) {
        Some("css") => "text/css; charset=utf-8",
        Some("html") => "text/html; charset=utf-8",
        Some("ico") => "image/x-icon",
        Some("jpeg") | Some("jpg") => "image/jpeg",
        Some("js") => "text/javascript; charset=utf-8",
        Some("json") => "application/json",
        Some("md") => "text/markdown; charset=utf-8",
        Some("png") => "image/png",
        Some("svg") => "image/svg+xml",
        Some("webp") => "image/webp",
        Some("woff2") => "font/woff2",
        _ => "application/octet-stream",
    }
}
