// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

/// Keep restic's Windows volume name stable instead of archiving a verbatim prefix.
pub fn restic_filesystem_path(path: &Path) -> String {
    let value = path.to_string_lossy();
    #[cfg(windows)]
    return windows_filesystem_path(&value).to_owned();
    #[cfg(not(windows))]
    value.into_owned()
}

/// Restic tree selectors use slash paths and a virtual drive directory on Windows.
pub fn restic_tree_path(path: &Path) -> String {
    let value = restic_filesystem_path(path);
    #[cfg(windows)]
    return windows_tree_path(&value);
    #[cfg(not(windows))]
    value
}

#[cfg(any(windows, test))]
fn windows_filesystem_path(value: &str) -> &str {
    value.strip_prefix(r"\\?\").unwrap_or(value)
}

#[cfg(any(windows, test))]
fn windows_tree_path(value: &str) -> String {
    let value = windows_filesystem_path(value);
    let bytes = value.as_bytes();
    if bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/')
    {
        format!("/{}{}", &value[..1], value[2..].replace('\\', "/"))
    } else {
        // A snapshot created on Unix already carries its slash-separated tree path.
        value.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_drive_tree_paths_preserve_spaces_unicode_and_case() {
        for input in [
            r"C:\Owner café\journal",
            r"\\?\C:\Owner café\journal",
            "C:/Owner café/journal",
        ] {
            assert_eq!(windows_tree_path(input), "/C/Owner café/journal");
        }
        assert_eq!(windows_tree_path(r"d:\journal"), "/d/journal");
        assert_eq!(
            windows_tree_path("/home/owner/journal"),
            "/home/owner/journal"
        );
        assert_eq!(windows_filesystem_path(r"\\?\C:\journal"), r"C:\journal");
    }

    #[cfg(unix)]
    #[test]
    fn unix_backslashes_and_colons_are_unchanged() {
        let path = Path::new(r"/journal/owner\notes:one");
        assert_eq!(restic_filesystem_path(path), path.to_str().unwrap());
        assert_eq!(restic_tree_path(path), path.to_str().unwrap());
    }
}
