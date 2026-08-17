// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::io;
use std::path::Path;

use crate::digest::sha256_hex;
use crate::inventory::{artifact_archives, artifact_set};

pub struct ReleaseInfo<'a> {
    pub product: &'a str,
    pub version: &'a str,
    pub target: &'a str,
    pub commit: &'a str,
    pub lock_sha256: &'a str,
}

pub fn write_sidecars(out_dir: &Path, release: &ReleaseInfo<'_>, basename: &str) -> io::Result<()> {
    let mut checksums = String::new();
    let mut files = Vec::new();
    for name in artifact_archives(basename) {
        let path = out_dir.join(&name);
        if !path.is_file() {
            continue;
        }
        let digest = sha256_hex(&fs::read(&path)?);
        checksums.push_str(&format!("{digest}  {name}\n"));
        files.push((name, digest));
    }
    let [_tar, _deb, _rpm, sha256, manifest_name, release_name] = artifact_set(basename);
    fs::write(out_dir.join(sha256), checksums)?;
    let mut manifest = String::from("{\n");
    manifest.push_str(&format!("  \"product\": {:?},\n", release.product));
    manifest.push_str(&format!("  \"version\": {:?},\n", release.version));
    manifest.push_str(&format!("  \"target\": {:?},\n", release.target));
    manifest.push_str("  \"files\": {\n");
    for (index, (name, digest)) in files.iter().enumerate() {
        let comma = if index + 1 == files.len() { "" } else { "," };
        manifest.push_str(&format!("    {name:?}: {digest:?}{comma}\n"));
    }
    manifest.push_str("  }\n}\n");
    fs::write(out_dir.join(manifest_name), manifest)?;
    fs::write(out_dir.join(release_name), render_release(release))?;
    Ok(())
}

#[must_use]
pub fn render_release(release: &ReleaseInfo<'_>) -> String {
    format!(
        "product={}\nversion={}\ntarget={}\ncommit={}\nlock_sha256={}\n",
        release.product, release.version, release.target, release.commit, release.lock_sha256
    )
}

pub fn self_inspect(out_dir: &Path, basename: &str) -> io::Result<Vec<(String, String)>> {
    let release_name = format!("{basename}.release");
    parse_release(&fs::read_to_string(out_dir.join(release_name))?)
}

pub fn parse_release(text: &str) -> io::Result<Vec<(String, String)>> {
    let mut pairs = Vec::new();
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "release-invalid",
            ));
        };
        pairs.push((key.to_owned(), value.to_owned()));
    }
    if pairs.len() != 5 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "release-invalid",
        ));
    }
    Ok(pairs)
}
