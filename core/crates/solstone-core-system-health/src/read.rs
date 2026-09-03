// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_journal_io::{MalformedPolicy, read_jsonl_with_report};

use crate::{FoldRead, HealthError, HealthLogSource, RunLogRecord};

pub(crate) fn read_day_records<S: HealthLogSource>(
    source: &S,
    day: &str,
) -> Result<FoldRead<Vec<RunLogRecord>>, HealthError> {
    let mut paths = source.health_log_paths(day)?;
    paths.sort();
    let mut records: Vec<RunLogRecord> = Vec::new();
    let mut malformed_line_count = 0;
    for path in paths {
        let report = read_jsonl_with_report(&path, Vec::new(), MalformedPolicy::Skip)?;
        malformed_line_count += report.malformed_line_count;
        records.extend(report.records.into_iter().map(|record| record.value));
    }
    records.sort_by_key(|record| record.ts);
    Ok(FoldRead {
        value: records,
        malformed_line_count,
    })
}
