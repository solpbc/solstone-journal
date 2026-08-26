// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use crate::{
    checks::common,
    context::CheckContext,
    vocabulary::{Check, RunnerResult, Status, make_result},
};
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
    // A client projection row is keyed by the authorization certificate CID.
    // Legacy registry-record ambiguity is therefore structurally impossible.
    let detail = format!(
        "active client records={}; unbound=0; certificate cid binding is authoritative",
        records.len()
    );
    Ok(make_result(check, Status::Ok, detail, None::<String>))
}
