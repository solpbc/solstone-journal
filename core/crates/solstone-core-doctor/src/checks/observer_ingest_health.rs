// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use crate::{
    checks::common,
    context::CheckContext,
    vocabulary::{Check, RunnerResult, Status, make_result, truncate},
};

fn rejection_date(rejection: &serde_json::Map<String, serde_json::Value>) -> String {
    rejection
        .get("first_ts")
        .and_then(serde_json::Value::as_f64)
        .filter(|timestamp| timestamp.is_finite())
        .and_then(|timestamp| chrono::DateTime::from_timestamp_millis(timestamp as i64))
        .map(|timestamp| timestamp.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "unknown".into())
}

pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
    let records = match common::observers(context) {
        Ok(records) => common::enabled(records),
        Err(error) => {
            return Ok(make_result(
                check,
                Status::Skip,
                format!("observer records unavailable: {error}"),
                None::<String>,
            ));
        }
    };
    if records.is_empty() {
        return Ok(make_result(
            check,
            Status::Skip,
            "no registered observers",
            None::<String>,
        ));
    }
    let failures = records
        .iter()
        .filter_map(|record| {
            record.ingest_rejection().map(|rejection| {
                let version = rejection
                    .get("version")
                    .and_then(serde_json::Value::as_str)
                    .map(|value| format!("v{value}"))
                    .unwrap_or_else(|| "version unknown".into());
                format!(
                    "observer {} ({version}) failing ingest: {}, {}x since {}",
                    record.name().unwrap_or("unknown"),
                    rejection
                        .get("summary")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or(""),
                    rejection
                        .get("active_count")
                        .and_then(serde_json::Value::as_i64)
                        .unwrap_or(0),
                    rejection_date(rejection)
                )
            })
        })
        .collect::<Vec<_>>();
    if failures.is_empty() {
        Ok(make_result(
            check,
            Status::Ok,
            "no observers failing ingest",
            None::<String>,
        ))
    } else {
        Ok(make_result(
            check,
            Status::Warn,
            truncate(&failures.join("; "), 400),
            Some(
                "update or restart the observer, then confirm a valid upload clears the rejection",
            ),
        ))
    }
}
