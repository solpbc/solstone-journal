// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Shared segment-selection helpers for paired-peer export.

use std::fs;
use std::path::Path;

use chrono::{Duration as ChronoDuration, NaiveDate};

use crate::TransferError;

/// Segment control files that must never be included in a journal upload.
pub(crate) const RESERVED_SEGMENT_FILENAMES: [&str; 4] = [
    "stream.json",
    "ingest.json",
    "ingest.json.lock",
    "timeline.state.json",
];

/// Parse the paired-peer export day selector.
pub(crate) fn parse_day_spec(
    spec: Option<&str>,
    journal: &Path,
) -> Result<Vec<String>, TransferError> {
    let Some(spec) = spec else {
        let chronicle = journal.join("chronicle");
        let day_root = if chronicle.is_dir() {
            chronicle
        } else {
            journal.into()
        };
        let mut days = fs::read_dir(day_root)?
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                (entry.path().is_dir() && is_eight_digit_day(&name)).then_some(name)
            })
            .collect::<Vec<_>>();
        days.sort();
        return Ok(days);
    };
    if is_eight_digit_day(spec) {
        return Ok(vec![spec.to_string()]);
    }
    let Some((start, end)) = spec.split_once('-') else {
        return Err(TransferError::InvalidDay);
    };
    if !is_eight_digit_day(start) || !is_eight_digit_day(end) || end.contains('-') {
        return Err(TransferError::InvalidDay);
    }
    let start = parse_calendar_day(start)?;
    let end = parse_calendar_day(end)?;
    if start > end {
        return Err(TransferError::InvalidDay);
    }
    let mut days = Vec::new();
    let mut current = start;
    while current <= end {
        days.push(current.format("%Y%m%d").to_string());
        current += ChronoDuration::days(1);
    }
    Ok(days)
}

fn is_eight_digit_day(value: &str) -> bool {
    value.len() == 8 && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn parse_calendar_day(value: &str) -> Result<NaiveDate, TransferError> {
    NaiveDate::parse_from_str(value, "%Y%m%d").map_err(|_| TransferError::InvalidDay)
}

#[cfg(test)]
mod tests {
    use super::parse_day_spec;
    use crate::TransferError;
    use std::fs;

    #[test]
    fn day_parser_matches_python_shapes() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("chronicle/20260203")).unwrap();
        fs::create_dir_all(root.path().join("chronicle/invalid")).unwrap();
        assert_eq!(parse_day_spec(None, root.path()).unwrap(), ["20260203"]);
        assert_eq!(
            parse_day_spec(Some("20260230"), root.path()).unwrap(),
            ["20260230"]
        );
        assert_eq!(
            parse_day_spec(Some("20260227-20260301"), root.path()).unwrap(),
            ["20260227", "20260228", "20260301"]
        );
        assert!(matches!(
            parse_day_spec(Some("20260230-20260301"), root.path()),
            Err(TransferError::InvalidDay)
        ));
        assert!(matches!(
            parse_day_spec(Some("20260301-20260228"), root.path()),
            Err(TransferError::InvalidDay)
        ));
    }
}
