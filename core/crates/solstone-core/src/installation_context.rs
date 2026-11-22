// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Shared executable/home-derived installation identity inputs.

use std::path::PathBuf;

use solstone_core_installation_identity::{IdentityError, OwnerBase, PlatformTag};
use solstone_core_journal::resolve_identity_root_from_executable_dir;

pub fn project_root_from_current_executable() -> Option<PathBuf> {
    std::env::current_exe().ok().and_then(|executable| {
        executable
            .ancestors()
            .find(|path| path.join("pyproject.toml").is_file())
            .map(std::path::Path::to_path_buf)
    })
}

pub fn identity_root_from_current_executable() -> Result<PathBuf, String> {
    identity_root_from_current_executable_in(solstone_core_cli_copy::ENGLISH)
}

pub fn identity_root_from_current_executable_in(locale: &str) -> Result<PathBuf, String> {
    let executable = std::env::current_exe().map_err(|error| {
        solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.host.installation.executable",
            [("error", &(error))],
        )
    })?;
    let executable_dir = executable.parent().ok_or_else(|| {
        solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.host.installation.executable.parent",
            &[],
        )
    })?;
    let resolved = resolve_identity_root_from_executable_dir(executable_dir)
        .or_else(project_root_from_current_executable);
    #[cfg(debug_assertions)]
    let resolved = resolved.or_else(supervisor_fixture_project_root);
    resolved.ok_or_else(|| {
        solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.host.installation.root",
            [("directory", &(executable_dir.display()))],
        )
    })
}

/// Integration-test-only root seam for binaries emitted to an external Cargo
/// target directory. Release builds do not compile this fallback, and debug
/// builds require the explicit supervisor app-fixture gate.
#[cfg(debug_assertions)]
fn supervisor_fixture_project_root() -> Option<PathBuf> {
    (std::env::var("SOLSTONE_SUPERVISOR_APP_FIXTURE").as_deref() == Ok("1"))
        .then(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.."))
        .and_then(|root| root.canonicalize().ok())
}

pub fn owner_base_at_home(home: PathBuf) -> Result<OwnerBase, IdentityError> {
    OwnerBase::at_home(home, PlatformTag::current())
}

/// Locked recovery guidance shared by direct service and hosted-supervisor
/// installation-binding refusals.
pub fn installation_recovery_copy(detail: &str) -> String {
    installation_recovery_copy_for(solstone_core_cli_copy::ENGLISH, detail)
}

pub fn installation_recovery_copy_for(locale: &str, detail: &str) -> String {
    let detail = solstone_core_system_health::sanitize_str_for_terminal_bounded(detail);
    solstone_core_cli_copy::display_for(
        locale,
        "journal.cli.host.installation.recovery",
        [("detail", &(detail))],
    )
}

#[cfg(test)]
mod tests {
    use super::installation_recovery_copy;

    #[test]
    fn installation_recovery_detail_is_sanitized_and_bounded() {
        let detail = |copy: &str| {
            copy.split_once("\ndetails: ")
                .expect("detail line")
                .1
                .to_owned()
        };
        let copy = installation_recovery_copy("binding\\detail\n\x1b");
        assert_eq!(detail(&copy), "binding\\\\detail\\n\\x1b");

        let oversized = installation_recovery_copy(&"\x1b".repeat(1025));
        assert!(oversized.ends_with("…[truncated]"));
        assert_eq!(detail(&oversized).chars().count(), 2048);
    }
}
