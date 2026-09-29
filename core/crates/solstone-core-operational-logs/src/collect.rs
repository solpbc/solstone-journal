// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use chrono::{DateTime, Local, NaiveDate, NaiveDateTime, Utc};
use solstone_core_journal_io::{
    JournalRoot,
    operational_log::{OplogCatalogEntry, OplogFormat, catalog_oplogs, fold_oplogs},
};
use solstone_core_system::operational_log_parse::{ParsedHealthLogRow, parse_health_log_row};
use solstone_core_system_health::GrepPattern;

use crate::error::CollectError;

/// Fully resolved options for a one-shot operational-log read.
#[derive(Debug, Clone)]
pub struct HealthLogsQuery {
    pub count: i64,
    pub since: Option<NaiveDateTime>,
    pub service: Option<String>,
    pub grep: Option<GrepPattern>,
}

/// Raw source-filtered tail plus the descriptor frontiers that produced it.
///
/// The retained descriptors are intentionally opaque: callers can render the
/// bytes and then transfer the exact descriptors to [`crate::run_follow_from_snapshot`]
/// without reopening by path.
pub struct SourceTailSnapshot {
    source_slug: String,
    tail: Vec<u8>,
    entries: Vec<(OplogCatalogEntry, File, u64)>,
}

impl SourceTailSnapshot {
    /// The source coordinate used to retain this snapshot.
    pub fn source_slug(&self) -> &str {
        &self.source_slug
    }

    /// Raw payload bytes retained for the one-shot tail.
    pub fn tail(&self) -> &[u8] {
        &self.tail
    }

    /// Whether at least one canonical source leaf was retained.
    pub fn has_descriptors(&self) -> bool {
        !self.entries.is_empty()
    }

    pub(crate) fn into_catalogued_frontiers(self) -> Vec<(OplogCatalogEntry, File, u64)> {
        self.entries
    }
}

/// Collect the current and prior local day's raw `.log` tail for one canonical
/// source and retain its exact descriptor frontiers for a subsequent follow
/// handoff.
pub fn collect_source_tail_snapshot(
    journal_root: &Path,
    now: NaiveDateTime,
    source_slug: &str,
    tail_byte_limit: usize,
) -> Result<SourceTailSnapshot, CollectError> {
    let root = JournalRoot::open(journal_root).map_err(|_| CollectError::Root)?;
    let today = now.date();
    let days = today
        .pred_opt()
        .map_or_else(|| vec![today], |previous| vec![previous, today]);
    let snapshot = catalog_oplogs(root, &days).map_err(CollectError::Catalog)?;
    let mut tail = Vec::new();
    let mut entries = Vec::new();
    for (entry, mut file) in snapshot.into_catalogued_entries() {
        if entry.name().source().display_slug() != source_slug
            || entry.name().format() != OplogFormat::Log
        {
            continue;
        }
        let frontier = file.metadata().map_err(|_| CollectError::CatalogIo)?.len();
        let payload_offset = entry.payload_offset() as u64;
        if frontier < payload_offset {
            return Err(CollectError::CatalogIo);
        }
        let start = frontier
            .saturating_sub(u64::try_from(tail_byte_limit).unwrap_or(u64::MAX))
            .max(payload_offset);
        file.seek(SeekFrom::Start(start))
            .map_err(|_| CollectError::CatalogIo)?;
        let byte_count = usize::try_from(frontier - start).map_err(|_| CollectError::CatalogIo)?;
        let mut bytes = vec![0; byte_count];
        file.read_exact(&mut bytes)
            .map_err(|_| CollectError::CatalogIo)?;
        tail.extend(bytes);
        if tail.len() > tail_byte_limit {
            let retained_start = tail.len() - tail_byte_limit;
            tail.drain(..retained_start);
        }
        entries.push((entry, file, frontier));
    }
    Ok(SourceTailSnapshot {
        source_slug: source_slug.to_owned(),
        tail,
        entries,
    })
}

/// The source a supervisor's own captured stdout and stderr is written under.
const SERVICE_CAPTURE_SOURCE: &str = "service";

