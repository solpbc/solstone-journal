// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use crate::{
    context::CheckContext,
    vocabulary::{Check, Platform, RunnerResult, Status, make_result},
};
pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
    if context.platform == Platform::Windows {
        return Ok(make_result(
            check,
            Status::Skip,
            "not supported on windows",
            None::<String>,
        ));
    }
    let home = &context.home_dir;
    if !home.exists() {
        return Ok(make_result(
            check,
            Status::Fail,
            format!("home directory does not exist: {}", home.display()),
            Some(format!("fix ownership/permissions of {}", home.display())),
        ));
    }
    let config = match context.platform {
        Platform::Darwin => home.join("Library/LaunchAgents"),
        Platform::Linux => home.join(".config"),
        Platform::Windows => {
            return Ok(make_result(
                check,
                Status::Skip,
                "not supported on windows",
                None::<String>,
            ));
        }
    };
    if !accessible(home) {
        return Ok(make_result(
            check,
            Status::Fail,
            format!(
                "home directory is not readable and writable: {}",
                home.display()
            ),
            Some(format!("fix ownership/permissions of {}", home.display())),
        ));
    }
    if config.exists() && !accessible(&config) {
        return Ok(make_result(
            check,
            Status::Fail,
            format!(
                "service config directory is not writable: {}",
                config.display()
            ),
            Some(format!("fix ownership/permissions of {}", config.display())),
        ));
    }
    let detail = if config.exists() {
        format!(
            "home and service config dir are writable ({})",
            config.display()
        )
    } else {
        format!("home is writable; install will create {}", config.display())
    };
    Ok(make_result(check, Status::Ok, detail, None::<String>))
}
fn accessible(path: &std::path::Path) -> bool {
    #[cfg(unix)]
    {
        nix::unistd::access(
            path,
            nix::unistd::AccessFlags::R_OK
                | nix::unistd::AccessFlags::W_OK
                | nix::unistd::AccessFlags::X_OK,
        )
        .is_ok()
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        false
    }
}
