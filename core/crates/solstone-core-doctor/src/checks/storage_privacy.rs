// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::{
    context::CheckContext,
    vocabulary::{Check, RunnerResult, Status, make_result},
};
use solstone_core_system::storage_privacy::{
    drive_cannot_run_journal, inspect_in, unavailable_message,
};
pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
    let locale = solstone_core_window_catalog::cli_locale();
    if drive_cannot_run_journal(&context.journal_path) {
        return Ok(make_result(
            check,
            Status::Fail,
            solstone_core_window_catalog::text(locale, "journal.setup.drive.unsupported"),
            None::<String>,
        ));
    }
    let (status, message) = match inspect_in(&context.journal_path, locale) {
        Ok(privacy) => (
            if privacy.private || privacy.consented {
                Status::Ok
            } else {
                Status::Warn
            },
            if !privacy.private && !privacy.consented {
                solstone_core_cli_copy::text("journal.cli.doctor.storage.privacy.choice")
            } else {
                privacy.message
            },
        ),
        Err(error) => {
            let diagnostic = error
                .get_ref()
                .and_then(|cause| {
                    cause.downcast_ref::<solstone_core_journal_config::ConfigLoadError>()
                })
                .map_or_else(|| error.to_string(), super::common::config_error);
            log::warn!(
                "{}",
                solstone_core_cli_copy::display(
                    "journal.cli.doctor.storage.inspection.log",
                    [("error", &(diagnostic))]
                )
            );
            (Status::Warn, unavailable_message(locale))
        }
    };
    Ok(make_result(check, status, message, None::<String>))
}