/// Collect and order canonical operational logs without rendering them.
///
/// Reads today's logs, or every day from `--since` through today.
pub fn collect_health_logs(
    journal_root: &Path,
    now: NaiveDateTime,
    query: &HealthLogsQuery,
) -> Result<Vec<ParsedHealthLogRow>, CollectError> {
    let root = JournalRoot::open(journal_root).map_err(|_| CollectError::Root)?;
    let mut rows = fold_oplogs(
        root,
        &window_days(now.date(), query.since),
        |rows: &mut Vec<ParsedHealthLogRow>, entry, input| {
            let source = entry.name().source().display_slug();
            if query
                .service
                .as_deref()
                .is_some_and(|service| !service.is_empty())
                && query.service.as_deref() != Some(source)
            {
                return Ok(());
            }
            let mut text = String::new();
            input.read_to_string(&mut text)?;
            let mut captured = (source == SERVICE_CAPTURE_SOURCE
                && entry.name().format() == OplogFormat::Log)
                .then(|| CapturedOutput::new(entry.name().opened_utc(), source));
            for raw in splitlines(&text) {
                let row = match captured.as_mut() {
                    Some(captured) => Some(captured.row(raw)),
                    None => parse_health_log_row(&raw),
                };
                if let Some(row) = row
                    && matches_filters(&row, query)
                {
                    rows.push(row);
                }
            }
            Ok(())
        },
    )
    .map_err(CollectError::Catalog)?;

    rows.sort_by_key(|row| row.timestamp);
    Ok(tail_slice(rows, query.count))
}

fn window_days(today: NaiveDate, since: Option<NaiveDateTime>) -> Vec<NaiveDate> {
    let first = since.map_or(today, |since| since.date().min(today));
    first.iter_days().take_while(|day| *day <= today).collect()
}

/// Rows for a supervisor's captured output, which is raw process bytes rather
/// than canonical rows: the supervisor's own logger lines and bare prints.
///
/// A logger line carries its own UTC timestamp. Any other line takes the time of
/// the line before it, or the leaf's opening time, so a bare refusal or a
/// multi-line error stays in place and is still reachable by `--since`.
struct CapturedOutput {
    service: String,
    last: Option<NaiveDateTime>,
}

impl CapturedOutput {
    fn new(opened_utc: &str, service: &str) -> Self {
        Self {
            service: service.to_owned(),
            last: NaiveDateTime::parse_from_str(opened_utc, "%Y%m%dT%H%M%S%.fZ")
                .ok()
                .map(|opened| local_time(opened.and_utc())),
        }
    }

    fn row(&mut self, raw: String) -> ParsedHealthLogRow {
        if let Some(row) = parse_health_log_row(&raw) {
            self.last = Some(row.timestamp);
            return row;
        }
        let timestamp = logger_timestamp(&raw).or(self.last).unwrap_or_default();
        self.last = Some(timestamp);
        ParsedHealthLogRow {
            timestamp,
            service: self.service.clone(),
            stream: "output".to_owned(),
            message: raw.clone(),
            raw,
        }
    }
}

/// The timestamp in the default `env_logger` header the supervisor writes:
/// `[2026-09-29T15:41:17Z WARN  target] message`.
fn logger_timestamp(line: &str) -> Option<NaiveDateTime> {
    let (stamp, rest) = line.strip_prefix('[')?.split_once(' ')?;
    let level = rest.split_whitespace().next()?;
    if !matches!(level, "ERROR" | "WARN" | "INFO" | "DEBUG" | "TRACE") || !rest.contains(']') {
        return None;
    }
    DateTime::parse_from_rfc3339(stamp)
        .ok()
        .map(|stamp| local_time(stamp.with_timezone(&Utc)))
}

