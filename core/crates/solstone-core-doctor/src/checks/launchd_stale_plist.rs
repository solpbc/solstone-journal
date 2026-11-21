// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_cli_copy as copy;

use std::path::PathBuf;

use crate::{
    context::CheckContext,
    vocabulary::{Check, RunnerResult, Status, make_result},
};

const REPAIR: &str = "journal.cli.doctor.launchd.stale.plist.repair";

pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
    let path = context
        .home_dir
        .join("Library/LaunchAgents/org.solpbc.solstone.plist");
    if !path.exists() {
        return Ok(make_result(
            check,
            Status::Skip,
            copy::text("journal.cli.doctor.launchd.stale.plist.absent"),
            None::<String>,
        ));
    }
    let value = match plist::Value::from_file(&path) {
        Ok(value) => value,
        Err(error) => {
            return Ok(make_result(
                check,
                Status::Fail,
                copy::display(
                    "journal.cli.doctor.launchd.stale.plist.invalid",
                    [("error", &error)],
                ),
                Some(copy::text(REPAIR)),
            )
            .changing_service());
        }
    };
    let executable = value
        .as_dictionary()
        .and_then(|dict| dict.get("ProgramArguments"))
        .and_then(plist::Value::as_array)
        .and_then(|arguments| arguments.first())
        .and_then(plist::Value::as_string);
    let Some(executable) = executable else {
        return Ok(make_result(
            check,
            Status::Fail,
            copy::text("journal.cli.doctor.launchd.stale.plist.arguments.missing"),
            Some(copy::text(REPAIR)),
        )
        .changing_service());
    };
    let executable = PathBuf::from(executable);
    if !executable.exists() {
        return Ok(make_result(
            check,
            Status::Fail,
            copy::display(
                "journal.cli.doctor.launchd.stale.plist.target.missing",
                [("path", &executable.display())],
            ),
            Some(copy::text(REPAIR)),
        )
        .changing_service());
    }
    Ok(make_result(
        check,
        Status::Ok,
        copy::display(
            "journal.cli.doctor.launchd.stale.plist.target.exists",
            [("path", &executable.display())],
        ),
        None::<String>,
    ))
}
