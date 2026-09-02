// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only segment-directory resolution shared by native health consumers.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use solstone_core_journal_io::{DEFAULT_STREAM, PathOrDay, day_path, iter_segments, segment_path};

use crate::{DataStateMap, scan::detect_data_state};

/// Locate a segment by its directory basename.  Native `Segment::key` strips a
/// valid suffix, while the Python reference compares the basename, so retain it.
pub fn find_segment_dir(
    journal: &Path,
    day: &str,
    segment: &str,
    stream: Option<&str>,
) -> Option<PathBuf> {
    if let Some(stream) = stream.filter(|value| !value.is_empty()) {
        // `_default` names the direct-under-day layout. Routing it through
        // `segment_path` would look in `day/_default/<segment>` — the named
        // layout — and can read a literal `_default/` directory.
        if stream == DEFAULT_STREAM {
            let path = day_path(journal, Some(day), false).ok()?.join(segment);
            return path.is_dir().then_some(path);
        }
        let path = segment_path(journal, day, segment, stream, false).ok()?;
        return path.is_dir().then_some(path);
    }
    iter_segments(journal, PathOrDay::Day(day))
        .ok()?
        .into_iter()
        .find_map(|entry| {
            (entry.name().to_str() == Some(segment)).then(|| entry.path().to_path_buf())
        })
}

/// Read a segment's modality data state, returning an empty map when absent or
/// unreadable, exactly as the reference's read-only entry point does.
pub fn read_segment_data_state(
    journal: &Path,
    day: &str,
    segment: &str,
    stream: Option<&str>,
    now: DateTime<Utc>,
) -> DataStateMap {
    let Some(path) = find_segment_dir(journal, day, segment, stream) else {
        return DataStateMap::default();
    };
    let parent = path
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .unwrap_or(DEFAULT_STREAM);
    detect_data_state(&path, parent, now)
        .map(|(states, _)| states)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use chrono::Utc;

    use super::{find_segment_dir, read_segment_data_state};

    fn snapshot(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        fn visit(root: &Path, current: &Path, rows: &mut Vec<(PathBuf, Vec<u8>)>) {
            let mut entries = fs::read_dir(current)
                .unwrap()
                .map(Result::unwrap)
                .collect::<Vec<_>>();
            entries.sort_by_key(|entry| entry.file_name());
            for entry in entries {
                let path = entry.path();
                let relative = path.strip_prefix(root).unwrap().to_path_buf();
                rows.push((
                    relative,
                    if path.is_file() {
                        fs::read(&path).unwrap()
                    } else {
                        Vec::new()
                    },
                ));
                if path.is_dir() {
                    visit(root, &path, rows);
                }
            }
        }
        let mut rows = Vec::new();
        visit(root, root, &mut rows);
        rows
    }

    #[test]
    fn resolver_is_basename_aware_and_all_reads_leave_the_tree_unchanged() {
        let root = tempfile::tempdir().unwrap();
        let day = root.path().join("chronicle/20260101");
        let alpha = day.join("alpha/090000_300_summary");
        let beta = day.join("beta/090001_300_summary");
        fs::create_dir_all(&alpha).unwrap();
        fs::create_dir_all(&beta).unwrap();
        fs::write(
            alpha.join("audio.jsonl"),
            "{}\n{\"start\":\"00:00:01\",\"text\":\"recognized\"}\n",
        )
        .unwrap();
        let before = snapshot(root.path());
        assert_eq!(
            find_segment_dir(root.path(), "20260101", "090000_300_summary", Some("alpha")),
            Some(alpha.clone())
        );
        assert_eq!(
            find_segment_dir(root.path(), "20260101", "090000_300_summary", None),
            Some(alpha.clone())
        );
        assert_eq!(
            find_segment_dir(root.path(), "20260101", "missing", None),
            None
        );
        assert_eq!(
            read_segment_data_state(
                root.path(),
                "20260101",
                "090000_300_summary",
                Some("alpha"),
                Utc::now(),
            ),
            crate::DataStateMap(std::collections::BTreeMap::from([(
                "audio".to_owned(),
                "analyzed".to_owned(),
            )]))
        );
        assert_eq!(snapshot(root.path()), before);
    }

    #[test]
    fn default_stream_sentinel_addresses_direct_layout_not_named_default() {
        let root = tempfile::tempdir().unwrap();
        let day = root.path().join("chronicle/20260101");
        let direct = day.join("090000_300");
        let named_default = day.join("_default/090000_300");
        fs::create_dir_all(&direct).unwrap();
        fs::create_dir_all(&named_default).unwrap();
        assert_eq!(
            find_segment_dir(root.path(), "20260101", "090000_300", Some("_default")),
            Some(direct.clone())
        );
        fs::remove_dir_all(&direct).unwrap();
        assert_eq!(
            find_segment_dir(root.path(), "20260101", "090000_300", Some("_default")),
            None,
            "named `_default/` must not satisfy the direct-layout sentinel"
        );
    }
}
