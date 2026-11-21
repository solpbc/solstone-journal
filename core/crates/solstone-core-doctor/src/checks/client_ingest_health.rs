// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::{
    checks::common,
    context::CheckContext,
    vocabulary::{Check, RunnerResult, Status, make_result, truncate},
};
use solstone_core_cli_copy as copy;

use solstone_core_sol_link::ledger::IngestRejection;

fn rejection_date(rejection: &IngestRejection) -> String {
    solstone_core_time::parse_rfc3339(&rejection.first)
        .ok()
        .map(|timestamp| timestamp.strftime("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "unknown".into())
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
    let failures = records
        .iter()
        .filter_map(|record| {
            record.ingest_rejection.as_ref().map(|rejection| {
                copy::display(
                    "journal.cli.doctor.client.ingest.health.rejection",
                    [
                        ("device", &record.cid),
                        ("reason", &rejection.reason_code),
                        ("attempts", &rejection.active_count),
                        ("since", &(rejection_date(rejection))),
                    ],
                )
            })
        })
        .collect::<Vec<_>>();
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
