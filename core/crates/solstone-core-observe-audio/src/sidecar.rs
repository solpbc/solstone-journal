// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs::{self, OpenOptions};
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use crate::AudioError;

/// Creates a durable, exclusive little-endian f32 sidecar.
pub fn write_f32le_exclusive(path: &Path, audio: &[f32]) -> Result<(), AudioError> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let file = options
        .open(path)
        .map_err(|source| AudioError::SidecarCreate {
            path: path.to_path_buf(),
            source,
        })?;

    let result = (|| {
        let mut file = file;
        for sample in audio {
            file.write_all(&sample.to_le_bytes())
                .map_err(|source| AudioError::SidecarWrite {
                    path: path.to_path_buf(),
                    source,
                })?;
        }
        file.flush().map_err(|source| AudioError::SidecarWrite {
            path: path.to_path_buf(),
            source,
        })?;
        file.sync_all().map_err(|source| AudioError::SidecarSync {
            path: path.to_path_buf(),
            source,
        })
    })();

    remove_on_error(path, result)
}

fn remove_on_error(path: &Path, result: Result<(), AudioError>) -> Result<(), AudioError> {
    if result.is_err() {
        let _ = fs::remove_file(path);
    }
    result
}

#[cfg(test)]
mod tests {
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::{remove_on_error, write_f32le_exclusive};
    use crate::AudioError;

    fn temporary_path(name: &str) -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "solstone-observe-audio-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn writes_exclusive_little_endian_mode_600_sidecar() {
        let path = temporary_path("sidecar");
        write_f32le_exclusive(&path, &[1.0, -0.5]).expect("write sidecar");
        assert_eq!(
            fs::read(&path).expect("read sidecar"),
            [0, 0, 128, 63, 0, 0, 0, 191]
        );
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(&path)
                .expect("stat sidecar")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let error = write_f32le_exclusive(&path, &[0.0]).expect_err("existing sidecar must fail");
        assert!(matches!(error, AudioError::SidecarCreate { .. }));
        fs::remove_file(path).expect("remove sidecar");
    }

    #[test]
    fn removes_created_file_after_write_or_sync_failure() {
        let path = temporary_path("cleanup");
        fs::write(&path, b"partial sidecar").expect("create partial sidecar");
        let error = remove_on_error(
            &path,
            Err(AudioError::SidecarSync {
                path: path.clone(),
                source: std::io::Error::other("simulated sync failure"),
            }),
        )
        .expect_err("simulated sync failure");
        assert!(matches!(error, AudioError::SidecarSync { .. }));
        assert!(!path.exists());
    }
}
