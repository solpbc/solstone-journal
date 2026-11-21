// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::{
    checks::directory_access::usable,
    context::CheckContext,
    vocabulary::{Check, ExecutionError, Platform, RunnerResult, Status, make_result},
};
use solstone_core_cli_copy as copy;
pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
    let home = &context.home_dir;
    if !home.exists() {
        return Ok(make_result(
            check,
            Status::Fail,
            copy::display(
                "journal.cli.doctor.config.dir.readable.home.missing",
                [("path", &home.display())],
            ),
            Some(copy::display(
                "journal.cli.doctor.path.repair",
                [("path", &home.display())],
            )),
        ));
    }
    let config = match context.platform {
        Platform::Darwin => home.join("Library/LaunchAgents"),
        Platform::Linux => home.join(".config"),
        // The Windows service is a Task Scheduler registration, which has no
        // directory; what the service keeps on disk is the installation
        // binding under the owner's local application data.
        Platform::Windows => match windows_service_config_dir() {
            Ok(path) => path,
            Err(error) => {
                return Err(ExecutionError::localized("OwnerBaseError", error));
            }
        },
    };
    if !usable(home) {
        return Ok(make_result(
            check,
            Status::Fail,
            copy::display(
                "journal.cli.doctor.config.dir.readable.home.unusable",
                [("path", &home.display())],
            ),
            Some(copy::display(
                "journal.cli.doctor.path.repair",
                [("path", &home.display())],
            )),
        ));
    }
    if config.exists() && !usable(&config) {
        return Ok(make_result(
            check,
            Status::Fail,
            copy::display(
                "journal.cli.doctor.config.dir.readable.config.unwritable",
                [("path", &config.display())],
            ),
            Some(copy::display(
                "journal.cli.doctor.path.repair",
                [("path", &config.display())],
            )),
        ));
    }
    let detail = if config.exists() {
        copy::display(
            "journal.cli.doctor.config.dir.readable.config.writable",
            [("path", &config.display())],
        )
    } else {
        copy::display(
            "journal.cli.doctor.config.dir.readable.config.will.create",
            [("path", &config.display())],
        )
    };
    Ok(make_result(check, Status::Ok, detail, None::<String>))
}

#[cfg(windows)]
fn windows_service_config_dir() -> Result<std::path::PathBuf, copy::Diagnostic> {
    solstone_core_installation_identity::owner_base()
        .map(|base| base.path())
        .map_err(|error| {
            copy::Diagnostic::new(
                "journal.cli.doctor.config.dir.readable.owner.unavailable",
                [("error", &error)],
            )
        })
}

#[cfg(not(windows))]
fn windows_service_config_dir() -> Result<std::path::PathBuf, copy::Diagnostic> {
    Err(copy::Diagnostic::new(
        "journal.cli.doctor.config.dir.readable.owner.windows",
        [],
    ))
}
