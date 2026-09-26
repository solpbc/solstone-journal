// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use crate::{
    checks::directory_access::usable,
    context::CheckContext,
    vocabulary::{Check, ExecutionError, Platform, RunnerResult, Status, make_result},
};
pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
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
        // The Windows service is a Task Scheduler registration, which has no
        // directory; what the service keeps on disk is the installation
        // binding under the owner's local application data.
        Platform::Windows => match windows_service_config_dir() {
            Ok(path) => path,
            Err(error) => {
                return Err(ExecutionError {
                    kind: "OwnerBaseError".into(),
                    message: error,
                });
            }
        },
    };
    if !usable(home) {
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
    if config.exists() && !usable(&config) {
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

#[cfg(windows)]
fn windows_service_config_dir() -> Result<std::path::PathBuf, String> {
    solstone_core_installation_identity::owner_base()
        .map(|base| base.path())
        .map_err(|error| format!("could not locate the owner base: {error}"))
}

#[cfg(not(windows))]
fn windows_service_config_dir() -> Result<std::path::PathBuf, String> {
    Err("the Windows owner base is only readable on Windows".to_owned())
}
