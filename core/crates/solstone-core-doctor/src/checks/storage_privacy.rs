// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::{
    context::CheckContext,
    vocabulary::{Check, RunnerResult, Status, make_result},
};
use solstone_core_system::storage_privacy::{UNAVAILABLE, inspect};
pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
    let (status, message) = match inspect(&context.journal_path) {
        Ok(privacy) => (
            if privacy.private || privacy.consented {
                Status::Ok
            } else {
                Status::Warn
            },
            if !privacy.private && !privacy.consented {
                format!(
                    "{} to keep it on this drive, choose that on your journal's health page.",
                    privacy.message
                )
            } else {
                privacy.message.to_owned()
            },
        ),
        Err(error) => {
            log::warn!("storage privacy: {error}");
            (Status::Warn, UNAVAILABLE.to_owned())
        }
    };
    Ok(make_result(check, status, message, None::<String>))
}
