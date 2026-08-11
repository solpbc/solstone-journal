// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use crate::{
    context::CheckContext,
    features,
    vocabulary::{Check, RunnerResult, Status, make_result},
};
pub fn run(name: &'static str, context: &CheckContext, check: Check) -> RunnerResult {
    let feature = features::find(name).expect("registry feature exists");
    let Some(environment) = context.python_env_root.as_deref() else {
        return Ok(make_result(
            check,
            Status::Skip,
            "Python environment unavailable; feature availability cannot be inspected without an interpreter",
            None::<String>,
        ));
    };
    if features::available(feature, environment) {
        Ok(make_result(
            check,
            Status::Ok,
            format!("{} available", feature.summary),
            None::<String>,
        ))
    } else {
        Ok(make_result(
            check,
            Status::Warn,
            format!("{} not installed", feature.summary),
            Some(features::hint(feature, context.platform)),
        ))
    }
}
