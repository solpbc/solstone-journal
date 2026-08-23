// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use solstone_core_journal_io::{ExactLookupError, resolve_stream_exact};

use crate::{SOURCE_APPLE_HEALTH, health_card_stream};

const DAY_SUMMARY_FILE: &str = "day_summary_transcript.md";

#[derive(Debug)]
pub enum ChronicleReadError {
    Read { path: PathBuf, source: io::Error },
    Path(ExactLookupError),
}

impl fmt::Display for ChronicleReadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => write!(
                formatter,
                "could not read day summary {}: {source}",
                path.display()
            ),
            Self::Path(error) => write!(formatter, "could not locate day summary stream: {error}"),
        }
    }
}
impl std::error::Error for ChronicleReadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            Self::Path(error) => Some(error),
        }
    }
}

pub fn find_day_summary(
    journal_root: impl AsRef<Path>,
    day: &str,
) -> Result<Option<String>, ChronicleReadError> {
    let stream = health_card_stream(SOURCE_APPLE_HEALTH).expect("Apple Health has a card stream");
    let root = match resolve_stream_exact(journal_root.as_ref(), day, stream) {
        Ok(None) => return Ok(None),
        Ok(Some(path)) => path,
        Err(error) => return Err(ChronicleReadError::Path(error)),
    };
    let mut segments = fs::read_dir(&root)
        .map_err(|source| ChronicleReadError::Read {
            path: root.clone(),
            source,
        })?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect::<Vec<_>>();
    segments.sort();
    for segment in segments {
        let candidate = segment.join(DAY_SUMMARY_FILE);
        if candidate.exists() {
            return fs::read_to_string(&candidate).map(Some).map_err(|source| {
                ChronicleReadError::Read {
                    path: candidate,
                    source,
                }
            });
        }
    }
    Ok(None)
}

pub fn has_chronicle_day(journal_root: impl AsRef<Path>, day: &str) -> bool {
    journal_root.as_ref().join("chronicle").join(day).is_dir()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;
    use crate::{SOURCE_APPLE_HEALTH, health_card_stream};

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TempDir(PathBuf);
    impl TempDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "solstone-convey-body-chronicle-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
    fn candidate(root: &Path, day: &str, segment: &str) -> PathBuf {
        root.join("chronicle")
            .join(day)
            .join(health_card_stream(SOURCE_APPLE_HEALTH).unwrap())
            .join(segment)
            .join(DAY_SUMMARY_FILE)
    }

    #[test]
    fn summary_reader_distinguishes_absence_from_unreadable_candidates() {
        let temporary = TempDir::new();
        let found = candidate(temporary.path(), "20240101", "100000_300");
        fs::create_dir_all(found.parent().unwrap()).unwrap();
        fs::write(&found, "summary").unwrap();
        assert_eq!(
            find_day_summary(temporary.path(), "20240101").unwrap(),
            Some("summary".to_owned())
        );
        let missing_day = find_day_summary(temporary.path(), "20240102").unwrap();
        let empty_stream = temporary
            .path()
            .join("chronicle/20240103")
            .join(health_card_stream(SOURCE_APPLE_HEALTH).unwrap());
        fs::create_dir_all(empty_stream).unwrap();
        assert_eq!(
            missing_day,
            find_day_summary(temporary.path(), "20240103").unwrap()
        );
        let first = candidate(temporary.path(), "20240104", "100000_300");
        fs::create_dir_all(&first).unwrap();
        let second = candidate(temporary.path(), "20240104", "200000_300");
        fs::create_dir_all(second.parent().unwrap()).unwrap();
        fs::write(second, "later").unwrap();
        assert!(
            matches!(find_day_summary(temporary.path(), "20240104"), Err(ChronicleReadError::Read { path, .. }) if path == first)
        );
    }

    #[test]
    fn chronicle_day_predicate_is_independent_of_summary_files() {
        let temporary = TempDir::new();
        fs::create_dir_all(temporary.path().join("chronicle/20240105")).unwrap();
        assert!(has_chronicle_day(temporary.path(), "20240105"));
        assert!(!has_chronicle_day(temporary.path(), "20240106"));
    }

    #[test]
    fn summary_reader_maps_wrong_kind_stream_to_path_error() {
        let temporary = TempDir::new();
        let day = temporary.path().join("chronicle/20240107");
        fs::create_dir_all(&day).unwrap();
        fs::write(
            day.join(health_card_stream(SOURCE_APPLE_HEALTH).unwrap()),
            b"not-a-stream",
        )
        .unwrap();
        match find_day_summary(temporary.path(), "20240107") {
            Err(ChronicleReadError::Path(
                solstone_core_journal_io::ExactLookupError::WrongKind { .. },
            )) => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn summary_reader_absent_stream_is_none() {
        let temporary = TempDir::new();
        fs::create_dir_all(temporary.path().join("chronicle/20240108")).unwrap();
        assert_eq!(
            find_day_summary(temporary.path(), "20240108").unwrap(),
            None
        );
    }
}
