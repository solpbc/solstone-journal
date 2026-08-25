// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Bounded stderr capture and dynamic-loader failure classification.

use std::io::{self, Read};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

/// Bytes of helper stderr retained for classification and rendering.
pub const STDERR_LIMIT: usize = 65_536;

const LOADER_PREFIX: &str = "error while loading shared libraries: ";
const LOADER_SUFFIX: &str = ": cannot open shared object file";
const DARWIN_LOADER_PREFIX: &str = "Library not loaded: ";
const MAX_LOADER_LIBRARY_BYTES: usize = 4096;
const LOADER_SCAN_TAIL: usize = {
    let linux = LOADER_PREFIX.len() + MAX_LOADER_LIBRARY_BYTES + LOADER_SUFFIX.len();
    let darwin = DARWIN_LOADER_PREFIX.len() + MAX_LOADER_LIBRARY_BYTES + 1;
    if linux > darwin { linux } else { darwin }
};
/// SIGABRT on Darwin and Linux; dyld aborts the process when a library is missing.
const DARWIN_LOADER_ABORT_SIGNAL: i32 = 6;

/// Stderr collected from a helper, truncated and scanned for a loader library name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BoundedStderr {
    pub bytes: Vec<u8>,
    pub truncated: bool,
    pub loader_library: Option<String>,
}

/// Read helper stderr up to [`STDERR_LIMIT`], scanning every chunk for a loader marker.
pub fn read_bounded_stderr(mut stderr: impl Read) -> io::Result<BoundedStderr> {
    let mut bytes = Vec::new();
    let mut truncated = false;
    let mut loader_library = None;
    let mut scan_tail = Vec::new();
    let mut buffer = [0_u8; 8192];
    loop {
        let count = stderr.read(&mut buffer)?;
        if count == 0 {
            return Ok(BoundedStderr {
                bytes,
                truncated,
                loader_library,
            });
        }
        let remaining = STDERR_LIMIT.saturating_sub(bytes.len());
        let kept = remaining.min(count);
        bytes.extend_from_slice(&buffer[..kept]);
        truncated |= kept != count;

        if loader_library.is_none() {
            let mut scan = scan_tail;
            scan.extend_from_slice(&buffer[..count]);
            loader_library = unresolved_library(&scan);
            let tail_start = scan.len().saturating_sub(LOADER_SCAN_TAIL);
            scan_tail = scan[tail_start..].to_vec();
        }
    }
}

/// Return the shared-library name from a Linux or Darwin loader diagnostic.
pub fn unresolved_library(stderr: &[u8]) -> Option<String> {
    if cfg!(target_os = "macos") {
        unresolved_darwin_library(stderr)
    } else {
        unresolved_linux_library(stderr)
    }
}

/// Named loader failure when the process died as a missing-library abort.
pub fn classify_loader_failure(
    exit_code: Option<i32>,
    signal: Option<i32>,
    loader_library: Option<&str>,
) -> Option<String> {
    let loader_failed = if cfg!(target_os = "macos") {
        signal == Some(DARWIN_LOADER_ABORT_SIGNAL)
    } else {
        exit_code == Some(127)
    };
    if loader_failed {
        loader_library
            .filter(|library| !library.is_empty())
            .map(str::to_owned)
    } else {
        None
    }
}

fn unresolved_linux_library(stderr: &[u8]) -> Option<String> {
    let prefix = LOADER_PREFIX.as_bytes();
    let suffix = LOADER_SUFFIX.as_bytes();
    let prefix_start = find_bytes(stderr, prefix)?;
    let library_start = prefix_start + prefix.len();
    let suffix_start = library_start + find_bytes(&stderr[library_start..], suffix)?;
    (suffix_start != library_start)
        .then(|| String::from_utf8_lossy(&stderr[library_start..suffix_start]).into_owned())
}

fn unresolved_darwin_library(stderr: &[u8]) -> Option<String> {
    let prefix = DARWIN_LOADER_PREFIX.as_bytes();
    let library_start = find_bytes(stderr, prefix)? + prefix.len();
    let library_length = find_bytes(&stderr[library_start..], b"\n")?;
    if library_length == 0 || library_length > MAX_LOADER_LIBRARY_BYTES {
        return None;
    }
    let library = Path::new(std::ffi::OsStr::from_bytes(
        &stderr[library_start..library_start + library_length],
    ));
    library
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::{
        classify_loader_failure, unresolved_darwin_library, unresolved_library,
        unresolved_linux_library,
    };

    #[test]
    fn linux_loader_line_yields_the_library_name() {
        let stderr = b"error while loading shared libraries: libonnxruntime.so.1: cannot open shared object file: No such file or directory\n";
        assert_eq!(
            unresolved_linux_library(stderr).as_deref(),
            Some("libonnxruntime.so.1")
        );
    }

    #[test]
    fn linux_exit_127_without_a_library_name_is_not_a_named_loader_failure() {
        assert_eq!(classify_loader_failure(Some(127), None, None), None);
        assert_eq!(classify_loader_failure(Some(127), None, Some("")), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_exit_127_with_a_library_name_is_a_named_loader_failure() {
        assert_eq!(
            classify_loader_failure(Some(127), None, Some("libonnxruntime.so.1")).as_deref(),
            Some("libonnxruntime.so.1")
        );
    }

    #[test]
    fn non_loader_exit_is_not_classified_even_with_a_library_name() {
        assert_eq!(
            classify_loader_failure(Some(64), None, Some("libonnxruntime.so.1")),
            None
        );
    }

    #[test]
    fn darwin_loader_line_yields_the_file_name() {
        let stderr =
            b"Library not loaded: /fixture/libonnxruntime.1.dylib\n  Reason: tried fixture paths\n";
        assert_eq!(
            unresolved_darwin_library(stderr).as_deref(),
            Some("libonnxruntime.1.dylib")
        );
    }

    #[test]
    fn host_unresolved_library_matches_this_platform() {
        let linux = b"error while loading shared libraries: libonnxruntime.so.1: cannot open shared object file";
        let darwin = b"Library not loaded: /fixture/libonnxruntime.1.dylib\n";
        if cfg!(target_os = "macos") {
            assert_eq!(
                unresolved_library(darwin).as_deref(),
                Some("libonnxruntime.1.dylib")
            );
        } else {
            assert_eq!(
                unresolved_library(linux).as_deref(),
                Some("libonnxruntime.so.1")
            );
        }
    }
}
