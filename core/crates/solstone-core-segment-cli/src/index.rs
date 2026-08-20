// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use rusqlite::{Connection, OpenFlags, params};
use solstone_core_indexer_store::db::db_path;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SegmentIndexStatus {
    Absent,
    Ready { indexed: bool, chunks: u64 },
    Unreadable { error: String },
}

impl SegmentIndexStatus {
    pub(crate) fn fields(&self) -> (bool, bool, u64, Option<&str>) {
        match self {
            Self::Absent => (false, false, 0, None),
            Self::Ready { indexed, chunks } => (true, *indexed, *chunks, None),
            Self::Unreadable { error } => (false, false, 0, Some(error)),
        }
    }
}

pub(crate) fn read_segment_index(journal: &Path, rel: &str) -> SegmentIndexStatus {
    let path = db_path(journal);
    if !path.is_file() {
        return SegmentIndexStatus::Absent;
    }
    let result = (|| -> Result<(bool, u64), rusqlite::Error> {
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        // Chunks are stored under the file path (segment/conversation_transcript.jsonl),
        // not the segment directory. A segment is indexed when it has any of those rows.
        let chunks: i64 = connection.query_row(
            "SELECT count(*) FROM chunks WHERE path=?1 OR path LIKE ?2",
            params![rel, format!("{rel}/%")],
            |row| row.get(0),
        )?;
        Ok((chunks > 0, chunks as u64))
    })();
    match result {
        Ok((indexed, chunks)) => SegmentIndexStatus::Ready { indexed, chunks },
        Err(error) => SegmentIndexStatus::Unreadable {
            error: error.to_string(),
        },
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[test]
    fn file_path_chunks_under_a_segment_count_as_indexed() {
        let root = tempfile::TempDir::new().unwrap();
        let db = db_path(root.path());
        std::fs::create_dir_all(db.parent().unwrap()).unwrap();
        let connection = rusqlite::Connection::open(&db).unwrap();
        connection
            .execute("CREATE TABLE chunks (path TEXT)", [])
            .unwrap();
        connection
            .execute(
                "INSERT INTO chunks (path) VALUES (?1)",
                ["20260818/import.text/072821_5/conversation_transcript.jsonl"],
            )
            .unwrap();
        match read_segment_index(root.path(), "20260818/import.text/072821_5") {
            SegmentIndexStatus::Ready { indexed, chunks } => {
                assert!(indexed);
                assert_eq!(chunks, 1);
            }
            other => panic!("expected indexed segment, got {other:?}"),
        }
    }
}
