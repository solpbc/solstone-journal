// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::{
    context::CheckContext,
    vocabulary::{Check, RunnerResult, Status, make_result},
};
use solstone_core_cli_copy as copy;
pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
    let result = context.speakers_analyze_resolvers.map_or_else(
        || {
            solstone_core_transcribe::check_speakers_analyze_installation_in(
                context.report_form.locale(),
            )
        },
        |(binary, model)| {
            solstone_core_transcribe::check_speakers_analyze_installation_with(binary, model)
        },
    );
    match result {
        Ok(()) => Ok(make_result(
            check,
            Status::Ok,
            copy::text("journal.cli.doctor.speakers.ready"),
            None::<String>,
        )),
        Err(error) => Ok(make_result(
            check,
            Status::Fail,
            solstone_core_transcribe::speakers_installation_detail_in(
                &error,
                context.report_form.locale(),
            )
            .unwrap_or_else(|| copy::text("journal.cli.doctor.speakers.unavailable")),
            Some(solstone_core_transcribe::speakers_repair_in(
                context.report_form.locale(),
            )),
        )),
    }
}
