// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::env;
use std::fs;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::json;
use solstone_core_artifact_download::{ByteDownload, download_verified_bytes, verify_sha256_bytes};
use solstone_core_journal_io::{AtomicWriteOptions, atomic_replace};

use crate::install::DOWNLOAD_ATTEMPTS;
use crate::readiness::{file_sha256, platform_info};
use crate::runner::{SystemToolRunner, ToolRunner, run_restic};

pub const RCLONE_VERSION: &str = "1.74.4";
pub const RCLONE_SCHEMA_VERSION: u64 = 1;
pub const RCLONE_TOOL: &str = "rclone";
pub const RCLONE_BUNDLE_ENV: &str = "SOLSTONE_RCLONE_BUNDLE";
pub const RCLONE_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(60);
pub const RCLONE_URL_TEMPLATE: &str =
    "https://downloads.rclone.org/v{version}/rclone-v{version}-{os}-{arch}.zip";
pub const RCLONE_ZIP_SHA256: [(&str, &str); 4] = [
    (
        "rclone-v1.74.4-linux-amd64.zip",
        "fe435e0c36228e7c2f116a8701f01127bb1f694005fc11d1f27186c8bca4115d",
    ),
    (
        "rclone-v1.74.4-linux-arm64.zip",
        "97685285c9ad6a0cf17d5844115d2a67245af6444db672187074bd9c358de419",
    ),
    (
        "rclone-v1.74.4-osx-amd64.zip",
        "4188aa84043d7a6240912923f47639a9d2da21f3b40a521c065c8d92e66563f6",
    ),
    (
        "rclone-v1.74.4-osx-arm64.zip",
        "c2100e2d4a4b3be04c55cd45380cafe7647e1ad772bb055f52f00876ed701167",
    ),
];
pub const RCLONE_LICENSE_TEXT: &str = "Copyright (C) 2012 by Nick Craig-Wood http://www.craig-wood.com/nick/\n\nPermission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the \"Software\"), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:\n\nThe above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.\n\nTHE SOFTWARE IS PROVIDED \"AS IS\", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.\n";

