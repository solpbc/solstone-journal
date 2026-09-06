// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

pub const DEFAULT_STREAM: &str = "_default";
// AC10 forbids memoizing journal reads, not compiling this fixed regex once.
static SEGMENT_KEY_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(\d{6})_(\d+)(?:_|\b)").expect("fixed segment regex"));

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentDirectory {
    pub stream: String,
    pub key: String,
    pub path: PathBuf,
}

/// Extract a segment key with Python `re.search` semantics.
pub fn segment_key(value: &str) -> Option<String> {
    SEGMENT_KEY_RE
        .captures(value)
        .map(|captures| format!("{}_{}", &captures[1], &captures[2]))
}

/// Route predicate: the whole supplied path component must be a bare key.
pub fn is_exact_segment_key(value: &str) -> bool {
    segment_key(value).as_deref() == Some(value)
}

// Deliberate local segment stack: sibling ports keep this traversal private; do not couple this crate to another app's route semantics.
pub fn iter_segments(day_dir: &Path) -> Vec<SegmentDirectory> {
    let Ok(entries) = fs::read_dir(day_dir) else {
        return Vec::new();
    };
    let mut segments = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if segment_key(&name).is_some() {
            segments.push(SegmentDirectory {
                stream: DEFAULT_STREAM.to_owned(),
                key: name,
                path,
            });
            continue;
        }
        if name == "health" {
            continue;
        }
        let Ok(children) = fs::read_dir(&path) else {
            continue;
        };
        for child in children.filter_map(Result::ok) {
            let child_path = child.path();
            let key = child.file_name().to_string_lossy().into_owned();
            if child_path.is_dir() && segment_key(&key).is_some() {
                segments.push(SegmentDirectory {
                    stream: name.clone(),
                    key,
                    path: child_path,
                });
            }
        }
    }
    segments.sort_by(|left, right| left.key.cmp(&right.key));
    segments
}

pub fn day_segment_counts(
    root: &Path,
    month: Option<&str>,
) -> std::collections::BTreeMap<String, usize> {
    let chronicle = root.join("chronicle");
    let Ok(days) = fs::read_dir(chronicle) else {
        return Default::default();
    };
    days.filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let day = entry.file_name().to_string_lossy().into_owned();
            (path.is_dir() && is_day(&day) && month.is_none_or(|month| day.starts_with(month)))
                .then(|| (day, iter_segments(&path).len()))
        })
        .filter(|(_, count)| *count > 0)
        .collect()
}

pub fn is_day(value: &str) -> bool {
    value.len() == 8 && value.bytes().all(|byte| byte.is_ascii_digit())
}

pub fn is_month(value: &str) -> bool {
    value.len() == 6 && value.bytes().all(|byte| byte.is_ascii_digit())
}

pub fn origin(day: &str, stream: &str, key: &str) -> String {
    if stream == DEFAULT_STREAM {
        format!("{day}/{key}")
    } else {
        format!("{day}/{stream}/{key}")
    }
}

#[cfg(test)]
mod tests {
    use super::{day_segment_counts, is_exact_segment_key, iter_segments, segment_key};
    use crate::test_support::{phase_root, write};

    #[test]
    fn segment_key_retains_search_and_exact_match_semantics() {
        assert_eq!(segment_key("100000_300").as_deref(), Some("100000_300"));
        assert_eq!(segment_key("999999_300").as_deref(), Some("999999_300"));
        assert_eq!(segment_key("foo_100000_300_bar"), None);
        assert_eq!(segment_key("workstation.browser"), None);
        assert!(is_exact_segment_key("999999_300"));
        assert!(!is_exact_segment_key("foo_100000_300_bar"));
    }

    #[test]
    fn ac18_health_children_do_not_become_segments() {
        let root = phase_root("populated");
        let day_dir = root.path().join("chronicle/20260510");
        let before_segments = iter_segments(&day_dir);
        let before_counts = day_segment_counts(root.path(), Some("202605"));

        write(
            &root
                .path()
                .join("chronicle/20260510/health/090000_300/audio.jsonl"),
            "health must not be a stream\n",
        );

        assert_eq!(iter_segments(&day_dir), before_segments);
        assert_eq!(
            day_segment_counts(root.path(), Some("202605")),
            before_counts
        );
    }
}
