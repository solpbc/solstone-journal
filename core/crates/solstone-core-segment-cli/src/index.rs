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
        let indexed = connection
            .query_row("SELECT 1 FROM chunks WHERE path=?1 LIMIT 1", [rel], |_| {
                Ok(())
            })
            .is_ok();
        let chunks: i64 = connection.query_row(
            "SELECT count(*) FROM chunks WHERE path=?1 OR path LIKE ?2",
            params![rel, format!("{rel}/%")],
            |row| row.get(0),
        )?;
        Ok((indexed, chunks as u64))
    })();
    match result {
        Ok((indexed, chunks)) => SegmentIndexStatus::Ready { indexed, chunks },
        Err(error) => SegmentIndexStatus::Unreadable {
            error: error.to_string(),
        },
    }
}
