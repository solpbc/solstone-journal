// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use crate::{
    checks::common,
    context::CheckContext,
    vocabulary::{Check, RunnerResult, Status, make_result, truncate},
};

use solstone_core_sol_link::ledger::IngestRejection;

fn rejection_date(rejection: &IngestRejection) -> String {
    chrono::DateTime::parse_from_rfc3339(&rejection.first)
        .ok()
        .map(|timestamp| timestamp.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "unknown".into())
}

pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
    let records = match common::clients(context) {
        Ok(records) => common::enabled(records),
        Err(error) => {
            return Ok(make_result(
                check,
                Status::Skip,
                format!("device records unavailable: {error}"),
                None::<String>,
            ));
        }
    };
    if records.is_empty() {
        return Ok(make_result(
            check,
            Status::Skip,
            "no registered devices",
            None::<String>,
        ));
    }
    let failures = records
        .iter()
        .filter_map(|record| {
            record.ingest_rejection.as_ref().map(|rejection| {
                format!(
                    "device {} failing ingest: {}, {}x since {}",
                    record.cid,
                    rejection.reason_code,
                    rejection.active_count,
                    rejection_date(rejection)
                )
            })
        })
        .collect::<Vec<_>>();
    if failures.is_empty() {
        Ok(make_result(
            check,
            Status::Ok,
            "no devices failing ingest",
            None::<String>,
        ))
    } else {
        Ok(make_result(
            check,
            Status::Warn,
            truncate(&failures.join("; "), 400),
            Some("update or restart the device, then confirm a valid upload clears the rejection"),
        ))
    }
}