fn local_time(instant: DateTime<Utc>) -> NaiveDateTime {
    instant.with_timezone(&Local).naive_local()
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

fn tail_slice<T>(mut lines: Vec<T>, count: i64) -> Vec<T> {
    let start = if count == 0 {
        0
    } else if count > 0 {
        lines.len().saturating_sub(count as usize)
    } else {
        lines.len().min(count.unsigned_abs() as usize)
    };
    lines.drain(..start);
    lines
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
    fn source_tail_snapshot_retains_previous_day_bytes_and_frontiers() {
        use std::io::Write;

        let temporary = tempfile::tempdir().unwrap();
        let offset = FixedOffset::east_opt(0).unwrap();
        let previous = offset
            .with_ymd_and_hms(2026, 8, 7, 23, 59, 59)
            .single()
            .unwrap();
        let today = offset
            .with_ymd_and_hms(2026, 8, 8, 0, 0, 1)
            .single()
            .unwrap();
        for (opened, bytes) in [
            (previous, b"before midnight\n".as_slice()),
            (today, b"after midnight\n".as_slice()),
        ] {
            let mut writer = create_oplog_at(
                JournalRoot::open(temporary.path()).unwrap(),
                "service",
                "supervisor",
                OplogFormat::Log,
                opened,
            )
            .unwrap();
            writer.write_all(bytes).unwrap();
        }

        let snapshot =
            collect_source_tail_snapshot(temporary.path(), today.naive_local(), "service", 1024)
                .unwrap();

        assert_eq!(snapshot.tail(), b"before midnight\nafter midnight\n");
        assert_eq!(snapshot.entries.len(), 2);
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

    #[test]
    fn unfiltered_count_is_applied_once_after_all_canonical_segments_merge() {
        use std::io::Write;

        let temporary = tempfile::tempdir().unwrap();
        let opened = FixedOffset::east_opt(0)
            .unwrap()
            .with_ymd_and_hms(2026, 8, 7, 12, 0, 0)
            .single()
            .unwrap();
        for (source, seconds) in [
            ("alpha", [1, 4, 7]),
            ("beta", [2, 5, 8]),
            ("gamma", [3, 6, 9]),
        ] {
            let mut writer = create_oplog_at(
                JournalRoot::open(temporary.path()).unwrap(),
                source,
                "run",
                OplogFormat::Log,
                opened,
            )
            .unwrap();
            for second in seconds {
                writeln!(
                    writer,
                    "2026-08-07T12:00:{second:02} [{source}:stdout] {source}-{second}"
                )
                .unwrap();
            }
        }
        let query = HealthLogsQuery {
            count: 2,
            since: None,
            service: None,
            grep: None,
        };
        let rows = collect_health_logs(temporary.path(), opened.naive_local(), &query).unwrap();
        assert_eq!(
            rows.iter()
                .map(|row| row.message.as_str())
                .collect::<Vec<_>>(),
            ["beta-8", "gamma-9"]
        );
    }

    #[test]
    fn source_tail_snapshot_filters_sources_and_skips_admission_bytes() {
        use std::io::Write;

        let temporary = tempfile::tempdir().unwrap();
        let opened = FixedOffset::east_opt(0)
            .unwrap()
            .with_ymd_and_hms(2026, 8, 7, 12, 0, 0)
            .single()
            .unwrap();
        for (source, payload) in [
            ("service", b"stdout\nstderr\n".as_slice()),
            ("other", b"hidden\n"),
        ] {
            let mut writer = create_oplog_at(
                JournalRoot::open(temporary.path()).unwrap(),
                source,
                "run",
                OplogFormat::Log,
                opened,
            )
            .unwrap();
            writer.write_all(payload).unwrap();
        }
        let snapshot =
            collect_source_tail_snapshot(temporary.path(), opened.naive_local(), "service", 32)
                .unwrap();
        assert!(snapshot.has_descriptors());
        assert_eq!(snapshot.source_slug(), "service");
        assert_eq!(snapshot.tail(), b"stdout\nstderr\n");
    }

    /// Captured from a hosted `journal start` refused by a journal with no saved
    /// binding, run with `RUST_LOG=trace` so the logger wrote a line too.
    const CAPTURED_SUPERVISOR_OUTPUT: &[u8] = b"[2026-09-29T15:41:17Z TRACE mio::poll] registering event source with poller: token=Token(1), interests=READABLE\n\
this installation couldn't be verified.\n\
run `journal setup` to check it. if setup finishes successfully, try again.\n\
details: saved binding: open storage directory: No such file or directory (os error 2)\n";

    fn write_leaf(
        journal: &Path,
        source: &str,
        opened: chrono::DateTime<chrono::FixedOffset>,
        bytes: &[u8],
    ) {
        use std::io::Write;

        let mut writer = create_oplog_at(
            JournalRoot::open(journal).unwrap(),
            source,
            "supervisor",
            OplogFormat::Log,
            opened,
        )
        .unwrap();
        writer.write_all(bytes).unwrap();
    }

    fn query(since: Option<NaiveDateTime>, grep: Option<&str>) -> HealthLogsQuery {
        HealthLogsQuery {
            count: 50,
            since,
            service: None,
            grep: grep
                .map(|pattern| solstone_core_system_health::compile_grep_pattern(pattern).unwrap()),
        }
    }

    fn raws(rows: &[ParsedHealthLogRow]) -> Vec<&str> {
        rows.iter().map(|row| row.raw.as_str()).collect()
    }

    #[test]
    fn a_captured_supervisors_logger_and_bare_lines_are_rows() {
        let temporary = tempfile::tempdir().unwrap();
        let opened = FixedOffset::east_opt(0)
            .unwrap()
            .with_ymd_and_hms(2026, 9, 29, 15, 41, 17)
            .single()
            .unwrap();
        write_leaf(
            temporary.path(),
            "service",
            opened,
            CAPTURED_SUPERVISOR_OUTPUT,
        );
        let now = opened.naive_local();

        let rows = collect_health_logs(temporary.path(), now, &query(None, None)).unwrap();
        let expected = String::from_utf8(CAPTURED_SUPERVISOR_OUTPUT.to_vec()).unwrap();
        assert_eq!(raws(&rows), expected.lines().collect::<Vec<_>>());
        let logged = local_time(opened.with_timezone(&Utc));
        assert!(rows.iter().all(|row| row.timestamp == logged));
        assert!(rows.iter().all(|row| row.service == "service"));

        let refused =
            collect_health_logs(temporary.path(), now, &query(None, Some("verified"))).unwrap();
        assert_eq!(raws(&refused), ["this installation couldn't be verified."]);
        let since = collect_health_logs(temporary.path(), now, &query(Some(logged), None)).unwrap();
        assert_eq!(since.len(), 4);
        let later = logged + chrono::TimeDelta::seconds(1);
        assert!(
            collect_health_logs(temporary.path(), now, &query(Some(later), None))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_bare_line_with_nothing_before_it_takes_the_leafs_opening_time() {
        let temporary = tempfile::tempdir().unwrap();
        let opened = FixedOffset::east_opt(0)
            .unwrap()
            .with_ymd_and_hms(2026, 9, 29, 12, 0, 0)
            .single()
            .unwrap();
        write_leaf(temporary.path(), "service", opened, b"bare refusal\n");
        write_leaf(
            temporary.path(),
            "other",
            opened,
            b"bare refusal elsewhere\n",
        );

        let rows = collect_health_logs(temporary.path(), opened.naive_local(), &query(None, None))
            .unwrap();
        assert_eq!(raws(&rows), ["bare refusal"]);
        assert_eq!(rows[0].timestamp, local_time(opened.with_timezone(&Utc)));
    }

    #[test]
    fn since_reaches_every_day_through_today() {
        let temporary = tempfile::tempdir().unwrap();
        let offset = FixedOffset::east_opt(0).unwrap();
        let earlier = offset
            .with_ymd_and_hms(2026, 9, 27, 23, 0, 0)
            .single()
            .unwrap();
        let today = offset
            .with_ymd_and_hms(2026, 9, 29, 1, 0, 0)
            .single()
            .unwrap();
        for (opened, line) in [
            (earlier, "2026-09-27T23:00:00 [sense:stderr] two days back"),
            (today, "2026-09-29T01:00:00 [sense:stderr] today"),
        ] {
            write_leaf(
                temporary.path(),
                "sense",
                opened,
                format!("{line}\n").as_bytes(),
            );
        }
        let now = today.naive_local();

        let unbounded = collect_health_logs(temporary.path(), now, &query(None, None)).unwrap();
        assert_eq!(
            raws(&unbounded),
            ["2026-09-29T01:00:00 [sense:stderr] today"]
        );
        let since = now - chrono::TimeDelta::days(2);
        let window = collect_health_logs(temporary.path(), now, &query(Some(since), None)).unwrap();
        assert_eq!(
            raws(&window),
            [
                "2026-09-27T23:00:00 [sense:stderr] two days back",
                "2026-09-29T01:00:00 [sense:stderr] today"
            ]
        );
    }
}
