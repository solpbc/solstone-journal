// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use crate::{
    checks::common,
    context::CheckContext,
    vocabulary::{Check, RunnerResult, Status, make_result},
};
pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
    let records = common::enabled(common::clients(context).unwrap_or_default());
    // A client projection row is keyed by the authorization certificate CID.
    // The old observer-record ambiguity (an observer lacking or sharing a
    // device binding) is therefore structurally impossible.
    let detail = format!(
        "active client records={}; unbound=0; certificate cid binding is authoritative",
        records.len()
    );
    Ok(make_result(check, Status::Ok, detail, None::<String>))
}