pub fn select_rclone_asset(
    os: Option<&str>,
    arch: Option<&str>,
) -> Result<(String, String, String), String> {
    let (os, arch) = match (os, arch) {
        (Some(os), Some(arch)) => (os.to_owned(), arch.to_owned()),
        _ => platform_info()?,
    };
    if !matches!(os.as_str(), "darwin" | "linux") || !matches!(arch.as_str(), "amd64" | "arm64") {
        return Err(format!(
            "rclone unsupported platform: {os}/{arch}; supported: darwin|linux on amd64|arm64"
        ));
    }
    let asset_os = asset_os(&os);
    let filename = format!("rclone-v{RCLONE_VERSION}-{asset_os}-{arch}.zip");
    let digest = RCLONE_ZIP_SHA256
        .iter()
        .find(|(name, _)| *name == filename)
        .map(|(_, digest)| *digest)
        .expect("asset matrix complete");
    Ok((
        filename,
        RCLONE_URL_TEMPLATE
            .replace("{version}", RCLONE_VERSION)
            .replace("{os}", asset_os)
            .replace("{arch}", &arch),
        digest.to_owned(),
    ))
}
pub fn asset_os(os: &str) -> &str {
    if os == "darwin" { "osx" } else { os }
}
pub fn rclone_tool_dir(os: &str) -> Result<PathBuf, String> {
    let home = env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| "HOME is not set".to_owned())?;
    match os {
        "darwin" => Ok(home.join("Library/Application Support/solstone/rclone")),
        "linux" => Ok(home.join(".cache/solstone/rclone")),
        _ => Err(format!("rclone unsupported platform: {os}")),
    }
}
pub fn ensure_rclone(
    force: bool,
    requested_dir: Option<&Path>,
    downloader: &dyn ByteDownload,
) -> Result<PathBuf, String> {
    let (os, arch) = platform_info()?;
    let dir = requested_dir
        .map(Path::to_path_buf)
        .unwrap_or(rclone_tool_dir(&os)?);
    if !force && let Some(path) = check_rclone_ready(&SystemToolRunner, &dir, &os, &arch) {
        return Ok(path);
    }
    let (filename, url, expected) = select_rclone_asset(Some(&os), Some(&arch))?;
    let data = match bundle_path(&filename)? {
        Some(path) => {
            let bytes = fs::read(path).map_err(|error| error.to_string())?;
            verify_sha256_bytes(&bytes, &expected)
                .map_err(|_| "rclone asset SHA mismatch".to_owned())?;
            bytes
        }
        None => download_verified_bytes(
            downloader,
            &url,
            &expected,
            DOWNLOAD_ATTEMPTS,
            RCLONE_DOWNLOAD_TIMEOUT,
        )
        .map_err(|_| "rclone download failed".to_owned())?,
    };
    install_from_zip(&data, &filename, &expected, &dir, &os, &arch)
}
pub fn install_from_zip(
    data: &[u8],
    filename: &str,
    expected: &str,
    dir: &Path,
    os: &str,
    arch: &str,
) -> Result<PathBuf, String> {
    verify_sha256_bytes(data, expected).map_err(|_| "rclone asset SHA mismatch".to_owned())?;
    let member = format!("{}/rclone", filename.trim_end_matches(".zip"));
    let mut archive = zip::ZipArchive::new(Cursor::new(data))
        .map_err(|_| format!("rclone asset extraction failed: {filename}"))?;
    let mut entry = archive
        .by_name(&member)
        .map_err(|_| format!("rclone asset extraction failed: {filename}"))?;
    let mut binary = Vec::new();
    entry
        .read_to_end(&mut binary)
        .map_err(|_| format!("rclone asset extraction failed: {filename}"))?;
    let binary_path = dir.join(RCLONE_TOOL);
    atomic_replace(
        &binary_path,
        &binary,
        AtomicWriteOptions { mode: Some(0o755) },
    )
    .map_err(|error| error.to_string())?;
    let sha256 = file_sha256(&binary_path).map_err(|error| error.to_string())?;
    let payload = json!({"schema_version":RCLONE_SCHEMA_VERSION,"tool":RCLONE_TOOL,"version":RCLONE_VERSION,"sha256":sha256,"platform":{"os":os,"arch":arch},"binary_path":binary_path});
    let mut sentinel = serde_json::to_vec_pretty(&payload).map_err(|error| error.to_string())?;
    sentinel.push(b'\n');
    atomic_replace(
        dir.join(".install-complete"),
        &sentinel,
        AtomicWriteOptions { mode: Some(0o600) },
    )
    .map_err(|error| error.to_string())?;
    fs::write(dir.join("rclone.LICENSE"), RCLONE_LICENSE_TEXT)
        .map_err(|error| error.to_string())?;
    Ok(binary_path)
}
fn check_rclone_ready(
    runner: &dyn ToolRunner,
    dir: &Path,
    os: &str,
    arch: &str,
) -> Option<PathBuf> {
    let binary = dir.join(RCLONE_TOOL);
    let payload: serde_json::Value =
        serde_json::from_slice(&fs::read(dir.join(".install-complete")).ok()?).ok()?;
    if payload.get("schema_version") != Some(&json!(RCLONE_SCHEMA_VERSION))
        || payload.get("tool") != Some(&json!(RCLONE_TOOL))
        || payload.get("version") != Some(&json!(RCLONE_VERSION))
        || payload.pointer("/platform/os")?.as_str()? != os
        || payload.pointer("/platform/arch")?.as_str()? != arch
        || payload.get("binary_path")?.as_str()? != binary.to_string_lossy()
        || file_sha256(&binary).ok()?.as_str() != payload.get("sha256")?.as_str()?
    {
        return None;
    }
    let result = run_restic(
        runner,
        &["version".into()],
        "unused",
        "unused",
        &binary,
        None,
        false,
        None,
        Some(std::time::Duration::from_secs(10)),
        &[],
    )
    .ok()?;
    (result.returncode == 0 && result.stdout.contains(&format!("rclone v{RCLONE_VERSION}")))
        .then_some(binary)
}
fn bundle_path(filename: &str) -> Result<Option<PathBuf>, String> {
    if let Some(path) = env::var_os(RCLONE_BUNDLE_ENV) {
        return Ok(Some(crate::install::expand_and_resolve(PathBuf::from(
            path,
        ))?));
    }
    let sibling = env::current_exe()
        .map_err(|error| error.to_string())?
        .parent()
        .map(|parent| parent.join("_bin").join(filename));
    Ok(sibling.filter(|path| path.exists()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::io::Write;

    fn deflate_zip(filename: &str, binary: &[u8]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .start_file(
                format!("{}/ignored", filename.trim_end_matches(".zip")),
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        writer.write_all(b"ignore").unwrap();
        writer
            .start_file(
                format!("{}/rclone", filename.trim_end_matches(".zip")),
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Deflated),
            )
            .unwrap();
        writer.write_all(binary).unwrap();
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn rclone_asset_matrix_selects_all_pins_and_darwin_alias() {
        for (filename, digest) in RCLONE_ZIP_SHA256 {
            let pieces = filename
                .trim_end_matches(".zip")
                .split('-')
                .collect::<Vec<_>>();
            let os = if pieces[2] == "osx" {
                "darwin"
            } else {
                pieces[2]
            };
            let (selected, url, actual) = select_rclone_asset(Some(os), Some(pieces[3])).unwrap();
            assert_eq!(selected, filename);
            assert_eq!(actual, digest);
            assert!(url.ends_with(filename));
        }
        assert_eq!(asset_os("darwin"), "osx");
        assert!(select_rclone_asset(Some("windows"), Some("amd64")).is_err());
    }

    #[test]
    fn deflate_member_is_extracted_and_invalid_archive_publishes_nothing() {
        let directory = tempfile::tempdir().unwrap();
        let filename = "rclone-v1.74.4-linux-amd64.zip";
        let bytes = deflate_zip(filename, b"rclone-bin");
        let digest = format!("{:x}", Sha256::digest(&bytes));
        let binary = install_from_zip(
            &bytes,
            filename,
            &digest,
            directory.path(),
            "linux",
            "amd64",
        )
        .unwrap();
        assert_eq!(fs::read(binary).unwrap(), b"rclone-bin");
        let bad = tempfile::tempdir().unwrap();
        assert!(
            install_from_zip(
                b"bad",
                filename,
                &format!("{:x}", Sha256::digest(b"bad")),
                bad.path(),
                "linux",
                "amd64"
            )
            .is_err()
        );
        assert!(!bad.path().join(RCLONE_TOOL).exists());
        assert!(!bad.path().join(".install-complete").exists());
    }
}
