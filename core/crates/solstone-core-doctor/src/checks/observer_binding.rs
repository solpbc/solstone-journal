// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use crate::{
    checks::common,
    context::CheckContext,
    vocabulary::{Check, RunnerResult, Status, make_result},
};
pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
    let records = common::enabled(common::observers(context).unwrap_or_default());
    let unbound = records
        .iter()
        .filter(|record| record.device_binding_kind().is_none())
        .map(|record| record.name().unwrap_or("unknown"))
        .collect::<Vec<_>>();
    let detail = if unbound.is_empty() {
        format!("active device records={}; unbound=0", records.len())
    } else {
        format!(
            "active device records={}; unbound={}; streams={}",
            records.len(),
            unbound.len(),
            unbound.join(", ")
        )
    };
    Ok(make_result(check, Status::Ok, detail, None::<String>))
}
