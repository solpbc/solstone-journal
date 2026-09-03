// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use crate::{
    context::CheckContext,
    vocabulary::{Check, RunnerResult, Status, make_result},
};
fn writable(path: &std::path::Path) -> bool {
    #[cfg(unix)]
    {
        nix::unistd::access(path, nix::unistd::AccessFlags::W_OK).is_ok()
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        false
    }
}
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
    if context.platform == crate::vocabulary::Platform::Windows {
        return Ok(make_result(
            check,
            Status::Skip,
            "not supported on windows",
            None::<String>,
        ));
    }
    let path = &context.journal_path;
    if path.is_dir() {
        return Ok(if writable(path) {
            make_result(
                check,
                Status::Ok,
                format!("journal dir writable: {}", path.display()),
                None::<String>,
            )
        } else {
            make_result(
                check,
                Status::Fail,
                format!("journal dir not writable: {}", path.display()),
                Some(format!("fix ownership/permissions of {}", path.display())),
            )
        });
    }
    if path.exists() {
        return Ok(make_result(
            check,
            Status::Fail,
            format!(
                "journal path exists but is not a directory: {}",
                path.display()
            ),
            Some(format!("move or remove {}, then re-run", path.display())),
        ));
    }
    let parent = ancestor(path);
    Ok(if parent.is_dir() && writable(parent) {
        make_result(
            check,
            Status::Ok,
            format!(
                "journal dir absent; parent {} is writable",
                parent.display()
            ),
            None::<String>,
        )
    } else {
        make_result(
            check,
            Status::Fail,
            format!(
                "journal dir absent; nearest existing ancestor is not writable: {}",
                parent.display()
            ),
            Some(format!("fix ownership/permissions of {}", parent.display())),
        )
    })
}
pub fn journal(context: &CheckContext, check: Check) -> RunnerResult {
    if !context.journal_path.exists() {
        Ok(make_result(
            check,
            Status::Skip,
            "no local journal",
            None::<String>,
        ))
    } else {
        shared(context, check)
    }
}
pub fn readiness(context: &CheckContext, check: Check) -> RunnerResult {
    shared(context, check)
}
