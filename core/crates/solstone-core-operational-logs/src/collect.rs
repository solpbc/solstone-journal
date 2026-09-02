// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use chrono::NaiveDateTime;
use solstone_core_journal_io::{JournalRoot, operational_log::catalog_oplogs};
use solstone_core_system::operational_log_parse::{ParsedHealthLogRow, parse_health_log_row};
use solstone_core_system_health::GrepPattern;

use crate::error::CollectError;
use crate::read::{StdTailFileOpener, tail_reverse_text, tail_slice};

/// Fully resolved options for a one-shot operational-log read.
#[derive(Debug, Clone)]
pub struct HealthLogsQuery {
    pub count: i64,
    pub since: Option<NaiveDateTime>,
    pub service: Option<String>,
    pub grep: Option<GrepPattern>,
}

/// Collect and order today's canonical operational logs without rendering them.
pub fn collect_health_logs(
    journal_root: &Path,
    now: NaiveDateTime,
    query: &HealthLogsQuery,
) -> Result<Vec<ParsedHealthLogRow>, CollectError> {
    let root = JournalRoot::open(journal_root).map_err(|_| CollectError::Root)?;
    let day = now.date();
    let snapshot = catalog_oplogs(root, &[day]).map_err(CollectError::Catalog)?;
    let has_filters = query.since.is_some()
        || query
            .service
            .as_deref()
            .is_some_and(|service| !service.is_empty())
        || query.grep.is_some();
    let mut rows = Vec::new();

    for (entry, mut file) in snapshot.into_catalogued_entries() {
        if query
            .service
            .as_deref()
            .is_some_and(|service| !service.is_empty())
            && query.service.as_deref() != Some(entry.name().source().display_slug())
        {
            continue;
        }
        file.seek(SeekFrom::Start(entry.payload_offset() as u64))
            .map_err(|_| CollectError::CatalogIo)?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .map_err(|_| CollectError::CatalogIo)?;
        let text = String::from_utf8(bytes).map_err(|_| CollectError::CatalogUtf8)?;
        let count = if has_filters { 0 } else { query.count };
        for raw in tail_slice(splitlines(&text), count) {
            if let Some(row) = parse_health_log_row(&raw)
                && (!has_filters || matches_filters(&row, query))
            {
                rows.push(row);
            }
        }
    }

    // `supervisor.log` is an explicit, non-canonical unfiltered input, not a
    // managed-process alias; retain its historical behaviour unchanged.
    if !has_filters {
        let supervisor_path = journal_root.join("health").join("supervisor.log");
        for raw in tail_reverse_text(&supervisor_path, query.count, &StdTailFileOpener) {
            if let Some(row) = parse_health_log_row(&raw) {
                rows.push(row);
            }
        }
    }

    rows.sort_by_key(|row| row.timestamp);
    Ok(if has_filters {
        tail_slice(rows, query.count)
    } else {
        rows
    })
}

fn matches_filters(row: &ParsedHealthLogRow, query: &HealthLogsQuery) -> bool {
    if let Some(since) = query.since
        && row.timestamp < since
    {
        return false;
    }
    if let Some(grep) = &query.grep
        && !grep.is_match(&row.raw)
    {
        return false;
    }
    true
}

fn splitlines(text: &str) -> Vec<String> {
    text.lines().map(ToOwned::to_owned).collect()
}

#[cfg(test)]
mod tests {
    use chrono::{FixedOffset, TimeZone};
    use solstone_core_journal_io::operational_log::{OplogFormat, create_oplog_at};

    use super::*;

    #[test]
    fn service_filter_uses_the_canonical_source_not_run_or_payload_text() {
        let temporary = tempfile::tempdir().unwrap();
        let root = JournalRoot::open(temporary.path()).unwrap();
        let opened = FixedOffset::east_opt(0)
            .unwrap()
            .with_ymd_and_hms(2026, 8, 7, 12, 0, 0)
            .single()
            .unwrap();
        let mut writer =
            create_oplog_at(root, "source", "run-only", OplogFormat::Log, opened).unwrap();
        use std::io::Write;
        writeln!(
            writer,
            "2026-08-07 12:00:00 [payload-only:stdout] ERROR one"
        )
        .unwrap();
        drop(writer);
        let base = HealthLogsQuery {
            count: 5,
            since: None,
            service: Some("source".to_owned()),
            grep: None,
        };
        assert_eq!(
            collect_health_logs(temporary.path(), opened.naive_local(), &base)
                .unwrap()
                .len(),
            1
        );
        for service in ["run-only", "payload-only"] {
            let query = HealthLogsQuery {
                service: Some(service.to_owned()),
                ..base.clone()
            };
            assert!(
                collect_health_logs(temporary.path(), opened.naive_local(), &query)
                    .unwrap()
                    .is_empty()
            );
        }
    }

    #[test]
    fn canonical_segments_are_merged_in_deterministic_timestamp_order() {
        use std::io::Write;

        let temporary = tempfile::tempdir().unwrap();
        let opened = FixedOffset::east_opt(0)
            .unwrap()
            .with_ymd_and_hms(2026, 8, 7, 12, 0, 0)
            .single()
            .unwrap();
        for (source, lines) in [
            (
                "alpha",
                [
                    "2026-08-07T12:00:03 [payload-a:stdout] third",
                    "2026-08-07T12:00:05 [payload-a:stdout] fifth",
                ],
            ),
            (
                "beta",
                [
                    "2026-08-07T12:00:01 [payload-b:stdout] first",
                    "2026-08-07T12:00:04 [payload-b:stdout] fourth",
                ],
            ),
            (
                "gamma",
                [
                    "2026-08-07T12:00:02 [payload-c:stdout] second",
                    "2026-08-07T12:00:06 [payload-c:stdout] sixth",
                ],
            ),
        ] {
            let mut writer = create_oplog_at(
                JournalRoot::open(temporary.path()).unwrap(),
                source,
                "run",
                OplogFormat::Log,
                opened,
            )
            .unwrap();
            for line in lines {
                writeln!(writer, "{line}").unwrap();
            }
        }
        let query = HealthLogsQuery {
            count: 10,
            since: None,
            service: None,
            grep: None,
        };
        let rows = collect_health_logs(temporary.path(), opened.naive_local(), &query).unwrap();
        assert_eq!(
            rows.iter()
                .map(|row| row.message.as_str())
                .collect::<Vec<_>>(),
            ["first", "second", "third", "fourth", "fifth", "sixth"]
        );
        assert!(rows.iter().all(|row| !row.raw.contains("oplog--")));
    }
}
