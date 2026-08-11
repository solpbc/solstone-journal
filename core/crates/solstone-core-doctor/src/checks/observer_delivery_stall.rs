// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use crate::{
    checks::common,
    context::CheckContext,
    vocabulary::{Check, RunnerResult, Status, make_result, truncate},
};
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
    let assessed = records
        .iter()
        .filter_map(|record| {
            solstone_core_observer::delivery_divergence(
                record,
                context.now.timestamp_millis(),
                solstone_core_observer::OBSERVER_STALE_MS,
            )
        })
        .collect::<Vec<_>>();
    if assessed.is_empty() {
        return Ok(make_result(
            check,
            Status::Ok,
            "delivery could not be assessed for any observer",
            None::<String>,
        ));
    }
    let failing = records
        .iter()
        .filter_map(|record| {
            solstone_core_observer::delivery_divergence(
                record,
                context.now.timestamp_millis(),
                solstone_core_observer::OBSERVER_STALE_MS,
            )
            .filter(|facts| {
                facts.last_segment_received_age_ms
                    > solstone_core_observer::OBSERVER_DELIVERY_STALL_MS
            })
            .map(|facts| delivery_clause(record, &facts))
        })
        .collect::<Vec<_>>();
    if failing.is_empty() {
        Ok(make_result(
            check,
            Status::Ok,
            "every observer is delivering",
            None::<String>,
        ))
    } else {
        Ok(make_result(
            check,
            Status::Warn,
            truncate(&failing.join("; "), 400),
            Some("restart the observer, then confirm a new upload lands"),
        ))
    }
}

fn delivery_clause(
    record: &solstone_core_observer::store::record::ObserverRecord,
    facts: &solstone_core_observer::DeliveryDivergence,
) -> String {
    let clause = format!(
        "observer {} is reaching the journal; last reach {}m ago, last upload landed {}m ago",
        facts.name,
        facts.last_seen_age_ms / 60_000,
        facts.last_segment_received_age_ms / 60_000
    );
    if let Some(duplicates) = record
        .stats()
        .and_then(|stats| stats.get("duplicates_rejected"))
        .and_then(serde_json::Value::as_i64)
        .filter(|count| *count > 0)
    {
        return format!(
            "{clause}; prior duplicate responses={duplicates}, so repeated uploads may be landing without a newer upload"
        );
    }
    if let Some(pending) = record
        .health_beacon()
        .and_then(|beacon| beacon.get("pending_queue_depth"))
        .and_then(serde_json::Value::as_i64)
    {
        return format!("{clause}; pending queue depth {pending}, so uploads may not be landing");
    }
    format!("{clause}; uploads may not be landing")
}
