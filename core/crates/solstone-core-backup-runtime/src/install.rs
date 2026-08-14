// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::env;
use std::fs;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::time::Duration;

use bzip2_rs::DecoderReader;
use serde_json::json;
use solstone_core_artifact_download::{ByteDownload, download_verified_bytes, verify_sha256_bytes};
use solstone_core_journal_io::{AtomicWriteOptions, atomic_replace};

use crate::readiness::{
    RESTIC_BUNDLE_ENV, RESTIC_SCHEMA_VERSION, RESTIC_TOOL, RESTIC_VERSION, binary_path,
    check_restic_ready_with, file_sha256, license_path, platform_info, select_restic_asset,
    sentinel_path, tool_dir,
};

pub const RESTIC_LICENSE_TEXT: &str = "BSD 2-Clause License\n\nCopyright (c) 2014, Alexander Neumann\nAll rights reserved.\n\nRedistribution and use in source and binary forms, with or without modification, are permitted provided that the following conditions are met:\n\n* Redistributions of source code must retain the above copyright notice, this list of conditions and the following disclaimer.\n\n* Redistributions in binary form must reproduce the above copyright notice, this list of conditions and the following disclaimer in the documentation and/or other materials provided with the distribution.\n\nTHIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS \"AS IS\" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.\n";

pub const DOWNLOAD_ATTEMPTS: u8 = 3;
pub const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30);

pub fn ensure_restic(
    runner: &dyn crate::runner::ToolRunner,
    force: bool,
    requested_dir: Option<&Path>,
    downloader: &dyn ByteDownload,
) -> Result<PathBuf, String> {
    let (os, arch) = platform_info()?;
    let tool_dir = requested_dir
        .map(Path::to_path_buf)
        .unwrap_or(tool_dir(&os)?);
    if !force && let Some(path) = check_restic_ready_with(runner, Some(&tool_dir)) {
        return Ok(path);
    }
    let (filename, url, expected) = select_restic_asset(Some(&os), Some(&arch))?;
    let data = match bundle_path(&filename)? {
        Some(path) => {
            let bytes = fs::read(path).map_err(|error| error.to_string())?;
            verify_sha256_bytes(&bytes, &expected)
                .map_err(|_| "restic asset SHA mismatch".to_owned())?;
            bytes
        }
        None => download_verified_bytes(
            downloader,
            &url,
            &expected,
            DOWNLOAD_ATTEMPTS,
            DOWNLOAD_TIMEOUT,
        )
        .map_err(|_| "restic download failed".to_owned())?,
    };
    install_from_bz2(&data, &expected, &tool_dir, &os, &arch)
}

pub fn install_from_bz2(
    data: &[u8],
    expected_sha256: &str,
    tool_dir: &Path,
    os: &str,
    arch: &str,
) -> Result<PathBuf, String> {
    verify_sha256_bytes(data, expected_sha256)
        .map_err(|_| "restic asset SHA mismatch".to_owned())?;
    let mut decoded = Vec::new();
    DecoderReader::new(Cursor::new(data))
        .read_to_end(&mut decoded)
        .map_err(|_| "restic asset decompression failed".to_owned())?;
    let binary = binary_path(tool_dir);
    atomic_replace(&binary, &decoded, AtomicWriteOptions { mode: Some(0o755) })
        .map_err(|error| error.to_string())?;
    let digest = file_sha256(&binary).map_err(|error| error.to_string())?;
    let payload = json!({"schema_version": RESTIC_SCHEMA_VERSION, "tool": RESTIC_TOOL, "version": RESTIC_VERSION, "sha256": digest, "platform":{"os":os,"arch":arch}, "binary_path":binary});
    let sentinel = serde_json::to_vec_pretty(&payload).map_err(|error| error.to_string())?;
    let mut sentinel_bytes = sentinel;
    sentinel_bytes.push(b'\n');
    atomic_replace(
        sentinel_path(tool_dir),
        &sentinel_bytes,
        AtomicWriteOptions { mode: Some(0o600) },
    )
    .map_err(|error| error.to_string())?;
    fs::write(license_path(tool_dir), RESTIC_LICENSE_TEXT).map_err(|error| error.to_string())?;
    Ok(binary)
}

fn bundle_path(filename: &str) -> Result<Option<PathBuf>, String> {
    if let Some(path) = env::var_os(RESTIC_BUNDLE_ENV) {
        return Ok(Some(expand_and_resolve(PathBuf::from(path))?));
    }
    let sibling = env::current_exe()
        .map_err(|error| error.to_string())?
        .parent()
        .map(|parent| parent.join("_bin").join(filename));
    Ok(sibling.filter(|path| path.exists()))
}

pub(crate) fn expand_and_resolve(path: PathBuf) -> Result<PathBuf, String> {
    let path = path
        .to_str()
        .and_then(|value| value.strip_prefix("~/"))
        .map(|suffix| {
            env::var_os("HOME")
                .map(PathBuf::from)
                .ok_or_else(|| "HOME is not set".to_owned())
                .map(|home| home.join(suffix))
        })
        .transpose()?
        .unwrap_or(path);
    if path.is_absolute() {
        Ok(path)
    } else {
        env::current_dir()
            .map(|cwd| cwd.join(path))
            .map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    const BZ2_FIXTURE: &[u8] = &[
        66, 90, 104, 57, 49, 65, 89, 38, 83, 89, 92, 93, 198, 140, 0, 0, 2, 209, 128, 0, 16, 0, 2,
        11, 32, 30, 64, 32, 0, 49, 0, 211, 77, 4, 0, 196, 77, 102, 181, 144, 136, 93, 171, 197,
        220, 145, 78, 20, 36, 23, 23, 113, 163, 0,
    ];
    fn digest(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    #[test]
    fn sha_and_decompression_fail_closed_before_publication() {
        let directory = tempfile::tempdir().unwrap();
        assert!(install_from_bz2(BZ2_FIXTURE, "00", directory.path(), "linux", "amd64").is_err());
        assert!(
            install_from_bz2(
                b"not-bzip2",
                &digest(b"not-bzip2"),
                directory.path(),
                "linux",
                "amd64"
            )
            .is_err()
        );
        for path in [
            binary_path(directory.path()),
            sentinel_path(directory.path()),
            license_path(directory.path()),
        ] {
            assert!(!path.exists());
        }
    }

    #[cfg(unix)]
    #[test]
    fn publication_keeps_reference_partial_order_and_modes() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let path = install_from_bz2(
            BZ2_FIXTURE,
            &digest(BZ2_FIXTURE),
            directory.path(),
            "linux",
            "amd64",
        )
        .unwrap();
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o755
        );
        assert_eq!(
            fs::metadata(sentinel_path(directory.path()))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let license = license_path(directory.path());
        fs::set_permissions(&license, fs::Permissions::from_mode(0o640)).unwrap();
        install_from_bz2(
            BZ2_FIXTURE,
            &digest(BZ2_FIXTURE),
            directory.path(),
            "linux",
            "amd64",
        )
        .unwrap();
        assert_eq!(
            fs::metadata(&license).unwrap().permissions().mode() & 0o777,
            0o640
        );
    }
}
