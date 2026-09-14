// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use crate::{
    checks::common,
    context::CheckContext,
    vocabulary::{Check, RunnerResult, Status, make_result, truncate},
};

use solstone_core_sol_link::ledger::TransportRefusal;

fn refusal_date(refusal: &TransportRefusal) -> String {
    chrono::DateTime::parse_from_rfc3339(&refusal.latest)
        .ok()
        .map(|timestamp| timestamp.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "unknown".into())
}

/// Reports streams this journal refused because a device's connection was
/// already carrying as many requests as it accepts at once.
///
/// It exists because that refusal is invisible everywhere else: it resets one
/// stream and leaves the connection up, so the device sees a closed stream with
/// no status and reports the same network error it reports when the network
/// genuinely dies. Without this an owner cannot tell a busy connection from a
/// dead one, and neither can we.
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
    let refused = records
        .iter()
        .filter_map(|record| {
            record.transport_refusal.as_ref().map(|refusal| {
                format!(
                    "device {} had requests turned away: {}, {}x through {}",
                    record.cid,
                    refusal.reason_code,
                    refusal.active_count,
                    refusal_date(refusal)
                )
            })
        })
        .collect::<Vec<_>>();
    if refused.is_empty() {
        Ok(make_result(
            check,
            Status::Ok,
            "no devices had requests turned away",
            None::<String>,
        ))
    } else {
        Ok(make_result(
            check,
            Status::Warn,
            truncate(&refused.join("; "), 400),
            // Deliberately not "restart the device": the refusal means the
            // device asked for more at once than one connection carries, and
            // the actionable half is knowing it was this rather than the
            // network. A device that keeps provoking it is a defect to report.
            Some("this is a busy connection, not a lost one; report it if it keeps happening"),
        ))
    }
}
