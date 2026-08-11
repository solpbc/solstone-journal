// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use crate::{
    context::CheckContext,
    vocabulary::{Check, RunnerResult, Status, make_result},
};
pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
    let result = context.speakers_analyze_resolvers.map_or_else(
        solstone_core_transcribe::check_speakers_analyze_installation,
        |(binary, model)| {
            solstone_core_transcribe::check_speakers_analyze_installation_with(binary, model)
        },
    );
    match result {
        Ok(()) => Ok(make_result(
            check,
            Status::Ok,
            "speakers-analyze installation ready",
            None::<String>,
        )),
        Err(error) => Ok(make_result(
            check,
            Status::Fail,
            error
                .message()
                .unwrap_or("speakers-analyze installation unavailable"),
            Some(solstone_core_transcribe::speakers_analyze_repair_text()),
        )),
    }
}
