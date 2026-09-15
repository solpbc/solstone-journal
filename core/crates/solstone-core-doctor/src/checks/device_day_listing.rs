// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use crate::{
    context::CheckContext,
    vocabulary::{Check, RunnerResult, Status, make_result, truncate},
};

use solstone_core_ingest::device_day_listing_faults;

/// Newest chronicle days the listing is run over, per bound device stream.
const RECENT_DAYS: usize = 14;

pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
    if !context.journal_path.is_dir() {
        return Ok(make_result(
            check,
            Status::Skip,
            "no local journal",
            None::<String>,
        ));
    }
    match device_day_listing_faults(&context.journal_path, RECENT_DAYS) {
        Err(error) => Ok(make_result(
            check,
            Status::Skip,
            format!("device streams unavailable: {error}"),
            None::<String>,
        )),
        Ok(None) => Ok(make_result(
            check,
            Status::Skip,
            "no device streams bound",
            None::<String>,
        )),
        Ok(Some(faults)) if faults.is_empty() => Ok(make_result(
            check,
            Status::Ok,
            format!("the journal can read the last {RECENT_DAYS} days for every bound device"),
            None::<String>,
        )),
        Ok(Some(faults)) => {
            let detail = faults
                .iter()
                .map(|fault| {
                    format!(
                        "day {} stream {} device {}: {}",
                        fault.day, fault.stream, fault.cid, fault.reason_code
                    )
                })
                .collect::<Vec<_>>()
                .join("; ");
            Ok(make_result(
                check,
                Status::Warn,
                truncate(&detail, 400),
                Some(
                    "a device syncing that day is told the journal cannot read it; a solstone app older than the 2026-09 fix shows that as offline; inspect the named stream directory's segments and their events.jsonl",
                ),
            ))
        }
    }
}
