// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::{
    checks::common,
    context::CheckContext,
    vocabulary::{Check, RunnerResult, Status, make_result, truncate},
};
use solstone_core_cli_copy as copy;

use solstone_core_sol_link::client_status::SourceDelivery;
use solstone_core_sol_link::ledger::IngestRejection;

fn rejection_date(rejection: &IngestRejection) -> String {
    solstone_core_time::parse_rfc3339(&rejection.first)
        .ok()
        .map(|timestamp| timestamp.strftime("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "unknown".into())
}

fn format_rejection(device: &str, rejection: &IngestRejection) -> String {
    copy::display(
        "journal.cli.doctor.client.ingest.health.rejection",
        [
            ("device", &device),
            ("reason", &rejection.reason_code),
            ("attempts", &rejection.active_count),
            ("since", &(rejection_date(rejection))),
        ],
    )
}

pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
    let records = match common::clients(context) {
        Ok(records) => common::enabled(records),
        Err(error) => {
            return Ok(make_result(
                check,
                Status::Skip,
                copy::display(
                    "journal.cli.doctor.devices.unavailable",
                    [("error", &error)],
                ),
                None::<String>,
            ));
        }
    };
    if records.is_empty() {
        return Ok(make_result(
            check,
            Status::Skip,
            copy::text("journal.cli.doctor.devices.absent"),
            None::<String>,
        ));
    }
    let mut failures = Vec::new();
    for record in &records {
        if let Some(rejection) = record.ingest_rejection.as_ref() {
            failures.push(format_rejection(&record.cid, rejection));
        }
        let mut skipped_top = false;
        for delivery in record.source_delivery.values() {
            if delivery.state != SourceDelivery::NeedsAttention {
                continue;
            }
            let Some(rejection) = delivery.ingest_rejection.as_ref() else {
                continue;
            };
            if !skipped_top && record.ingest_rejection.as_ref() == Some(rejection) {
                skipped_top = true;
                continue;
            }
            failures.push(format_rejection(&record.cid, rejection));
        }
    }
    if failures.is_empty() {
        Ok(make_result(
            check,
            Status::Ok,
            copy::text("journal.cli.doctor.client.ingest.health.healthy"),
            None::<String>,
        ))
    } else {
        Ok(make_result(
            check,
            Status::Warn,
            truncate(&failures.join("; "), 400),
            Some(copy::text("journal.cli.doctor.client.ingest.health.repair")),
        ))
    }
}
