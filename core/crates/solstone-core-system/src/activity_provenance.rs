// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Where an activity's input provenance lives.
//!
//! Activity work writes this file once every talent for the activity has
//! finished on its current input, so the file's modification time is when the
//! activity's talents last completed. Activity work owns the write; anything
//! else only reads it.

use std::path::{Path, PathBuf};

/// The activity's input provenance file for `day`.
pub fn input_path(journal: &Path, day: &str, facet: &str, activity_id: &str) -> PathBuf {
    journal
        .join("chronicle")
        .join(day)
        .join("health/talent-provenance/activity-inputs")
        .join(component(facet))
        .join(format!("{}.json", component(activity_id)))
}

/// Whether the activity's talents completed at or after `since_ms`. A missing
/// or unreadable file is not a completion.
pub fn completed_since(
    journal: &Path,
    day: &str,
    facet: &str,
    activity_id: &str,
    since_ms: i64,
) -> bool {
    std::fs::metadata(input_path(journal, day, facet, activity_id))
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
        .is_some_and(|modified_ms| modified_ms >= since_ms)
}

/// Match `urllib.parse.quote(value, safe="._-")`, keeping an identifier
/// confined to one filename.
fn component(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'-' => {
                encoded.push(char::from(byte));
            }
            _ => {
                encoded.push('%');
                encoded.push(char::from(HEX[usize::from(byte >> 4)]));
                encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
            }
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn components_are_quote_encoded_into_one_filename() {
        let path = input_path(Path::new("/j"), "20260101", "my facet", "a/b");
        assert_eq!(
            path,
            Path::new(
                "/j/chronicle/20260101/health/talent-provenance/activity-inputs/my%20facet/a%2Fb.json"
            )
        );
    }

    #[test]
    fn completion_is_read_from_the_file_time() {
        let root = tempfile::tempdir().unwrap();
        assert!(!completed_since(root.path(), "20260101", "work", "a", 0));
        let path = input_path(root.path(), "20260101", "work", "a");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{}").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(10))
            .unwrap();
        assert!(completed_since(
            root.path(),
            "20260101",
            "work",
            "a",
            10_000
        ));
        assert!(!completed_since(
            root.path(),
            "20260101",
            "work",
            "a",
            10_001
        ));
    }
}
