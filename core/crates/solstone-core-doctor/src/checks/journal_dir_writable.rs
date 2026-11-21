// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::{
    checks::directory_access::writable,
    context::CheckContext,
    vocabulary::{Check, RunnerResult, Status, make_result},
};
use solstone_core_cli_copy as copy;
fn ancestor(path: &std::path::Path) -> &std::path::Path {
    let mut current = path;
    while !current.exists() {
        match current.parent() {
            Some(parent) if parent != current => current = parent,
            _ => break,
        }
    }
    current
}
pub fn shared(context: &CheckContext, check: Check) -> RunnerResult {
    let path = &context.journal_path;
    if path.is_dir() {
        return Ok(if writable(path) {
            make_result(
                check,
                Status::Ok,
                copy::display(
                    "journal.cli.doctor.journal.dir.writable.writable",
                    [("path", &path.display())],
                ),
                None::<String>,
            )
        } else {
            make_result(
                check,
                Status::Fail,
                copy::display(
                    "journal.cli.doctor.journal.dir.writable.unwritable",
                    [("path", &path.display())],
                ),
                Some(copy::display(
                    "journal.cli.doctor.path.repair",
                    [("path", &path.display())],
                )),
            )
        });
    }
    if path.exists() {
        return Ok(make_result(
            check,
            Status::Fail,
            copy::display(
                "journal.cli.doctor.journal.dir.writable.not.directory",
                [("path", &path.display())],
            ),
            Some(copy::display(
                "journal.cli.doctor.journal.dir.writable.move",
                [("path", &path.display())],
            )),
        ));
    }
    let parent = ancestor(path);
    Ok(if parent.is_dir() && writable(parent) {
        make_result(
            check,
            Status::Ok,
            copy::display(
                "journal.cli.doctor.journal.dir.writable.parent.writable",
                [("path", &parent.display())],
            ),
            None::<String>,
        )
    } else {
        make_result(
            check,
            Status::Fail,
            copy::display(
                "journal.cli.doctor.journal.dir.writable.parent.unwritable",
                [("path", &parent.display())],
            ),
            Some(copy::display(
                "journal.cli.doctor.path.repair",
                [("path", &parent.display())],
            )),
        )
    })
}
pub fn journal(context: &CheckContext, check: Check) -> RunnerResult {
    if !context.journal_path.exists() {
        Ok(make_result(
            check,
            Status::Skip,
            copy::text("journal.cli.doctor.journal.absent"),
            None::<String>,
        ))
    } else {
        shared(context, check)
    }
}
pub fn readiness(context: &CheckContext, check: Check) -> RunnerResult {
    shared(context, check)
}
